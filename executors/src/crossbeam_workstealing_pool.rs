// Copyright 2017-2020 Lars Kroll. See the LICENSE
// file at the top-level directory of this distribution.
//
// Licensed under the MIT license
// <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed
// except according to those terms.

//! A thread pool `Executor` used to execute functions in parallel.
//!
//! Spawns a specified number of worker threads and replenishes the pool
//! if any worker threads panic.
//!
//! The pool automatically shuts down all workers when the last handle
//! is dropped.
//!
//! The interface is compatible with the
//! standard [threadpool](https://crates.io/crates/threadpool), but the
//! implementation runs faster, especially with multiple workers.
//!
//! Uses per-thread local queues
//! (based on [crossbeam-deque](https://crates.io/crates/crossbeam-deque))
//! for internal scheduling (like a fork-join-pool) and a global queue
//! (based on [crossbeam-channel](https://crates.io/crates/crossbeam-channel))
//! for scheduling from external (non-worker) threads.
//!
//! There are two ways of managing fairness between local and global queues:
//!
//! - Timeout based: Check global queue at most every 1ms
//! (activated via the `ws-timed-fairness` feature, which is in the
//! default feature set)
//! - Job count based: Check global every 100 local jobs
//! (used if the `ws-timed-fairness` feature is disabled)
//!
//! Timeout based fairness is more predictable, as it is less dependent
//! on the job execution time. But if a platform implementation of
//! `time::precise_time_ns` locks across threads, it would have
//! significant performance impact.
//!
//! # Examples
//!
//! ## Synchronized with a channel
//!
//! Every thread sends one message over the channel, which then is collected with the `take()`.
//!
//! ```
//! use executors::*;
//! use executors::crossbeam_workstealing_pool;
//! use std::sync::mpsc::channel;
//!
//! let n_workers = 4;
//! let n_jobs = 8;
//! let pool = crossbeam_workstealing_pool::small_pool(n_workers);
//!
//! let (tx, rx) = channel();
//! for _ in 0..n_jobs {
//!     let tx = tx.clone();
//!     pool.execute(move|| {
//!         tx.send(1).expect("channel will be there waiting for the pool");
//!     });
//! }
//!
//! assert_eq!(rx.iter().take(n_jobs).fold(0, |a, b| a + b), 8);
//! # pool.shutdown();
//! ```

use super::*;
#[cfg(feature = "numa-aware")]
use crate::numa_utils::{equidistance, ProcessingUnitDistance};
#[cfg(not(feature = "ws-no-park"))]
use crate::parker::ParkResult;
use crate::{
    futures_executor::FuturesExecutor,
    parker,
    parker::{DynParker, Parker},
};
#[cfg(feature = "thread-pinning")]
use core_affinity::CoreId;
use crossbeam_channel as channel;
use crossbeam_deque as deque;
use crossbeam_utils::Backoff;
use rand::prelude::*;
#[cfg(feature = "ws-timed-fairness")]
use std::time::Instant;
use std::{
    cell::UnsafeCell,
    collections::BTreeMap,
    fmt::{Debug, Formatter},
    future::Future,
    ops::Deref,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
        Mutex,
        Weak,
    },
    thread,
    time::Duration,
    vec::Vec,
};

/// Creates a thread pool with support for up to 32 threads.
///
/// This a convenience function, which is equivalent to `ThreadPool::new(threads, parker::small())`.
pub fn small_pool(threads: usize) -> ThreadPool<parker::StaticParker<parker::SmallThreadData>> {
    let p = parker::small();
    ThreadPool::new(threads, p)
}
/// Creates a thread pool with support for up to 64 threads.
///
/// This a convenience function, which is equivalent to `ThreadPool::new(threads, parker::large())`.
pub fn large_pool(threads: usize) -> ThreadPool<parker::StaticParker<parker::LargeThreadData>> {
    let p = parker::large();
    ThreadPool::new(threads, p)
}

/// Creates a thread pool with support for an arbitrary* number threads.
///
/// (*Well, technically the limit is something like `std::usize::MAX`.)
///
/// This a convenience function, which is equivalent to `ThreadPool::new(threads, parker::dyamic())`.
pub fn dyn_pool(threads: usize) -> ThreadPool<parker::StaticParker<parker::DynamicThreadData>> {
    let p = parker::dynamic();
    ThreadPool::new(threads, p)
}

/// Creates a new thread pool capable of executing `threads` number of jobs concurrently.
///
/// Decides on the parker implementation automatically, but relies on dynamic dispatch to abstract
/// over the chosen implementation, which comes with a slight performance cost.
///
/// # Panics
///
/// This function will panic if `threads` is 0.
///
/// # Examples
///
/// Create a new thread pool capable of executing four jobs concurrently:
///
/// ```
/// use executors::*;
/// use executors::crossbeam_workstealing_pool;
///
/// let pool = crossbeam_workstealing_pool::pool_with_auto_parker(4);
/// # pool.shutdown();
/// ```
pub fn pool_with_auto_parker(threads: usize) -> ThreadPool<DynParker> {
    if threads <= parker::SmallThreadData::MAX_THREADS {
        let p = parker::small().dynamic().unwrap();
        ThreadPool::new(threads, p)
    } else if threads <= parker::LargeThreadData::MAX_THREADS {
        let p = parker::large().dynamic().unwrap();
        ThreadPool::new(threads, p)
    } else {
        let p = parker::dynamic().dynamic().unwrap();
        ThreadPool::new(threads, p)
    }
}

#[cfg(feature = "ws-timed-fairness")]
const CHECK_GLOBAL_INTERVAL_NS: u32 = timeconstants::NS_PER_MS;
#[cfg(not(feature = "ws-timed-fairness"))]
const CHECK_GLOBAL_MAX_MSGS: u64 = 100;
const MAX_WAIT_SHUTDOWN_MS: u64 = 5 * (timeconstants::MS_PER_S as u64);

