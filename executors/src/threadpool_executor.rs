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
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use threadpool::ThreadPool;

/// A handle to a [threadpool_executor](threadpool_executor)
///
/// See module level documentation for usage information.
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

impl CanExecute for ThreadPoolExecutor {
    fn execute_job(&self, job: Box<dyn FnOnce() + Send + 'static>) {
        if self.active.load(Ordering::SeqCst) {
            #[cfg(feature = "produce-metrics")]
            let job = {
                increment_gauge!("executors.jobs_queued", 1.0, "executor" => std::any::type_name::<ThreadPoolExecutor>());
                Box::new(move || {
                    job();
                    increment_counter!("executors.jobs_executed", "executor" => std::any::type_name::<ThreadPoolExecutor>());
                    decrement_gauge!("executors.jobs_queued", 1.0, "executor" => std::any::type_name::<ThreadPoolExecutor>());
                })
            };

            self.pool.execute(job);
        } else {
            warn!("Ignoring job as pool is shutting down.");
        }
    }
}

impl Executor for ThreadPoolExecutor {
    // override this here instead of using the default implementation to avoid double boxing
    fn execute<F>(&self, job: F)
    where
        F: FnOnce() + Send + 'static,
    {
        if self.active.load(Ordering::SeqCst) {
            #[cfg(feature = "produce-metrics")]
            let job = {
                increment_gauge!("executors.jobs_queued", 1.0, "executor" => std::any::type_name::<ThreadPoolExecutor>());
                move || {
                    job();
                    increment_counter!("executors.jobs_executed", "executor" => std::any::type_name::<ThreadPoolExecutor>());
                    decrement_gauge!("executors.jobs_queued", 1.0, "executor" => std::any::type_name::<ThreadPoolExecutor>());
                }
            };

            self.pool.execute(job);
        } else {
            warn!("Ignoring job as pool is shutting down.");
        }
    }

    fn shutdown_async(&self) {
        if self
            .active
            .compare_exchange(true, false, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            debug!("Shutting down executor.");
        } else {
            warn!("Executor was already shut down!");
        }
    }

    fn shutdown_borrowed(&self) -> Result<(), String> {
        if self
            .active
            .compare_exchange(true, false, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            debug!("Waiting for pool to shut down.");
            self.pool.join();
            debug!("Pool was shut down.");
            Result::Ok(())
        } else {
            Result::Err(String::from("Pool was already shut down!"))
        }
    }

    #[cfg(feature = "produce-metrics")]
    fn register_metrics(&self) {
        register_counter!("executors.jobs_executed", "The total number of jobs that were executed", "executor" => std::any::type_name::<ThreadPoolExecutor>());
        register_gauge!("executors.jobs_queued", "The number of jobs that are currently waiting to be executed", "executor" => std::any::type_name::<ThreadPoolExecutor>());
    }
}

#[cfg(test)]
mod tests {

    use super::*;
    use std::time::Duration;

    const LABEL: &str = "Threadpool";

    #[test]
    fn test_debug() {
        let exec = ThreadPoolExecutor::new(2);
        crate::tests::test_debug(&exec, LABEL);
        exec.shutdown().expect("Pool didn't shut down!");
    }

    #[test]
    fn test_sleepy() {
        let exec = ThreadPoolExecutor::new(4);
        crate::tests::test_sleepy(exec, LABEL);
    }

    #[test]
    fn test_defaults() {
        crate::tests::test_defaults::<ThreadPoolExecutor>(LABEL);
    }

    // replace ignore with panic cfg gate when https://github.com/rust-lang/rust/pull/74754 is merged
    #[test]
    #[ignore]
    #[should_panic] // this executor does not actually support local scheduling
    fn test_local() {
        let exec = ThreadPoolExecutor::default();
        crate::tests::test_local(exec, LABEL);
    }

    #[test]
    fn run_with_two_threads() {
        let _ = env_logger::try_init();

        let latch = Arc::new(CountdownEvent::new(2));
        let pool = ThreadPoolExecutor::new(2);
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
    }

    // replace ignore with panic cfg gate when https://github.com/rust-lang/rust/pull/74754 is merged
    #[test]
    #[ignore]
    fn keep_pool_size() {
        let _ = env_logger::try_init();

        let latch = Arc::new(CountdownEvent::new(2));
        let pool = ThreadPoolExecutor::new(1);
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

        let pool = ThreadPoolExecutor::new(1);
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
}
