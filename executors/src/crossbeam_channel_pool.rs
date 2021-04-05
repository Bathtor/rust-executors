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
//! Uses [crossbeam-channel](https://crates.io/crates/crossbeam-channel)
//! internally for work distribution.
//! # Examples
//!
//! ## Synchronized with a channel
//!
//! Every thread sends one message over the channel, which then is collected with the `take()`.
//!
//! ```
//! use executors::*;
//! use executors::crossbeam_channel_pool::ThreadPool;
//! use std::sync::mpsc::channel;
//!
//! let n_workers = 4;
//! let n_jobs = 8;
//! let pool = ThreadPool::new(n_workers);
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
//! # pool.shutdown().expect("shutdown");
//! ```

use super::*;

use crossbeam_channel as channel;
use std::{
    future::Future,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
        Mutex,
        Weak,
    },
    thread,
    time::Duration,
};

/// A handle to a [crossbeam_channel_pool](crossbeam_channel_pool)
///
/// See module level documentation for usage information.
#[derive(Clone, Debug)]
pub struct ThreadPool {
    core: Arc<Mutex<ThreadPoolCore>>,
    sender: channel::Sender<JobMsg>,
    threads: usize,
    shutdown: Arc<AtomicBool>,
}

impl ThreadPool {
    /// Creates a new thread pool capable of executing `threads` number of jobs concurrently.
    ///
    /// # Panics
    ///
    /// This function will panic if `threads` is 0.
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
    /// use executors::crossbeam_channel_pool::ThreadPool;
    ///
    /// let pool = ThreadPool::new(4);
    /// # pool.shutdown();
    /// ```
    pub fn new(threads: usize) -> ThreadPool {
        assert!(threads > 0);
        let (tx, rx) = channel::unbounded();
        let shutdown = Arc::new(AtomicBool::new(false));
        let pool = ThreadPool {
            core: Arc::new(Mutex::new(ThreadPoolCore {
                sender: tx.clone(),
                receiver: rx,
                shutdown: shutdown.clone(),
                threads,
                ids: 0,
            })),
            sender: tx,
            threads,
            shutdown,
        };
        #[cfg(not(feature = "thread-pinning"))]
        {
            for _ in 0..threads {
                spawn_worker(pool.core.clone());
            }
        }
        #[cfg(feature = "thread-pinning")]
        {
            let cores = core_affinity::get_core_ids().expect("core ids");
            let num_pinned = cores.len().min(threads);
            for core in cores.into_iter().take(num_pinned) {
                spawn_worker_pinned(pool.core.clone(), core);
            }
            if num_pinned < threads {
                let num_unpinned = threads - num_pinned;
                for _ in 0..num_unpinned {
                    spawn_worker(pool.core.clone());
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
    pub fn with_affinity(cores: &[core_affinity::CoreId], floating: usize) -> ThreadPool {
        let total_threads = cores.len() + floating;
        assert!(total_threads > 0);
        let (tx, rx) = channel::unbounded();
        let shutdown = Arc::new(AtomicBool::new(false));
        let pool = ThreadPool {
            core: Arc::new(Mutex::new(ThreadPoolCore {
                sender: tx.clone(),
                receiver: rx,
                shutdown: shutdown.clone(),
                threads: total_threads,
                ids: 0,
            })),
            sender: tx,
            threads: total_threads,
            shutdown,
        };
        cores.iter().for_each(|core_id| {
            spawn_worker_pinned(pool.core.clone(), *core_id);
        });
        for _ in 0..floating {
            spawn_worker(pool.core.clone());
        }
        pool
    }

    fn schedule_task(&self, task: async_task::Runnable) -> () {
        // schedule tasks always, even if the pool is already stopped, since it's unsafe to drop from the schedule function
        // this might lead to some "memory leaks" if an executor remains stopped but allocated for a long time

        self.sender
            .send(JobMsg::Task(task))
            .unwrap_or_else(|e| error!("Error submitting job: {:?}", e));
    }
}

/// Create a thread pool with one thread per CPU.
/// On machines with hyperthreading,
/// this will create one thread per hyperthread.
#[cfg(feature = "defaults")]
impl Default for ThreadPool {
    fn default() -> Self {
        ThreadPool::new(num_cpus::get())
    }
}

impl CanExecute for ThreadPool {
    fn execute_job(&self, job: Box<dyn FnOnce() + Send + 'static>) {
        // NOTE: This check costs about 150k schedulings/s in a 2 by 2 experiment over 20 runs.
        if !self.shutdown.load(Ordering::SeqCst) {
            self.sender
                .send(JobMsg::Job(job))
                .unwrap_or_else(|e| error!("Error submitting job: {:?}", e));

            #[cfg(feature = "produce-metrics")]
            increment_gauge!("executors.jobs_queued", 1.0, "executor" => "crossbeam_channel_pool");
        } else {
            warn!("Ignoring job as pool is shutting down.");
        }
    }
}

impl Executor for ThreadPool {
    fn shutdown_async(&self) {
        if self
            .shutdown
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            let latch = Arc::new(CountdownEvent::new(self.threads));
            debug!("Shutting down {} threads", self.threads);
            for _ in 0..self.threads {
                self.sender
                    .send(JobMsg::Stop(latch.clone()))
                    .unwrap_or_else(|e| error!("Error submitting Stop msg: {:?}", e));
            }
        } else {
            warn!("Pool is already shutting down!");
        }
    }

    fn shutdown_borrowed(&self) -> Result<(), String> {
        if self
            .shutdown
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            let latch = Arc::new(CountdownEvent::new(self.threads));
            debug!("Shutting down {} threads", self.threads);
            for _ in 0..self.threads {
                self.sender
                    .send(JobMsg::Stop(latch.clone()))
                    .unwrap_or_else(|e| error!("Error submitting Stop msg: {:?}", e));
            }
            let timeout = Duration::from_millis(5000);
            let remaining = latch.wait_timeout(timeout);
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
        register_counter!("executors.jobs_executed", "The total number of jobs that were executed", "executor" => "crossbeam_channel_pool");
        register_gauge!("executors.jobs_queued", "The number of jobs that are currently waiting to be executed", "executor" => "crossbeam_channel_pool");
    }
}

impl FuturesExecutor for ThreadPool {
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

fn spawn_worker(core: Arc<Mutex<ThreadPoolCore>>) {
    let id = {
        let mut guard = core.lock().unwrap();
        guard.new_worker_id()
    };
    let mut worker = ThreadPoolWorker::new(id, core);
    thread::Builder::new()
        .name("cb-channel-pool-worker".to_string())
        .spawn(move || worker.run())
        .expect("Could not create worker thread!");
}

#[cfg(feature = "thread-pinning")]
fn spawn_worker_pinned(core: Arc<Mutex<ThreadPoolCore>>, core_id: core_affinity::CoreId) {
    let id = {
        let mut guard = core.lock().unwrap();
        guard.new_worker_id()
    };
    let mut worker = ThreadPoolWorker::new(id, core);
    thread::Builder::new()
        .name("cb-channel-pool-worker".to_string())
        .spawn(move || {
            core_affinity::set_for_current(core_id);
            worker.run()
        })
        .expect("Could not create worker thread!");
}

#[derive(Debug)]
struct ThreadPoolCore {
    sender: channel::Sender<JobMsg>,
    receiver: channel::Receiver<JobMsg>,
    shutdown: Arc<AtomicBool>,
    threads: usize,
    ids: usize,
}

impl ThreadPoolCore {
    fn new_worker_id(&mut self) -> usize {
        let id = self.ids;
        self.ids += 1;
        id
    }
}

impl Drop for ThreadPoolCore {
    fn drop(&mut self) {
        if self
            .shutdown
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            let latch = Arc::new(CountdownEvent::new(self.threads));
            debug!("Shutting down {} threads", self.threads);
            for _ in 0..self.threads {
                self.sender
                    .send(JobMsg::Stop(latch.clone()))
                    .unwrap_or_else(|e| error!("Error submitting Stop msg: {:?}", e));
            }
            let timeout = Duration::from_millis(5000);
            let remaining = latch.wait_timeout(timeout);
            if remaining == 0 {
                debug!("All threads shut down");
            } else {
                warn!(
                    "Pool failed to shut down in time. {:?} threads remaining.",
                    remaining
                );
            }
        } else {
            warn!("Pool is already shutting down!");
        }
    }
}

struct ThreadLocalExecute(channel::Sender<JobMsg>);
impl CanExecute for ThreadLocalExecute {
    fn execute_job(&self, job: Box<dyn FnOnce() + Send + 'static>) {
        self.0
            .send(JobMsg::Job(job))
            .unwrap_or_else(|e| error!("Error submitting Stop msg: {:?}", e));

        #[cfg(feature = "produce-metrics")]
        increment_gauge!("executors.jobs_queued", 1.0, "executor" => "crossbeam_channel_pool");
    }
}

struct ThreadPoolWorker {
    id: usize,
    core: Weak<Mutex<ThreadPoolCore>>,
    recv: channel::Receiver<JobMsg>,
}

impl ThreadPoolWorker {
    fn new(id: usize, core: Arc<Mutex<ThreadPoolCore>>) -> ThreadPoolWorker {
        let recv = {
            let guard = core.lock().unwrap();
            guard.receiver.clone()
        };
        ThreadPoolWorker {
            id,
            core: Arc::downgrade(&core),
            recv,
        }
    }

    fn run(&mut self) {
        debug!("CrossbeamWorker {} starting", self.id);
        let sender = {
            let core = self.core.upgrade().expect("Core already shut down!");
            let guard = core.lock().expect("Mutex poisoned!");
            guard.sender.clone()
        };
        let sentinel = Sentinel::new(self.core.clone(), self.id);
        set_local_executor(ThreadLocalExecute(sender));
        while let Ok(msg) = self.recv.recv() {
            match msg {
                JobMsg::Job(f) => {
                    f();
                    #[cfg(feature = "produce-metrics")]
                    {
                        increment_counter!("executors.jobs_executed", "executor" => "crossbeam_channel_pool", "thread_id" => format!("{}", self.id));
                        decrement_gauge!("executors.jobs_queued", 1.0, "executor" => "crossbeam_channel_pool");
                    }
                }
                JobMsg::Task(t) => {
                    let _ = t.run();

                    #[cfg(feature = "produce-metrics")]
                    {
                        increment_counter!("executors.jobs_executed", "executor" => "crossbeam_channel_pool", "thread_id" => format!("{}", self.id));
                        decrement_gauge!("executors.jobs_queued", 1.0, "executor" => "crossbeam_channel_pool");
                    }
                }
                JobMsg::Stop(latch) => {
                    let _ = latch.decrement();
                    break;
                }
            }
        }
        unset_local_executor();
        sentinel.cancel();
        debug!("CrossbeamWorker {} shutting down", self.id);
    }
}

enum JobMsg {
    Job(Box<dyn FnOnce() + Send + 'static>),
    Task(async_task::Runnable),
    Stop(Arc<CountdownEvent>),
}

struct Sentinel {
    core: Weak<Mutex<ThreadPoolCore>>,
    id: usize,
    active: bool,
}

impl Sentinel {
    fn new(core: Weak<Mutex<ThreadPoolCore>>, id: usize) -> Sentinel {
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

impl Drop for Sentinel {
    fn drop(&mut self) {
        if self.active {
            warn!("Active worker {} died! Restarting...", self.id);
            match self.core.upgrade() {
                Some(core) => spawn_worker(core),
                None => warn!("Could not restart worker, as pool has been deallocated!"),
            }
        }
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    const LABEL: &str = "Crossbeam Channel Pool";

    #[test]
    fn test_debug() {
        let exec = ThreadPool::new(2);
        crate::tests::test_debug(&exec, LABEL);
        exec.shutdown().expect("Pool didn't shut down!");
    }

    #[test]
    fn test_sleepy() {
        let exec = ThreadPool::new(4);
        crate::tests::test_sleepy(exec, LABEL);
    }

    #[test]
    fn test_defaults() {
        crate::tests::test_defaults::<ThreadPool>(LABEL);
    }

    #[test]
    fn test_local() {
        let exec = ThreadPool::default();
        crate::tests::test_local(exec, LABEL);
    }

    #[test]
    fn test_custom_large() {
        let exec = ThreadPool::new(36);
        crate::tests::test_custom(exec, LABEL);
    }

    #[cfg(feature = "thread-pinning")]
    #[test]
    fn test_custom_affinity() {
        let cores = core_affinity::get_core_ids().expect("core ids");
        // travis doesn't have enough cores for this
        //let exec = ThreadPool::with_affinity(&cores[0..4], 4);
        let exec = ThreadPool::with_affinity(&cores[0..1], 1);
        crate::tests::test_custom(exec, LABEL);
    }

    #[test]
    fn run_with_two_threads() {
        let _ = env_logger::try_init();

        let latch = Arc::new(CountdownEvent::new(2));
        let pool = ThreadPool::new(2);
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
        pool.shutdown().expect("shutdown");
    }

    // replace ignore with panic cfg gate when https://github.com/rust-lang/rust/pull/74754 is merged
    #[test]
    #[ignore]
    fn keep_pool_size() {
        let _ = env_logger::try_init();

        let latch = Arc::new(CountdownEvent::new(2));
        let pool = ThreadPool::new(1);
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
        pool.shutdown().expect("shutdown");
    }

    #[test]
    fn shutdown_from_worker() {
        let _ = env_logger::try_init();

        let pool = ThreadPool::new(1);
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
        pool.execute(move || {
            let _ = latch3.decrement();
        });
        let res = latch.wait_timeout(Duration::from_secs(1));
        assert_eq!(res, 1);
    }

    #[test]
    fn shutdown_external() {
        let _ = env_logger::try_init();

        let pool = ThreadPool::new(1);
        let pool2 = pool.clone();
        let latch = Arc::new(CountdownEvent::new(2));
        let latch2 = latch.clone();
        let latch3 = latch.clone();
        pool.execute(move || {
            let _ = latch2.decrement();
        });
        pool.shutdown().expect("pool to shut down");
        pool2.execute(move || {
            let _ = latch3.decrement();
        });
        let res = latch.wait_timeout(Duration::from_secs(1));
        assert_eq!(res, 1);
    }

    // replace ignore with panic cfg gate when https://github.com/rust-lang/rust/pull/74754 is merged
    #[test]
    #[ignore]
    fn shutdown_on_handle_drop() {
        let _ = env_logger::try_init();

        let pool = ThreadPool::new(1);
        let core = Arc::downgrade(&pool.core);
        drop(pool);
        std::thread::sleep(std::time::Duration::from_secs(1));
        assert!(core.upgrade().is_none());
    }
}