// UnsafeCell has 10x the performance of RefCell
// and the scoping guarantees that the borrows are exclusive
thread_local!(
    static LOCAL_JOB_QUEUE: UnsafeCell<Option<deque::Worker<Job>>> = UnsafeCell::new(Option::None);
);

/// Try to append the job to the thread-local job queue
///
/// This only work if called from a thread that is part of the pool.
/// Otherwise the job will be returned in an `Err` variant.
pub fn execute_locally<F>(job: F) -> Result<(), F>
where
    F: FnOnce() + Send + 'static,
{
    LOCAL_JOB_QUEUE.with(|qo| unsafe {
        match *qo.get() {
            Some(ref q) => {
                let msg = Job::Function(Box::new(job));
                q.push(msg);

                #[cfg(feature = "produce-metrics")]
                increment_gauge!("executors.jobs_queued", 1.0, "executor" => "crossbeam_workstealing_pool");

                Ok(())
            }
            None => Err(job),
        }
    })
}

/// A handle associated with the thread pool structure
#[derive(Clone, Debug)]
pub struct ThreadPool<P>
where
    P: Parker + Clone + 'static,
{
    inner: Arc<ThreadPoolInner<P>>,
}

#[derive(Debug)]
struct ThreadPoolInner<P>
where
    P: Parker + Clone + 'static,
{
    core: Mutex<ThreadPoolCore>,
    global_sender: deque::Injector<Job>,
    threads: usize,
    shutdown: AtomicBool,
    parker: P,
    #[cfg(feature = "numa-aware")]
    pu_distance: ProcessingUnitDistance,
}

#[derive(Debug)]
struct ThreadPoolCore {
    ids: usize,
    workers: BTreeMap<usize, WorkerEntry>,
}

impl<P> ThreadPool<P>
where
    P: Parker + Clone + 'static,
{
    /// Creates a new thread pool capable of executing `threads` number of jobs concurrently.
    ///
    /// Must supply a `parker` that can handle the requested number of threads.
    ///
    /// # Panics
    ///
    /// - This function will panic if `threads` is 0.
    /// - It will also panic if `threads` is larger than the `ThreadData::MAX_THREADS` value of the provided `parker`.
    ///
    /// # Core Affinity
    ///
    /// If compiled with `thread-pinning` it will assign a worker to each cores,
    /// until it runs out of cores or workers. If there are more workers than cores,
    /// the extra workers will be "floating", i.e. not pinned.
    ///
    /// # Examples
    ///
    /// Create a new thread pool capable of executing four jobs concurrently:
    ///
    /// ```
    /// use executors::*;
    /// use executors::crossbeam_workstealing_pool::ThreadPool;
    ///
    /// let pool = ThreadPool::new(4, parker::small());
    /// # pool.shutdown();
    /// ```
    pub fn new(threads: usize, parker: P) -> ThreadPool<P> {
        assert!(threads > 0);
        if let Some(max) = parker.max_threads() {
            assert!(threads <= max);
        }
        let core = ThreadPoolCore::new();
        #[cfg(feature = "numa-aware")]
        let pu_distance = {
            let num_pus = core_affinity::get_core_ids().expect("CoreIds").len(); // this should work, if not we'd fail below anyway
            ProcessingUnitDistance::from_function(num_pus, equidistance)
        };
        let inner = ThreadPoolInner {
            core: Mutex::new(core),
            global_sender: deque::Injector::new(),
            threads,
            shutdown: AtomicBool::new(false),
            parker,
            #[cfg(feature = "numa-aware")]
            pu_distance,
        };
        let pool = ThreadPool {
            inner: Arc::new(inner),
        };
        #[cfg(not(feature = "thread-pinning"))]
        {
            let mut guard = pool.inner.core.lock().unwrap();
            for _ in 0..threads {
                pool.inner
                    .spawn_worker(&mut guard, pool.inner.clone(), None);
            }
        }
        #[cfg(feature = "thread-pinning")]
        {
            let mut guard = pool.inner.core.lock().unwrap();
            let cores = core_affinity::get_core_ids().expect("core ids");
            let num_pinned = cores.len().min(threads);
            for core in cores.iter().take(num_pinned) {
                pool.inner
                    .spawn_worker_pinned(&mut guard, pool.inner.clone(), None, *core);
            }
            if num_pinned < threads {
                let num_unpinned = threads - num_pinned;
                for _ in 0..num_unpinned {
                    pool.inner
                        .spawn_worker(&mut guard, pool.inner.clone(), None);
                }
            }
        }
        pool
    }

    /// Creates a new thread pool capable of executing `threads` number of jobs concurrently with a particular core affinity.
    ///
    /// For each core id in the `core` slice, it will generate a single thread pinned to that id.
    /// Additionally, it will create `floating` number of unpinned threads.
    ///
    /// # Panics
    ///
    /// - This function will panic if `cores.len() + floating` is 0.
    /// - This function will panic if `cores.len() + floating` is greater than `parker.max_threads()`.
    /// - This function will panic if no core ids can be accessed.
    #[cfg(feature = "thread-pinning")]
    pub fn with_affinity(cores: &[CoreId], floating: usize, parker: P) -> ThreadPool<P> {
        let total_threads = cores.len() + floating;
        assert!(total_threads > 0);
        if let Some(max) = parker.max_threads() {
            assert!(total_threads <= max);
        }
        let core = ThreadPoolCore::new();
        #[cfg(feature = "numa-aware")]
        let pu_distance = {
            let num_pus = core_affinity::get_core_ids().expect("CoreIds").len(); // this should work if cores is supplied
            ProcessingUnitDistance::from_function(num_pus, equidistance)
        };
        let inner = ThreadPoolInner {
            core: Mutex::new(core),
            global_sender: deque::Injector::new(),
            threads: total_threads,
            shutdown: AtomicBool::new(false),
            parker,
            #[cfg(feature = "numa-aware")]
            pu_distance,
        };
        let pool = ThreadPool {
            inner: Arc::new(inner),
        };
        {
            let mut guard = pool.inner.core.lock().unwrap();
            cores.iter().for_each(|core_id| {
                pool.inner
                    .spawn_worker_pinned(&mut guard, pool.inner.clone(), None, *core_id);
            });
            for _ in 0..floating {
                pool.inner
                    .spawn_worker(&mut guard, pool.inner.clone(), None);
            }
        }
        pool
    }

    /// Creates a new thread pool capable of executing `threads` number of jobs concurrently with a particular core affinity.
    ///
    /// For each core id in the `core` slice, it will generate a single thread pinned to that id.
    /// Additionally, it will create `floating` number of unpinned threads.
    ///
    /// Internally the stealers will use the provided PU distance matrix to prioritise stealing from
    /// queues that are "closer" by, in order to try and reduce memory movement across caches and NUMA nodes.
    ///
    /// # Panics
    ///
    /// - This function will panic if `cores.len() + floating` is 0.
    /// - This function will panic if `cores.len() + floating` is greater than `parker.max_threads()`.
    /// - This function will panic if no core ids can be accessed.
    #[cfg(feature = "numa-aware")]
    pub fn with_numa_affinity(
        cores: &[CoreId],
        floating: usize,
        parker: P,
        pu_distance: ProcessingUnitDistance,
    ) -> ThreadPool<P> {
        let total_threads = cores.len() + floating;
        assert!(total_threads > 0);
        if let Some(max) = parker.max_threads() {
            assert!(total_threads <= max);
        }
        let core = ThreadPoolCore::new();
        let inner = ThreadPoolInner {
            core: Mutex::new(core),
            global_sender: deque::Injector::new(),
            threads: total_threads,
            shutdown: AtomicBool::new(false),
            parker,
            pu_distance,
        };
        let pool = ThreadPool {
            inner: Arc::new(inner),
        };
        {
            let mut guard = pool.inner.core.lock().unwrap();
            cores.iter().for_each(|core_id| {
                pool.inner
                    .spawn_worker_pinned(&mut guard, pool.inner.clone(), None, *core_id);
            });
            for _ in 0..floating {
                pool.inner
                    .spawn_worker(&mut guard, pool.inner.clone(), None);
            }
        }
        pool
    }

    fn schedule_task(&self, task: async_task::Runnable) -> () {
        // schedule tasks always, even if the pool is already stopped, since it's unsafe to drop from the schedule function
        // this might lead to some "memory leaks" if an executor remains stopped but allocated for a long time
        LOCAL_JOB_QUEUE.with(|qo| unsafe {
            let msg = Job::Task(task);
            match *qo.get() {
                Some(ref q) => {
                    q.push(msg);

                    #[cfg(feature = "produce-metrics")]
                    increment_gauge!("executors.jobs_queued", 1.0, "executor" => "crossbeam_workstealing_pool");
                }
                None => {
                    debug!("Scheduling on global pool.");
                    self.inner.global_sender.push(msg);

                    #[cfg(not(feature = "ws-no-park"))]
                    self.inner.parker.unpark_one();

                    #[cfg(feature = "produce-metrics")]
                    increment_gauge!("executors.jobs_queued", 1.0, "executor" => "crossbeam_workstealing_pool");
                }
            }
        })
    }
}

