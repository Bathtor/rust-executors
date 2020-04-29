// Copyright 2017-2020 Lars Kroll. See the LICENSE
// file at the top-level directory of this distribution.
//
// Licensed under the MIT license
// <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed
// except according to those terms.

//! A thread pool `Executor` used to execute functions in parallel.
//!
//! This implementation is simply a wrapper for
//! [threadpool](https://crates.io/crates/threadpool)
//! to allow it to be used where the `Executor` trait is expected.
//!
//! # Examples
//!
//! ## Synchronized with a channel
//!
//! Every thread sends one message over the channel, which then is collected with the `take()`.
//!
//! ```
//! use executors::*;
//! use executors::threadpool_executor::ThreadPoolExecutor;
//! use std::sync::mpsc::channel;
//!
//! let n_workers = 4;
//! let n_jobs = 8;
//! let pool = ThreadPoolExecutor::new(n_workers);
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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use threadpool::ThreadPool;

#[derive(Clone, Debug)]
pub struct ThreadPoolExecutor {
    pool: ThreadPool,
    active: Arc<AtomicBool>,
}

impl ThreadPoolExecutor {
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
    /// use executors::*;
    /// use executors::threadpool_executor::ThreadPoolExecutor;
    ///
    /// let pool = ThreadPoolExecutor::new(4);
    /// ```
    pub fn new(threads: usize) -> ThreadPoolExecutor {
        let pool = ThreadPool::new(threads);
        ThreadPoolExecutor {
            pool,
            active: Arc::new(AtomicBool::new(true)),
        }
    }
}

/// Create a thread pool with one thread per CPU.
/// On machines with hyperthreading,
/// this will create one thread per hyperthread.
#[cfg(feature = "defaults")]
impl Default for ThreadPoolExecutor {
    fn default() -> Self {
        ThreadPoolExecutor::new(num_cpus::get())
    }
}

impl Executor for ThreadPoolExecutor {
    fn execute<F>(&self, job: F)
    where
        F: FnOnce() + Send + 'static,
    {
        if self.active.load(Ordering::SeqCst) {
            self.pool.execute(job);
        } else {
            warn!("Ignoring job as pool is shutting down.");
        }
    }

    fn shutdown_async(&self) {
        if self.active.compare_and_swap(true, false, Ordering::SeqCst) {
            debug!("Shutting down executor.");
        } else {
            warn!("Executor was already shut down!");
        }
    }

    fn shutdown_borrowed(&self) -> Result<(), String> {
        if self.active.compare_and_swap(true, false, Ordering::SeqCst) {
            debug!("Waiting for pool to shut down.");
            self.pool.join();
            debug!("Pool was shut down.");
            Result::Ok(())
        } else {
            Result::Err(String::from("Pool was already shut down!"))
        }
    }
}

#[cfg(test)]
mod tests {
    use env_logger;

    use super::*;
    use std::time::Duration;

    const LABEL: &'static str = "Threadpool";

    #[test]
    fn test_debug() {
        let exec = ThreadPoolExecutor::new(2);
        crate::tests::test_debug(&exec, LABEL);
        exec.shutdown().expect("Pool didn't shut down!");
    }

    #[test]
    fn test_sleepy() {
        let exec = ThreadPoolExecutor::new(4);
        crate::tests::test_sleepy(&exec, LABEL);
        exec.shutdown().expect("Pool didn't shut down!");
    }

    #[test]
    fn test_defaults() {
        crate::tests::test_defaults::<ThreadPoolExecutor>(LABEL);
    }

    #[test]
    fn run_with_two_threads() {
        let _ = env_logger::try_init();

        let latch = Arc::new(CountdownEvent::new(2));
        let pool = ThreadPoolExecutor::new(2);
        let latch2 = latch.clone();
        let latch3 = latch.clone();
        pool.execute(move || ignore(latch2.decrement()));
        pool.execute(move || ignore(latch3.decrement()));
        let res = latch.wait_timeout(Duration::from_secs(5));
        assert_eq!(res, 0);
    }

    #[test]
    fn keep_pool_size() {
        let _ = env_logger::try_init();

        let latch = Arc::new(CountdownEvent::new(2));
        let pool = ThreadPoolExecutor::new(1);
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
        let _ = env_logger::try_init();

        let pool = ThreadPoolExecutor::new(1);
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
        let _ = env_logger::try_init();

        let pool = ThreadPoolExecutor::new(1);
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
}
