// Copyright 2017 Lars Kroll. See the LICENSE
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
//! ```

use super::*;
use crossbeam_channel as channel;
use crossbeam_deque as deque;
use crossbeam_utils::Backoff;
//use std::sync::mpsc;
use crate::parker;
use crate::parker::{DynParker, ParkResult, Parker};
//use num_cpus;
use rand::prelude::*;
use std::cell::UnsafeCell;
use std::collections::BTreeMap;
use std::collections::LinkedList;
use std::fmt::{Debug, Formatter};
use std::iter::{FromIterator, IntoIterator};
use std::ops::Deref;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::thread;
use std::time::Duration;
#[cfg(feature = "ws-timed-fairness")]
use std::time::Instant;
// use time;
use std::vec::Vec;

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

#[derive(Clone, Debug)]
pub struct ThreadPool<P>
where
    P: Parker + Clone + 'static,
{
    core: Arc<Mutex<ThreadPoolCore<P>>>,
    global_sender: Arc<deque::Injector<Job>>,
    threads: usize,
    shutdown: Arc<AtomicBool>,
    parker: P,
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
    /// This function will panic if `threads` is 0.
    /// It will also panic if `threads` is larger than the `ThreadData::MAX_THREADS` value of the provided `parker`.
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
    /// ```
    pub fn new(threads: usize, parker: P) -> ThreadPool<P> {
        assert!(threads > 0);
        if let Some(max) = parker.max_threads() {
            assert!(threads <= max);
        }
        let injector = Arc::new(deque::Injector::new());
        let shutdown = Arc::new(AtomicBool::new(false));
        let core = ThreadPoolCore::new(injector.clone(), shutdown.clone(), threads, parker.clone());
        let pool = ThreadPool {
            core: Arc::new(Mutex::new(core)),
            global_sender: injector,
            threads,
            shutdown,
            parker,
        };
        #[cfg(not(feature = "thread-pinning"))]
        {
            let mut guard = pool.core.lock().unwrap();
            for _ in 0..threads {
                guard.spawn_worker(pool.core.clone(), None);
            }
        }
        #[cfg(feature = "thread-pinning")]
        {
            let mut guard = pool.core.lock().unwrap();
            let cores = core_affinity::get_core_ids().expect("core ids");
            let num_pinned = cores.len().min(threads);
            for i in 0..num_pinned {
                guard.spawn_worker_pinned(pool.core.clone(), None, cores[i]);
            }
            if num_pinned < threads {
                let num_unpinned = threads - num_pinned;
                for _ in 0..num_unpinned {
                    guard.spawn_worker(pool.core.clone(), None);
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
    /// This function will panic if `cores.len() + floating` is 0.
    #[cfg(feature = "thread-pinning")]
    pub fn with_affinity(
        cores: &[core_affinity::CoreId],
        floating: usize,
        parker: P,
    ) -> ThreadPool<P> {
        let total_threads = cores.len() + floating;
        assert!(total_threads > 0);
        if let Some(max) = parker.max_threads() {
            assert!(total_threads <= max);
        }
        let injector = Arc::new(deque::Injector::new());
        let shutdown = Arc::new(AtomicBool::new(false));
        let core = ThreadPoolCore::new(
            injector.clone(),
            shutdown.clone(),
            total_threads,
            parker.clone(),
        );
        let pool = ThreadPool {
            core: Arc::new(Mutex::new(core)),
            global_sender: injector.clone(),
            threads: total_threads,
            shutdown,
            parker,
        };
        {
            let mut guard = pool.core.lock().unwrap();
            cores.iter().for_each(|core_id| {
                guard.spawn_worker_pinned(pool.core.clone(), None, *core_id);
            });
            for _ in 0..floating {
                guard.spawn_worker(pool.core.clone(), None);
            }
        }
        pool
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

impl<P> Executor for ThreadPool<P>
where
    P: Parker + Clone + 'static,
{
    fn execute<F>(&self, job: F)
    where
        F: FnOnce() + Send + 'static,
    {
        if !self.shutdown.load(Ordering::Relaxed) {
            LOCAL_JOB_QUEUE.with(|qo| unsafe {
                let msg = Job(Box::new(job));
                match *qo.get() {
                    Some(ref q) => {
                        q.push(msg);
                    }
                    None => {
                        debug!("Scheduling on global pool.");
                        self.global_sender.push(msg);
                        #[cfg(not(feature = "ws-no-park"))]
                        self.parker.unpark_one();
                    }
                }
            })
        } else {
            warn!("Ignoring job as pool is shutting down.")
        }
    }

    fn shutdown_async(&self) {
        let shutdown_pool = self.clone();
        // Need to make sure that pool is shut down before drop is called.
        thread::spawn(move || {
            shutdown_pool.shutdown().expect("Pool didn't shut down");
        });
    }

    fn shutdown_borrowed(&self) -> Result<(), String> {
        if !self
            .shutdown
            .compare_and_swap(false, true, Ordering::SeqCst)
        {
            let latch = Arc::new(CountdownEvent::new(self.threads));
            debug!("Shutting down {} threads", self.threads);
            {
                let guard = self.core.lock().unwrap();
                for worker in guard.workers.values() {
                    worker
                        .control
                        .send(ControlMsg::Stop(latch.clone()))
                        .unwrap_or_else(|e| error!("Error submitting Stop msg: {:?}", e));
                }
            }
            #[cfg(not(feature = "ws-no-park"))]
            self.parker.unpark_all(); // gotta wake up all threads so they can get shut down
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
}

//fn spawn_worker(core: Arc<Mutex<ThreadPoolCore>>) {
//    let id = {
//        let mut guard = core.lock().unwrap();
//        guard.new_worker_id()
//    };
//    let core2 = core.clone();
//    thread::spawn(move || {
//        let mut worker = ThreadPoolWorker::new(id, core2);
//        worker.run()
//    });
//}

#[derive(Debug)]
struct WorkerEntry {
    control: channel::Sender<ControlMsg>,
    stealer: Option<JobStealer>,
}

#[derive(Debug)]
struct ThreadPoolCore<P>
where
    P: Parker + Clone + 'static,
{
    global_injector: Arc<deque::Injector<Job>>,
    shutdown: Arc<AtomicBool>,
    _threads: usize,
    ids: usize,
    workers: BTreeMap<usize, WorkerEntry>,
    parker: P,
}

impl<P> ThreadPoolCore<P>
where
    P: Parker + Clone + 'static,
{
    fn new(
        global_injector: Arc<deque::Injector<Job>>,
        shutdown: Arc<AtomicBool>,
        threads: usize,
        parker: P,
    ) -> ThreadPoolCore<P> {
        ThreadPoolCore {
            global_injector,
            shutdown,
            _threads: threads,
            ids: 0,
            workers: BTreeMap::new(),
            parker,
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
            let l: LinkedList<JobStealer> = stealers
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

    fn spawn_worker(&mut self, core: Arc<Mutex<ThreadPoolCore<P>>>, old_id: Option<usize>) {
        let id = old_id.unwrap_or_else(|| self.new_worker_id());
        let (tx_control, rx_control) = channel::unbounded();
        let worker = WorkerEntry {
            control: tx_control,
            stealer: Option::None,
        };
        let recv = self.global_injector.clone();
        self.workers.insert(id, worker);
        let p = self.parker.clone();
        thread::Builder::new()
            .name(format!("cb-ws-pool-worker-{}", id))
            .spawn(move || {
                let mut worker = ThreadPoolWorker::new(id, recv, rx_control, core, p);
                worker.run()
            })
            .expect("Could not create worker thread!");
    }

    #[cfg(feature = "thread-pinning")]
    fn spawn_worker_pinned(
        &mut self,
        core: Arc<Mutex<ThreadPoolCore<P>>>,
        old_id: Option<usize>,
        core_id: core_affinity::CoreId,
    ) {
        let id = old_id.unwrap_or_else(|| self.new_worker_id());
        let (tx_control, rx_control) = channel::unbounded();
        let worker = WorkerEntry {
            control: tx_control,
            stealer: Option::None,
        };
        let recv = self.global_injector.clone();
        self.workers.insert(id, worker);
        let p = self.parker.clone();
        thread::Builder::new()
            .name(format!("cb-ws-pool-worker-{}", id))
            .spawn(move || {
                core_affinity::set_for_current(core_id);
                let mut worker = ThreadPoolWorker::new(id, recv, rx_control, core, p);
                worker.run()
            })
            .expect("Could not create worker thread!");
    }
}

impl<P> Drop for ThreadPoolCore<P>
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
struct ThreadPoolWorker<P>
where
    P: Parker + Clone + 'static,
{
    id: usize,
    core: Weak<Mutex<ThreadPoolCore<P>>>,
    global_injector: Arc<deque::Injector<Job>>,
    control: channel::Receiver<ControlMsg>,
    stealers: Vec<JobStealer>,
    random: ThreadRng,
    parker: P,
}

impl<P> ThreadPoolWorker<P>
where
    P: Parker + Clone + 'static,
{
    fn new(
        id: usize,
        global_injector: Arc<deque::Injector<Job>>,
        control: channel::Receiver<ControlMsg>,
        core: Arc<Mutex<ThreadPoolCore<P>>>,
        parker: P,
    ) -> ThreadPoolWorker<P> {
        ThreadPoolWorker {
            id,
            core: Arc::downgrade(&core),
            global_injector,
            control,
            stealers: Vec::new(),
            random: rand::thread_rng(),
            parker,
        }
    }

    #[inline(always)]
    fn id(&self) -> &usize {
        &self.id
    }

    #[inline(always)]
    fn abort_sleep(&self, backoff: &Backoff, snoozing: &mut bool, parking: &mut bool) -> () {
        if *parking {
            #[cfg(feature = "ws-no-park")]
            unreachable!("parking should never be true in ws-no-park!");
            #[cfg(not(feature = "ws-no-park"))]
            {
                self.parker.abort_park(*self.id());
                *parking = false;
            }
        }
        if *snoozing {
            backoff.reset();
            *snoozing = false;
        }
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
        let local_stealer = JobStealer::new(local_stealer_raw, self.id);
        self.register_stealer(local_stealer.clone());
        self.parker.init(*self.id());
        let sentinel = Sentinel::new(self.core.clone(), self.id);
        let backoff = Backoff::new();
        let mut snoozing = false;
        let mut parking = false;
        'main: loop {
            let mut fairness_check = false;
            #[cfg(feature = "ws-timed-fairness")]
            let next_global_check = Instant::now() + Duration::new(0, CHECK_GLOBAL_INTERVAL_NS);
            // Try the local queue usually
            LOCAL_JOB_QUEUE.with(|q| unsafe {
                if let Some(ref local_queue) = *q.get() {
                    #[cfg(not(feature = "ws-timed-fairness"))]
                    let mut count = 0u64;
                    'local: loop {
                        match local_queue.pop() {
                            Some(d) => {
                                let Job(f) = d;
                                f.call_box();
                            }
                            None => break 'local,
                        }
                        backoff.reset();
                        #[cfg(not(feature = "ws-timed-fairness"))]
                        {
                            count += 1;
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
                } else {
                    panic!("Queue should have been initialised!");
                }
            });
            // drain the control queue
            'ctrl: loop {
                match self.control.try_recv() {
                    Ok(msg) => match msg {
                        ControlMsg::Stealers(l) => {
                            self.update_stealers(l);
                            self.abort_sleep(&backoff, &mut snoozing, &mut parking);
                        }
                        ControlMsg::Stop(latch) => {
                            latch.decrement().expect("stop latch decrements");
                            break 'main;
                        }
                    },
                    Err(channel::TryRecvError::Empty) => break 'ctrl,
                    Err(channel::TryRecvError::Disconnected) => {
                        warn!("Worker {} self-terminating.", self.id());
                        sentinel.cancel();
                        panic!("Threadpool wasn't shut down properly!");
                    }
                }
            }
            // sometimes try the global queue
            let glob_res = LOCAL_JOB_QUEUE.with(|q| unsafe {
                if let Some(ref local_queue) = *q.get() {
                    self.global_injector.steal_batch_and_pop(local_queue)
                } else {
                    panic!("Queue should have been initialised!");
                }
            });
            if let deque::Steal::Success(msg) = glob_res {
                let Job(f) = msg;
                f.call_box();
                self.abort_sleep(&backoff, &mut snoozing, &mut parking);
                continue 'main;
            }
            // only go on if there was no work left on the local queue
            if fairness_check {
                #[cfg(not(feature = "ws-no-park"))]
                self.parker.unpark_one(); // wake up more threads if there is more work to do
                continue 'main;
            }
            // try to steal something!
            for stealer in self.stealers.iter() {
                if let deque::Steal::Success(msg) = stealer.steal() {
                    let Job(f) = msg;
                    f.call_box();
                    self.abort_sleep(&backoff, &mut snoozing, &mut parking);
                    continue 'main; // only steal once before checking locally again
                }
            }
            snoozing = true;
            if parking {
                #[cfg(feature = "ws-no-park")]
                unreachable!("parking should never be true in ws-no-park!");
                #[cfg(not(feature = "ws-no-park"))]
                match self.parker.park(*self.id()) {
                    ParkResult::Retry => (), // just start over
                    ParkResult::Abort | ParkResult::Woken => {
                        self.stop_sleep(&backoff, &mut snoozing, &mut parking);
                    }
                }
            } else if backoff.is_completed() && cfg!(not(feature = "ws-no-park")) {
                self.parker.prepare_park(*self.id());
                parking = true;
            } else {
                backoff.snooze();
            }
            // aaaaand starting over with 'local
        }
        sentinel.cancel();
        self.unregister_stealer(local_stealer);
        LOCAL_JOB_QUEUE.with(|q| unsafe {
            *q.get() = None;
        });
        debug!("Worker {} shutting down", self.id());
    }

    fn register_stealer(&mut self, stealer: JobStealer) {
        match self.core.upgrade() {
            Some(core) => {
                let mut guard = core.lock().unwrap();
                guard.add_stealer(stealer);
            }
            None => panic!("Core was already shut down!"),
        }
    }

    fn unregister_stealer(&mut self, _stealer: JobStealer) {
        match self.core.upgrade() {
            Some(core) => {
                let mut guard = core.lock().unwrap();
                guard.drop_worker(self.id);
            }
            None => panic!("Core was already shut down!"),
        }
    }

    fn update_stealers(&mut self, l: LinkedList<JobStealer>) {
        let mut v = Vec::from_iter(l.into_iter());
        v.shuffle(&mut self.random);
        self.stealers = v;
    }
}

trait FnBox {
    fn call_box(self: Box<Self>);
}

impl<F: FnOnce()> FnBox for F {
    fn call_box(self: Box<F>) {
        (*self)()
    }
}

#[derive(Clone)]
struct JobStealer {
    inner: deque::Stealer<Job>,
    id: usize,
}

impl JobStealer {
    fn new(stealer: deque::Stealer<Job>, id: usize) -> JobStealer {
        JobStealer { inner: stealer, id }
    }
    fn id(&self) -> usize {
        self.id
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

struct Job(Box<dyn FnBox + Send + 'static>);

enum ControlMsg {
    Stealers(LinkedList<JobStealer>),
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
    core: Weak<Mutex<ThreadPoolCore<P>>>,
    id: usize,
    active: bool,
}

impl<P> Sentinel<P>
where
    P: Parker + Clone + 'static,
{
    fn new(core: Weak<Mutex<ThreadPoolCore<P>>>, id: usize) -> Sentinel<P> {
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
                    let mut guard = core.lock().unwrap();
                    let gsend = guard.global_injector.clone();
                    // cleanup
                    guard.drop_worker(self.id);
                    // restart
                    // make sure the new thread starts with the same worker id, so the parker doesn't run out of slots
                    guard.spawn_worker(core.clone(), Some(self.id));
                    drop(guard);
                    for job in jobs.into_iter() {
                        gsend.push(job);
                    }
                }
                None => warn!("Could not restart worker, as pool has been deallocated!"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use env_logger;

    use super::*;

    const LABEL: &'static str = "Workstealing Pool";

    #[test]
    fn test_debug() {
        let exec = ThreadPool::new(2, parker::small());
        crate::tests::test_debug(&exec, LABEL);
        exec.shutdown().expect("Pool didn't shut down!");
    }

    #[test]
    fn test_sleepy() {
        let exec = ThreadPool::new(4, parker::small());
        crate::tests::test_sleepy(&exec, LABEL);
        exec.shutdown().expect("Pool didn't shut down!");
    }

    #[test]
    fn test_custom_small() {
        let exec = ThreadPool::new(4, parker::small());
        crate::tests::test_custom(exec, LABEL);
    }

    #[test]
    fn test_custom_large() {
        let exec = ThreadPool::new(36, parker::large());
        crate::tests::test_custom(exec, LABEL);
    }

    #[test]
    fn test_custom_dyn() {
        let exec = ThreadPool::new(72, parker::dynamic());
        crate::tests::test_custom(exec, LABEL);
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
        pool.execute(move || ignore(latch2.decrement()));
        pool.execute(move || ignore(latch3.decrement()));
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
            ignore(latch2.decrement());
            pool2.execute(move || ignore(latch2.decrement()));
        });
        pool.execute(move || {
            ignore(latch3.decrement());
            pool3.execute(move || ignore(latch3.decrement()));
        });
        let res = latch.wait_timeout(Duration::from_secs(5));
        assert_eq!(res, 0);
        pool.shutdown()
            .unwrap_or_else(|e| error!("Error during pool shutdown {:?}", e));
    }

    #[test]
    fn keep_pool_size() {
        let _ = env_logger::try_init();

        let latch = Arc::new(CountdownEvent::new(2));
        let pool = ThreadPool::new(1, parker::small());
        let latch2 = latch.clone();
        let latch3 = latch.clone();
        pool.execute(move || ignore(latch2.decrement()));
        pool.execute(move || panic!("test panic please ignore"));
        pool.execute(move || ignore(latch3.decrement()));
        let res = latch.wait_timeout(Duration::from_secs(5));
        assert_eq!(res, 0);
        pool.shutdown()
            .unwrap_or_else(|e| error!("Error during pool shutdown {:?}", e));
    }

    #[test]
    fn reassign_jobs_from_failed_queues() {
        let _ = env_logger::try_init();

        let latch = Arc::new(CountdownEvent::new(2));
        let pool = ThreadPool::new(1, parker::small());
        let pool2 = pool.clone();
        let latch2 = latch.clone();
        let latch3 = latch.clone();
        pool.execute(move || ignore(latch2.decrement()));
        pool.execute(move || {
            pool2.execute(move || ignore(latch3.decrement()));
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
        pool.execute(move || ignore(latch2.decrement()));
        pool.execute(move || {
            pool2.shutdown_async();
            ignore(stop_latch2.decrement());
        });
        let res = stop_latch.wait_timeout(Duration::from_secs(1));
        assert_eq!(res, 0);
        thread::sleep(Duration::from_secs(1)); // give the new shutdown thread some time to run
        pool.execute(move || ignore(latch3.decrement()));
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
        pool.execute(move || ignore(run_latch2.decrement()));
        let res = run_latch.wait_timeout(Duration::from_secs(1));
        assert_eq!(res, 0);
        pool.shutdown().expect("pool to shut down");
        pool2.execute(move || ignore(stop_latch2.decrement()));
        let res = stop_latch.wait_timeout(Duration::from_secs(1));
        assert_eq!(res, 1);
    }

    /// This test may/should panic on the worker threads, for example with "Threadpool wasn't shut down properly!".
    /// Otherwise they aren't shut down properly.
    #[test]
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
        let core = Arc::downgrade(&pool.core);
        // sleep before drop, to ensure that even parked threads shut down
        std::thread::sleep(std::time::Duration::from_secs(1));
        drop(pool);
        std::thread::sleep(std::time::Duration::from_secs(1));
        assert!(core.upgrade().is_none());
    }
}