/// Create a thread pool with one thread per CPU.
/// On machines with hyperthreading,
/// this will create one thread per hyperthread.
#[cfg(feature = "defaults")]
impl Default for ThreadPool<DynParker> {
    fn default() -> Self {
        let p = parker::dynamic().dynamic().unwrap();
        ThreadPool::new(num_cpus::get(), p)
    }
}

//impl !Sync for ThreadPool {}

impl<P> CanExecute for ThreadPool<P>
where
    P: Parker + Clone + 'static,
{
    fn execute_job(&self, job: Box<dyn FnOnce() + Send + 'static>) {
        if !self.inner.shutdown.load(Ordering::Relaxed) {
            LOCAL_JOB_QUEUE.with(|qo| unsafe {
                let msg = Job::Function(job);
                match *qo.get() {
                    Some(ref q) => {
                        q.push(msg);

                        #[cfg(feature = "produce-metrics")]
                        increment_gauge!("executors.jobs_queued", 1.0, "executor" => "crossbeam_workstealing_pool");
                    }
                    None => {
                        debug!("Scheduling on global pool.");
                        self.inner.global_sender.push(msg);

                        #[cfg(not(feature = "ws-no-park"))]
                        self.inner.parker.unpark_one();

                        #[cfg(feature = "produce-metrics")]
                        increment_gauge!("executors.jobs_queued", 1.0, "executor" => "crossbeam_workstealing_pool");
                    }
                }
            })
        } else {
            warn!("Ignoring job as pool is shutting down.")
        }
    }
}

impl<P> Executor for ThreadPool<P>
where
    P: Parker + Clone + 'static,
{
    fn shutdown_async(&self) {
        let shutdown_pool = self.clone();
        // Need to make sure that pool is shut down before drop is called.
        thread::spawn(move || {
            shutdown_pool.shutdown().expect("Pool didn't shut down");
        });
    }

    fn shutdown_borrowed(&self) -> Result<(), String> {
        if self
            .inner
            .shutdown
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            let latch = Arc::new(CountdownEvent::new(self.inner.threads));
            debug!("Shutting down {} threads", self.inner.threads);
            {
                let guard = self.inner.core.lock().unwrap();
                for worker in guard.workers.values() {
                    worker
                        .control
                        .send(ControlMsg::Stop(latch.clone()))
                        .unwrap_or_else(|e| error!("Error submitting Stop msg: {:?}", e));
                }
            }
            #[cfg(not(feature = "ws-no-park"))]
            self.inner.parker.unpark_all(); // gotta wake up all threads so they can get shut down
            let remaining = latch.wait_timeout(Duration::from_millis(MAX_WAIT_SHUTDOWN_MS));
            if remaining == 0 {
                debug!("All threads shut down");
                Result::Ok(())
            } else {
                Result::Err(format!(
                    "Pool failed to shut down in time. {:?} threads remaining.",
                    remaining
                ))
            }
        } else {
            Result::Err(String::from("Pool is already shutting down!"))
        }
    }

    #[cfg(feature = "produce-metrics")]
    fn register_metrics(&self) {
        register_counter!("executors.jobs_executed", "The total number of jobs that were executed", "executor" => "crossbeam_workstealing_pool");
        register_gauge!("executors.jobs_queued", "The number of jobs that are currently waiting to be executed", "executor" => "crossbeam_workstealing_pool");
    }
}

