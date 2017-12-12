// Copyright 2017 Lars Kroll. See the LICENSE
// file at the top-level directory of this distribution.
//
// Licensed under the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed
// except according to those terms.

//! A thread pool used to execute functions in parallel.
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
//! use crossbeam_channel_pool::ThreadPool;
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
//! ```

extern crate crossbeam_channel;
extern crate synchronoise;
extern crate utils;
#[macro_use]
extern crate log;

use crossbeam_channel::*;
use synchronoise::CountdownEvent;
use std::sync::{Arc, Weak, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use std::thread;
use utils::*;

//pub trait ThreadPoolHandle {
//    /// Executes the job on one of the w
//    fn execute<F>(&self, job: F)
//    where
//        F: FnOnce() + Send + 'static;
//    fn shutdown_async(&self);
//    fn shutdown(&self) -> Result<(), String>;
//}

#[derive(Clone)]
pub struct ThreadPool {
    core: Arc<Mutex<ThreadPoolCore>>,
    sender: Sender<JobMsg>,
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
    /// # Examples
    ///
    /// Create a new thread pool capable of executing four jobs concurrently:
    ///
    /// ```
    /// use crossbeam_channel_pool::ThreadPool;
    ///
    /// let pool = ThreadPool::new(4);
    /// ```
    pub fn new(threads: usize) -> ThreadPool {
        assert!(threads > 0);
        let (tx, rx) = unbounded();
        let shutdown = Arc::new(AtomicBool::new(false));
        let pool = ThreadPool {
            core: Arc::new(Mutex::new(ThreadPoolCore {
                sender: tx.clone(),
                receiver: rx.clone(),
                shutdown: shutdown.clone(),
                threads,
                ids: 0,
            })),
            sender: tx.clone(),
            threads,
            shutdown,
        };
        for _ in 0..threads {
            spawn_worker(pool.core.clone());
        }
        pool
    }

    /// Executes the function `job` on a thread in the pool.
    ///
    /// # Examples
    ///
    /// Execute four jobs on a thread pool that can run two jobs concurrently:
    ///
    /// ```
    /// use crossbeam_channel_pool::ThreadPool;
    ///
    /// let pool = ThreadPool::new(2);
    /// pool.execute(|| println!("hello"));
    /// pool.execute(|| println!("world"));
    /// pool.execute(|| println!("foo"));
    /// pool.execute(|| println!("bar"));
    /// std::thread::sleep(std::time::Duration::from_secs(1));
    /// ```
    pub fn execute<F>(&self, job: F)
    where
        F: FnOnce() + Send + 'static,
    {
        // NOTE: This check costs about 150k schedulings/s in a 2 by 2 experiment over 20 runs.
        if !self.shutdown.load(Ordering::SeqCst) {
            self.sender.send(JobMsg::Job(Box::new(job))).expect(
                "Couldn't schedule job in queue!",
            );
        } else {
            warn!("Ignoring job as pool is shutting down.")
        }
    }

    /// Shutdown the thread pool without waiting.
    ///
    /// This method can be used from one of the worker threads without
    /// risk of deadlocking.
    ///
    /// # Examples
    ///
    /// Shutdown a [`ThreadPool`] from within a worker.
    ///
    /// ```
    /// use crossbeam_channel_pool::ThreadPool;
    ///
    /// let pool = ThreadPool::new(1);
    /// let pool2 = pool.clone();
    /// pool.execute(|| println!("Hello!"));
    /// pool.execute(move || {
    /// 		println!("Shutting down");
    ///		pool2.shutdown_async();
    ///	});
    /// std::thread::sleep(std::time::Duration::from_secs(1)); // or wait with a barrier
    /// pool.execute(|| println!("doesn't work!"));
    /// ```
    pub fn shutdown_async(&self) {
        if !self.shutdown.compare_and_swap(
            false,
            true,
            Ordering::SeqCst,
        )
        {
            let latch = Arc::new(CountdownEvent::new(self.threads));
            debug!("Shutting down {} threads", self.threads);
            for _ in 0..self.threads {
                self.sender.send(JobMsg::Stop(latch.clone())).expect(
                    "Couldn't send stop msg to thread!",
                );
            }
        } else {
            warn!("Pool is already shutting down!");
        }
    }

    /// Shutdown the thread pool and wait for it to shut down all workers.
    ///
    /// This method can be ONLY be used from *outside* the worker threads
    /// without risk of deadlocking.
    ///
    ///
    /// # Examples
    ///
    /// Shutdown a [`ThreadPool`] from an external thread.
    ///
    /// ```
    /// use crossbeam_channel_pool::ThreadPool;
    ///
    /// let pool = ThreadPool::new(1);
    /// let pool2 = pool.clone();
    /// pool.execute(|| println!("Hello!"));
    /// pool.shutdown().expect("pool to shut down");
    /// pool2.execute(|| println!("doesn't work!"));
    /// ```

    pub fn shutdown(self) -> Result<(), String> {
        if !self.shutdown.compare_and_swap(
            false,
            true,
            Ordering::SeqCst,
        )
        {
            let latch = Arc::new(CountdownEvent::new(self.threads));
            debug!("Shutting down {} threads", self.threads);
            for _ in 0..self.threads {
                self.sender.send(JobMsg::Stop(latch.clone())).expect(
                    "Couldn't send stop msg to thread!",
                );
            }
            let timeout = Duration::from_millis(5000);
            let remaining = latch.wait_timeout(timeout);
            if remaining == 0 {
                debug!("All threads shut down");
                Result::Ok(())
            } else {
                let msg = format!(
                    "Pool failed to shut down in time. {:?} threads remaining.",
                    remaining
                );
                Result::Err(String::from(msg))
            }
        } else {
            Result::Err(String::from("Pool is already shutting down!"))
        }
    }
}

fn spawn_worker(core: Arc<Mutex<ThreadPoolCore>>) {

    let id = {
        let mut guard = core.lock().unwrap();
        guard.new_worker_id()
    };
    let mut worker = ThreadPoolWorker::new(id, core.clone());
    thread::spawn(move || worker.run());
}

struct ThreadPoolCore {
    sender: Sender<JobMsg>,
    receiver: Receiver<JobMsg>,
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
        if !self.shutdown.compare_and_swap(
            false,
            true,
            Ordering::SeqCst,
        )
        {
            let latch = Arc::new(CountdownEvent::new(self.threads));
            debug!("Shutting down {} threads", self.threads);
            for _ in 0..self.threads {
                self.sender.send(JobMsg::Stop(latch.clone())).expect(
                    "Couldn't send stop msg to thread!",
                );
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

struct ThreadPoolWorker {
    id: usize,
    core: Weak<Mutex<ThreadPoolCore>>,
    recv: Receiver<JobMsg>,
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
    fn id(&self) -> &usize {
        &self.id
    }
    fn run(&mut self) {
        debug!("CrossbeamWorker {} starting", self.id());
        let sentinel = Sentinel::new(self.core.clone(), self.id);
        while let Ok(msg) = self.recv.recv() {
            match msg {
                JobMsg::Job(f) => f.call_box(),
                JobMsg::Stop(latch) => {
                    ignore(latch.decrement());
                    break;
                }
            }
        }
        sentinel.cancel();
        debug!("CrossbeamWorker {} shutting down", self.id());
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

enum JobMsg {
    Job(Box<FnBox + Send + 'static>),
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
                Some(core) => spawn_worker(core.clone()),
                None => warn!("Could not restart worker, as pool has been deallocated!"),
            }            
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate env_logger;

    use super::*;

    #[test]
    fn run_with_two_threads() {
        env_logger::init();

        let latch = Arc::new(CountdownEvent::new(2));
        let pool = ThreadPool::new(2);
        let latch2 = latch.clone();
        let latch3 = latch.clone();
        pool.execute(move || ignore(latch2.decrement()));
        pool.execute(move || ignore(latch3.decrement()));
        let res = latch.wait_timeout(Duration::from_secs(5));
        assert_eq!(res, 0);
    }

    #[test]
    fn keep_pool_size() {
        env_logger::init();

        let latch = Arc::new(CountdownEvent::new(2));
        let pool = ThreadPool::new(1);
        let latch2 = latch.clone();
        let latch3 = latch.clone();
        pool.execute(move || ignore(latch2.decrement()));
        pool.execute(move || panic!("test panic please ignore"));
        pool.execute(move || ignore(latch3.decrement()));
        let res = latch.wait_timeout(Duration::from_secs(5));
        assert_eq!(res, 0);
    }

    #[test]
    fn shutdown_from_worker() {
        env_logger::init();

        let pool = ThreadPool::new(1);
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
        pool.execute(move || ignore(latch3.decrement()));
        let res = latch.wait_timeout(Duration::from_secs(1));
        assert_eq!(res, 1);
    }

    #[test]
    fn shutdown_external() {
        env_logger::init();

        let pool = ThreadPool::new(1);
        let pool2 = pool.clone();
        let latch = Arc::new(CountdownEvent::new(2));
        let latch2 = latch.clone();
        let latch3 = latch.clone();
        pool.execute(move || ignore(latch2.decrement()));
        pool.shutdown().expect("pool to shut down");
        pool2.execute(move || ignore(latch3.decrement()));
        let res = latch.wait_timeout(Duration::from_secs(1));
        assert_eq!(res, 1);
    }

    #[test]
    fn shutdown_on_handle_drop() {
        env_logger::init();

        let pool = ThreadPool::new(1);
        let core = Arc::downgrade(&pool.core);
        drop(pool);
        std::thread::sleep(std::time::Duration::from_secs(1));
        assert!(core.upgrade().is_none());
    }
}