impl<P> FuturesExecutor for ThreadPool<P>
where
    P: Parker + Clone + 'static,
{
    fn spawn<R: Send + 'static>(
        &self,
        future: impl Future<Output = R> + 'static + Send,
    ) -> JoinHandle<R> {
        let exec = self.clone();
        let (task, handle) = async_task::spawn(future, move |task| {
            exec.schedule_task(task);
        });
        task.schedule();
        handle
    }
}

impl<P> ThreadPoolInner<P>
where
    P: Parker + Clone + 'static,
{
    fn spawn_worker(
        &self,
        guard: &mut std::sync::MutexGuard<'_, ThreadPoolCore>,
        inner: Arc<Self>,
        old_id: Option<usize>,
    ) {
        let id = old_id.unwrap_or_else(|| guard.new_worker_id());
        let (tx_control, rx_control) = channel::unbounded();
        let worker = WorkerEntry {
            control: tx_control,
            stealer: Option::None,
        };
        guard.workers.insert(id, worker);
        thread::Builder::new()
            .name(format!("cb-ws-pool-worker-{}", id))
            .spawn(move || {
                #[cfg(feature = "thread-pinning")]
                let mut worker = ThreadPoolWorker::new(id, rx_control, &inner, None);
                #[cfg(not(feature = "thread-pinning"))]
                let mut worker = ThreadPoolWorker::new(id, rx_control, &inner);
                drop(inner);
                worker.run()
            })
            .expect("Could not create worker thread!");
    }

    #[cfg(feature = "thread-pinning")]
    fn spawn_worker_pinned(
        &self,
        guard: &mut std::sync::MutexGuard<'_, ThreadPoolCore>,
        inner: Arc<Self>,
        old_id: Option<usize>,
        core_id: CoreId,
    ) {
        let id = old_id.unwrap_or_else(|| guard.new_worker_id());
        let (tx_control, rx_control) = channel::unbounded();
        let worker = WorkerEntry {
            control: tx_control,
            stealer: Option::None,
        };
        guard.workers.insert(id, worker);
        thread::Builder::new()
            .name(format!("cb-ws-pool-worker-{}", id))
            .spawn(move || {
                core_affinity::set_for_current(core_id);
                let mut worker = ThreadPoolWorker::new(id, rx_control, &inner, Some(core_id));
                drop(inner);
                worker.run()
            })
            .expect("Could not create worker thread!");
    }
}

impl<P> Drop for ThreadPoolInner<P>
where
    P: Parker + Clone + 'static,
{
    fn drop(&mut self) {
        if !self.shutdown.load(Ordering::SeqCst) {
            warn!("Threadpools should be shut down before deallocating.");
            // the threads won't leak, as they'll get errors on their control queues next time they check
            self.parker.unpark_all(); // must wake them all up so they don't idle forever
        }
    }
}

#[derive(Debug)]
struct WorkerEntry {
    control: channel::Sender<ControlMsg>,
    stealer: Option<JobStealer>,
}

impl ThreadPoolCore {
    fn new() -> Self {
        ThreadPoolCore {
            ids: 0,
            workers: BTreeMap::new(),
        }
    }

    fn new_worker_id(&mut self) -> usize {
        let id = self.ids;
        self.ids += 1;
        id
    }

    fn add_stealer(&mut self, stealer: JobStealer) {
        let id = stealer.id();
        match self.workers.get_mut(&id) {
            Some(worker) => {
                debug!("Registered stealer for #{}", id);
                worker.stealer = Some(stealer);
            }
            None => panic!("Adding stealer for non-existant worker!"),
        }
        self.send_stealers();
    }

    fn drop_worker(&mut self, id: usize) {
        debug!("Dropping worker #{}", id);
        self.workers
            .remove(&id)
            .expect("Key should have been present!");
        self.send_stealers();
    }

    fn send_stealers(&mut self) {
        let stealers: Vec<JobStealer> = self
            .workers
            .values()
            .filter(|w| w.stealer.is_some())
            .map(|w| w.stealer.clone().unwrap())
            .collect();
        for (wid, worker) in self.workers.iter() {
            let l: Vec<JobStealer> = stealers
                .iter()
                .filter(|s| s.id() != *wid)
                .cloned()
                .collect();
            worker
                .control
                .send(ControlMsg::Stealers(l))
                .unwrap_or_else(|e| error!("Error submitting Stealer msg: {:?}", e));
        }
    }
}

struct ThreadLocalExecute;
impl CanExecute for ThreadLocalExecute {
    fn execute_job(&self, job: Box<dyn FnOnce() + Send + 'static>) {
        LOCAL_JOB_QUEUE.with(|qo| unsafe {
            let msg = Job::Function(job);
            match *qo.get() {
                Some(ref q) => {
                    q.push(msg);

                    #[cfg(feature = "produce-metrics")]
                    increment_gauge!("executors.jobs_queued", 1.0, "executor" => "crossbeam_workstealing_pool");
                }
                None => {
                    unreachable!("Local Executor was set but local job queue was not!");
                }
            }
        })
    }
}

#[derive(Debug)]
struct ThreadPoolWorker<P>
where
    P: Parker + Clone + 'static,
{
    id: usize,
    core: Weak<ThreadPoolInner<P>>,
    control: channel::Receiver<ControlMsg>,
    stealers: Vec<(i32, JobStealer)>,
    random: ThreadRng,
    #[cfg(feature = "thread-pinning")]
    core_id: Option<CoreId>,
}

impl<P> ThreadPoolWorker<P>
where
    P: Parker + Clone + 'static,
{
    fn new(
        id: usize,
        control: channel::Receiver<ControlMsg>,
        core: &Arc<ThreadPoolInner<P>>,
        #[cfg(feature = "thread-pinning")] core_id: Option<CoreId>,
    ) -> ThreadPoolWorker<P> {
        ThreadPoolWorker {
            id,
            core: Arc::downgrade(core),
            control,
            stealers: Vec::new(),
            random: rand::thread_rng(),
            #[cfg(feature = "thread-pinning")]
            core_id,
        }
    }

    #[inline(always)]
    fn id(&self) -> &usize {
        &self.id
    }

    #[inline(always)]
    #[cfg(feature = "ws-no-park")]
    fn abort_sleep(
        &self,
        backoff: &Backoff,
        snoozing: &mut bool,
        failed_steal_attempts: &mut i32,
    ) -> () {
        if *snoozing {
            backoff.reset();
            *snoozing = false;
        }
        *failed_steal_attempts = 0;
    }

    #[inline(always)]
    #[cfg(not(feature = "ws-no-park"))]
    fn abort_sleep(
        &self,
        core: &Arc<ThreadPoolInner<P>>,
        backoff: &Backoff,
        snoozing: &mut bool,
        parking: &mut bool,
        failed_steal_attempts: &mut i32,
    ) -> () {
        if *parking {
            core.parker.abort_park(*self.id());
            *parking = false;
        }
        if *snoozing {
            backoff.reset();
            *snoozing = false;
        }
        *failed_steal_attempts = 0;
    }

    #[inline(always)]
    #[cfg(not(feature = "ws-no-park"))]
    fn stop_sleep(&self, backoff: &Backoff, snoozing: &mut bool, parking: &mut bool) -> () {
        backoff.reset();
        *snoozing = false;
        *parking = false;
    }

    fn run(&mut self) {
        debug!("Worker {} starting", self.id());

        let local_stealer_raw = LOCAL_JOB_QUEUE.with(|q| unsafe {
            let worker = deque::Worker::new_fifo();
            let stealer = worker.stealer();
            *q.get() = Some(worker);
            stealer
        });
        set_local_executor(ThreadLocalExecute);
        let core = self.core.upgrade().expect("Core shut down already!");
        //info!("Setting up new worker {}. strong={}, weak={}", self.id(), Arc::strong_count(&core), Arc::weak_count(&core));

        #[cfg(feature = "numa-aware")]
        let local_stealer = JobStealer::new(local_stealer_raw, self.id, self.core_id);
        #[cfg(not(feature = "numa-aware"))]
        let local_stealer = JobStealer::new(local_stealer_raw, self.id);
        self.register_stealer(&core, local_stealer);
        core.parker.init(*self.id());
        drop(core);
        let sentinel = Sentinel::new(self.core.clone(), self.id);
        let backoff = Backoff::new();
        let mut snoozing = false;
        #[cfg(not(feature = "ws-no-park"))]
        let mut parking = false;
        let mut failed_steal_attempts: i32 = 0;
        let mut stop_latch: Option<Arc<CountdownEvent>>;
        'main: loop {
            //info!("Worker {} starting main loop", self.id());
            let mut fairness_check = false;
            #[cfg(feature = "ws-timed-fairness")]
            let next_global_check = Instant::now() + Duration::new(0, CHECK_GLOBAL_INTERVAL_NS);
            // Try the local queue usually
            LOCAL_JOB_QUEUE.with(|q| unsafe {
                if let Some(ref local_queue) = *q.get() {

                    #[allow(unused_variables)]
                    let mut count = 0u64;

                    'local: loop {
                        match local_queue.pop() {
                            Some(d) => {
                                d.run();
                            }
                            None => break 'local,
                        }
                        backoff.reset();
                        count += 1;

                        #[cfg(not(feature = "ws-timed-fairness"))]
                        {
                            if count >= CHECK_GLOBAL_MAX_MSGS {
                                fairness_check = true;
                                break 'local;
                            }
                        }
                        #[cfg(feature = "ws-timed-fairness")]
                        {
                            if (Instant::now() > next_global_check) {
                                fairness_check = true;
                                break 'local;
                            }
                        }
                    }

                    #[cfg(feature = "produce-metrics")]
                    {
                        counter!("executors.jobs_executed", count, "executor" => "crossbeam_workstealing_pool", "thread_id" => format!("{}", self.id()));
                        decrement_gauge!("executors.jobs_queued", count as f64, "executor" => "crossbeam_workstealing_pool");
                    }
                } else {
                    panic!("Queue should have been initialised!");
                }
            });

            let core = self.core.upgrade().expect("Core shut down already!");
            //info!("Worker {} taking new core handle. strong={}, weak={}", self.id(), Arc::strong_count(&core), Arc::weak_count(&core));
            // drain the control queue
            'ctrl: loop {
                match self.control.try_recv() {
                    Ok(msg) => match msg {
                        ControlMsg::Stealers(l) => {
                            #[cfg(feature = "numa-aware")]
                            self.update_stealers(&core, l);
                            #[cfg(not(feature = "numa-aware"))]
                            self.update_stealers(l);

                            #[cfg(feature = "ws-no-park")]
                            self.abort_sleep(&backoff, &mut snoozing, &mut failed_steal_attempts);
                            #[cfg(not(feature = "ws-no-park"))]
                            self.abort_sleep(
                                &core,
                                &backoff,
                                &mut snoozing,
                                &mut parking,
                                &mut failed_steal_attempts,
                            );
                        }
                        ControlMsg::Stop(latch) => {
                            stop_latch = Some(latch);
                            break 'main;
                        }
                    },
                    Err(channel::TryRecvError::Empty) => break 'ctrl,
                    Err(channel::TryRecvError::Disconnected) => {
                        drop(core);
                        warn!("Worker {} self-terminating.", self.id());
                        sentinel.cancel();
                        panic!("Threadpool wasn't shut down properly!");
                    }
                }
            }
            // sometimes try the global queue
            let glob_res = LOCAL_JOB_QUEUE.with(|q| unsafe {
                if let Some(ref local_queue) = *q.get() {
                    core.global_sender.steal_batch_and_pop(local_queue)
                } else {
                    panic!("Queue should have been initialised!");
                }
            });
            if let deque::Steal::Success(msg) = glob_res {
                msg.run();

                #[cfg(feature = "ws-no-park")]
                self.abort_sleep(&backoff, &mut snoozing, &mut failed_steal_attempts);
                #[cfg(not(feature = "ws-no-park"))]
                self.abort_sleep(
                    &core,
                    &backoff,
                    &mut snoozing,
                    &mut parking,
                    &mut failed_steal_attempts,
                );

                #[cfg(feature = "produce-metrics")]
                {
                    increment_counter!("executors.jobs_executed", "executor" => "crossbeam_workstealing_pool", "thread_id" => format!("{}", self.id()));
                    decrement_gauge!("executors.jobs_queued", 1.0, "executor" => "crossbeam_workstealing_pool");
                }

                continue 'main;
            }
            // only go on if there was no work left on the local queue
            if fairness_check {
                #[cfg(not(feature = "ws-no-park"))]
                core.parker.unpark_one(); // wake up more threads if there is more work to do
                continue 'main;
            }
            // try to steal something!
            for (_weight, stealer) in self
                .stealers
                .iter()
                .take_while(|(weight, _)| *weight <= failed_steal_attempts)
            {
                if let deque::Steal::Success(msg) = stealer.steal() {
                    msg.run();
                    #[cfg(feature = "ws-no-park")]
                    self.abort_sleep(&backoff, &mut snoozing, &mut failed_steal_attempts);
                    #[cfg(not(feature = "ws-no-park"))]
                    self.abort_sleep(
                        &core,
                        &backoff,
                        &mut snoozing,
                        &mut parking,
                        &mut failed_steal_attempts,
                    );
                    continue 'main; // only steal once before checking locally again
                }
            }
            failed_steal_attempts = failed_steal_attempts.saturating_add(1);
            snoozing = true;
            #[cfg(feature = "ws-no-park")]
            {
                drop(core);
                backoff.snooze();
            }
            #[cfg(not(feature = "ws-no-park"))]
            {
                if parking {
                    let parker = core.parker.clone();
                    drop(core);
                    match parker.park(*self.id()) {
                        ParkResult::Retry => (), // just start over
                        ParkResult::Abort | ParkResult::Woken => {
                            self.stop_sleep(&backoff, &mut snoozing, &mut parking);
                        }
                    }
                } else if backoff.is_completed() {
                    core.parker.prepare_park(*self.id());
                    parking = true;
                } else {
                    drop(core);
                    backoff.snooze();
                }
            }
            // aaaaand starting over with 'local
        }
        sentinel.cancel();
        let core = self.core.upgrade().expect("Core shut down already!");
        self.unregister_stealer(&core);
        unset_local_executor();
        LOCAL_JOB_QUEUE.with(|q| unsafe {
            *q.get() = None;
        });
        debug!("Worker {} shutting down", self.id());
        drop(core);
        if let Some(latch) = stop_latch.take() {
            latch.decrement().expect("stop latch decrements");
        }
    }

    fn register_stealer(&mut self, core: &Arc<ThreadPoolInner<P>>, stealer: JobStealer) {
        let mut guard = core.core.lock().unwrap();
        guard.add_stealer(stealer);
    }

    fn unregister_stealer(&mut self, core: &Arc<ThreadPoolInner<P>>) {
        let mut guard = core.core.lock().unwrap();
        guard.drop_worker(self.id);
    }

    #[cfg(not(feature = "numa-aware"))]
    fn update_stealers(&mut self, v: Vec<JobStealer>) {
        let mut weighted: Vec<(i32, JobStealer)> =
            v.into_iter().map(|stealer| (0, stealer)).collect();
        weighted.shuffle(&mut self.random);
        self.stealers = weighted;
    }

    #[cfg(feature = "numa-aware")]
    fn update_stealers(&mut self, _core: &ThreadPoolInner<P>, v: Vec<JobStealer>) {
        let mut weighted: Vec<(i32, JobStealer)> = if let Some(my_id) = self.core_id {
            let distances = &_core.pu_distance;
            v.into_iter()
                .map(|stealer| {
                    let weight = if let Some(stealer_id) = stealer.core_id() {
                        distances.distance(my_id, *stealer_id)
                    } else {
                        std::i32::MIN
                    };
                    (weight, stealer)
                })
                .collect()
        } else {
            v.into_iter().map(|stealer| (0, stealer)).collect()
        };
        weighted.shuffle(&mut self.random);
        weighted.sort_unstable_by_key(|(weight, _stealer)| *weight);
        self.stealers = weighted;
    }
}

#[derive(Clone)]
struct JobStealer {
    inner: deque::Stealer<Job>,
    id: usize,
    #[cfg(feature = "numa-aware")]
    core_id: Option<CoreId>,
}

impl JobStealer {
    fn new(
        stealer: deque::Stealer<Job>,
        id: usize,
        #[cfg(feature = "numa-aware")] core_id: Option<CoreId>,
    ) -> JobStealer {
        JobStealer {
            inner: stealer,
            id,
            #[cfg(feature = "numa-aware")]
            core_id,
        }
    }

    fn id(&self) -> usize {
        self.id
    }

    #[cfg(feature = "numa-aware")]
    fn core_id(&self) -> &Option<CoreId> {
        &self.core_id
    }
}

impl PartialEq for JobStealer {
    fn eq(&self, other: &JobStealer) -> bool {
        self.id == other.id
    }
}

impl Eq for JobStealer {}

impl Deref for JobStealer {
    type Target = deque::Stealer<Job>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl Debug for JobStealer {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "JobStealer#{}(_)", self.id)
    }
}

//struct Job(Box<dyn FnOnce() + Send + 'static>);
enum Job {
    Function(Box<dyn FnOnce() + Send + 'static>),
    Task(async_task::Runnable),
}
impl Job {
    fn run(self) {
        match self {
            Job::Function(f) => f(),
            Job::Task(t) => {
                let _ = t.run();
            }
        }
    }
}

enum ControlMsg {
    Stealers(Vec<JobStealer>),
    Stop(Arc<CountdownEvent>),
}

impl Debug for ControlMsg {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match *self {
            ControlMsg::Stealers(ref l) => write!(f, "Stealers({:?})", l),
            ControlMsg::Stop(_) => write!(f, "Stop(_)"),
        }
    }
}

struct Sentinel<P>
where
    P: Parker + Clone + 'static,
{
    core: Weak<ThreadPoolInner<P>>,
    id: usize,
    active: bool,
}

impl<P> Sentinel<P>
where
    P: Parker + Clone + 'static,
{
    fn new(core: Weak<ThreadPoolInner<P>>, id: usize) -> Sentinel<P> {
        Sentinel {
            core,
            id,
            active: true,
        }
    }

    // Cancel and destroy this sentinel.
    fn cancel(mut self) {
        self.active = false;
    }
}

impl<P> Drop for Sentinel<P>
where
    P: Parker + Clone + 'static,
{
    fn drop(&mut self) {
        if self.active {
            warn!("Active worker {} died! Restarting...", self.id);
            match self.core.upgrade() {
                Some(core) => {
                    let jobs = LOCAL_JOB_QUEUE.with(|q| unsafe {
                        let mut jobs: Vec<Job> = Vec::new();
                        if let Some(ref local_queue) = *q.get() {
                            'drain: loop {
                                match local_queue.pop() {
                                    Some(d) => jobs.push(d),
                                    None => break 'drain,
                                }
                            }
                        }
                        jobs
                    });
                    let mut guard = core.core.lock().unwrap();
                    // cleanup
                    guard.drop_worker(self.id);
                    // restart
                    // make sure the new thread starts with the same worker id, so the parker doesn't run out of slots
                    core.spawn_worker(&mut guard, core.clone(), Some(self.id));
                    drop(guard);
                    for job in jobs.into_iter() {
                        core.global_sender.push(job);
                    }
                }
                None => warn!("Could not restart worker, as pool has been deallocated!"),
            }
        }
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    const LABEL: &str = "Workstealing Pool";

    #[test]
    fn test_debug() {
        let exec = ThreadPool::new(2, parker::small());
        crate::tests::test_debug(&exec, LABEL);
        exec.shutdown().expect("Pool didn't shut down!");
    }

    #[test]
    fn test_sleepy_small() {
        let exec = ThreadPool::new(4, parker::small());
        crate::tests::test_sleepy(exec, LABEL);
    }

    #[test]
    fn test_sleepy_large() {
        let exec = ThreadPool::new(36, parker::large());
        crate::tests::test_sleepy(exec, LABEL);
    }

    #[test]
    fn test_sleepy_dyn() {
        let exec = ThreadPool::new(72, parker::dynamic());
        crate::tests::test_sleepy(exec, LABEL);
    }

    #[test]
    fn test_custom_small() {
        let exec = small_pool(4);
        crate::tests::test_custom(exec, LABEL);
    }

    #[test]
    fn test_custom_large() {
        let exec = large_pool(36);
        crate::tests::test_custom(exec, LABEL);
    }

    #[test]
    fn test_custom_dyn() {
        let exec = dyn_pool(72);
        crate::tests::test_custom(exec, LABEL);
    }

    #[test]
    fn test_custom_auto_large() {
        let exec = pool_with_auto_parker(36);
        crate::tests::test_custom(exec, LABEL);
    }

    #[test]
    fn test_custom_auto_dyn() {
        let exec = pool_with_auto_parker(72);
        crate::tests::test_custom(exec, LABEL);
    }

    #[test]
    fn test_local() {
        let exec = ThreadPool::default();
        crate::tests::test_local(exec, LABEL);
    }

    #[cfg(feature = "thread-pinning")]
    #[test]
    fn test_custom_affinity() {
        let cores = core_affinity::get_core_ids().expect("core ids");
        // travis doesn't have enough cores for this
        //let exec = ThreadPool::with_affinity(&cores[0..4], 4, parker::small());
        let exec = ThreadPool::with_affinity(&cores[0..1], 1, parker::small());
        crate::tests::test_custom(exec, LABEL);
    }

    #[cfg(feature = "numa-aware")]
    #[test]
    fn test_custom_numa_affinity() {
        let cores = core_affinity::get_core_ids().expect("core ids");
        // travis doesn't have enough cores for this
        //let exec = ThreadPool::with_affinity(&cores[0..4], 4, parker::small());
        let distances = ProcessingUnitDistance::from_function(cores.len(), equidistance);
        let exec = ThreadPool::with_numa_affinity(&cores[0..1], 1, parker::small(), distances);
        crate::tests::test_custom(exec, LABEL);
    }

    #[test]
    fn test_defaults() {
        crate::tests::test_defaults::<ThreadPool<DynParker>>(LABEL);
    }

    #[test]
    fn run_with_two_threads() {
        let _ = env_logger::try_init();

        let latch = Arc::new(CountdownEvent::new(2));
        let pool = ThreadPool::new(2, parker::small());
        let latch2 = latch.clone();
        let latch3 = latch.clone();
        pool.execute(move || {
            let _ = latch2.decrement();
        });
        pool.execute(move || {
            let _ = latch3.decrement();
        });
        let res = latch.wait_timeout(Duration::from_secs(5));
        assert_eq!(res, 0);
        pool.shutdown()
            .unwrap_or_else(|e| error!("Error during pool shutdown {:?}", e));
    }

    #[test]
    fn amplify() {
        let _ = env_logger::try_init();

        let latch = Arc::new(CountdownEvent::new(4));
        let pool = ThreadPool::new(2, parker::small());
        let pool2 = pool.clone();
        let pool3 = pool.clone();
        let latch2 = latch.clone();
        let latch3 = latch.clone();
        pool.execute(move || {
            let _ = latch2.decrement();
            pool2.execute(move || {
                let _ = latch2.decrement();
            });
        });
        pool.execute(move || {
            let _ = latch3.decrement();
            pool3.execute(move || {
                let _ = latch3.decrement();
            });
        });
        let res = latch.wait_timeout(Duration::from_secs(5));
        assert_eq!(res, 0);
        pool.shutdown()
            .unwrap_or_else(|e| error!("Error during pool shutdown {:?}", e));
    }

    // replace ignore with panic cfg gate when https://github.com/rust-lang/rust/pull/74754 is merged
    #[test]
    #[ignore]
    fn keep_pool_size() {
        let _ = env_logger::try_init();

        let latch = Arc::new(CountdownEvent::new(2));
        let pool = ThreadPool::new(1, parker::small());
        let latch2 = latch.clone();
        let latch3 = latch.clone();
        pool.execute(move || {
            let _ = latch2.decrement();
        });
        pool.execute(move || panic!("test panic please ignore"));
        pool.execute(move || {
            let _ = latch3.decrement();
        });
        let res = latch.wait_timeout(Duration::from_secs(5));
        assert_eq!(res, 0);
        pool.shutdown()
            .unwrap_or_else(|e| error!("Error during pool shutdown {:?}", e));
    }

    // replace ignore with panic cfg gate when https://github.com/rust-lang/rust/pull/74754 is merged
    #[test]
    #[ignore]
    fn reassign_jobs_from_failed_queues() {
        let _ = env_logger::try_init();

        let latch = Arc::new(CountdownEvent::new(2));
        let pool = ThreadPool::new(1, parker::small());
        let pool2 = pool.clone();
        let latch2 = latch.clone();
        let latch3 = latch.clone();
        pool.execute(move || {
            let _ = latch2.decrement();
        });
        pool.execute(move || {
            pool2.execute(move || {
                let _ = latch3.decrement();
            });
            panic!("test panic please ignore")
        });
        let res = latch.wait_timeout(Duration::from_secs(5));
        assert_eq!(res, 0);
        pool.shutdown()
            .unwrap_or_else(|e| error!("Error during pool shutdown {:?}", e));
    }

    #[test]
    fn check_stealers_on_dequeue_drop() {
        let dq: deque::Worker<u64> = deque::Worker::new_fifo();
        let s1 = dq.stealer();
        let s2 = s1.clone();
        dq.push(1);
        dq.push(2);
        dq.push(3);
        dq.push(4);
        dq.push(5);
        assert_eq!(dq.pop(), Some(1));
        assert_eq!(s1.steal(), deque::Steal::Success(2));
        assert_eq!(s2.steal(), deque::Steal::Success(3));
        drop(dq);
        assert_eq!(s1.steal(), deque::Steal::Success(4));
        drop(s1);
        assert_eq!(s2.steal(), deque::Steal::Success(5));
    }

    #[test]
    fn shutdown_from_worker() {
        let _ = env_logger::try_init();

        let pool = ThreadPool::new(1, parker::small());
        let pool2 = pool.clone();
        let latch = Arc::new(CountdownEvent::new(2));
        let latch2 = latch.clone();
        let latch3 = latch.clone();
        let stop_latch = Arc::new(CountdownEvent::new(1));
        let stop_latch2 = stop_latch.clone();
        pool.execute(move || {
            let _ = latch2.decrement();
        });
        pool.execute(move || {
            pool2.shutdown_async();
            let _ = stop_latch2.decrement();
        });
        let res = stop_latch.wait_timeout(Duration::from_secs(1));
        assert_eq!(res, 0);
        thread::sleep(Duration::from_secs(1)); // give the new shutdown thread some time to run
        pool.execute(move || {
            let _ = latch3.decrement();
        });
        let res = latch.wait_timeout(Duration::from_secs(1));
        assert_eq!(res, 1);
    }

    #[test]
    fn shutdown_external() {
        let _ = env_logger::try_init();

        let pool = ThreadPool::new(1, parker::small());
        let pool2 = pool.clone();
        let run_latch = Arc::new(CountdownEvent::new(1));
        let stop_latch = Arc::new(CountdownEvent::new(1));
        let run_latch2 = run_latch.clone();
        let stop_latch2 = stop_latch.clone();
        pool.execute(move || {
            let _ = run_latch2.decrement();
        });
        let res = run_latch.wait_timeout(Duration::from_secs(1));
        assert_eq!(res, 0);
        pool.shutdown().expect("pool to shut down");
        pool2.execute(move || {
            let _ = stop_latch2.decrement();
        });
        let res = stop_latch.wait_timeout(Duration::from_secs(1));
        assert_eq!(res, 1);
    }

    /// This test may/should panic on the worker threads, for example with "Threadpool wasn't shut down properly!".
    /// Otherwise they aren't shut down properly.
    // replace ignore with panic cfg gate when https://github.com/rust-lang/rust/pull/74754 is merged
    #[test]
    #[ignore]
    fn dealloc_on_handle_drop() {
        let _ = env_logger::try_init();

        let pool = ThreadPool::new(1, parker::small());
        let latch = Arc::new(CountdownEvent::new(1));
        let latch2 = latch.clone();
        pool.execute(move || {
            latch2.decrement().expect("Latch should have decremented!");
        });
        let res = latch.wait_timeout(Duration::from_secs(1));
        assert_eq!(res, 0);
        let core_weak = Arc::downgrade(&pool.inner);
        // sleep before drop, to ensure that even parked threads shut down
        std::thread::sleep(std::time::Duration::from_secs(1));
        drop(pool);
        std::thread::sleep(std::time::Duration::from_secs(1));
        if let Some(core) = core_weak.upgrade() {
            println!(
                "Outstanding references: strong={}, weak={}",
                Arc::strong_count(&core),
                Arc::weak_count(&core)
            );
            panic!("Pool should have been dropped!");
        }
    }
}
