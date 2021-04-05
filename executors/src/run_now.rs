// Copyright 2017-2020 Lars Kroll. See the LICENSE
// file at the top-level directory of this distribution.
//
// Licensed under the MIT license
// <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed
// except according to those terms.

//! A simple `Executor` that simply runs tasks immediately on the current thread.
//!
//! This is mostly useful to work with APIs that require an `Executor`,
//! even if "normal" stack-based execution is actually desired.
//!
//! # Examples
//!
//! Run tasks in order.
//!
//! ```
//! use executors::*;
//! use executors::run_now::RunNowExecutor;
//!
//! let exec = RunNowExecutor::new();
//! exec.execute(|| println!("hello"));
//! exec.execute(|| println!("world"));
//! exec.execute(|| println!("foo"));
//! exec.execute(|| println!("bar"));
//! exec.shutdown();
//! ```
//!
//! # Note
//!
//! If you use [try_execute_locally](try_execute_locally) from within a job closure,
//! it will be the same as running recursively, so you may run of out stack space, eventually.
use super::*;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

/// A handle to the [run_now](run_now) executor
///
/// See module level documentation for usage information.
#[derive(Clone, Debug)]
pub struct RunNowExecutor {
    active: Arc<AtomicBool>,
}

struct ThreadLocalRunNow;
impl CanExecute for ThreadLocalRunNow {
    fn execute_job(&self, job: Box<dyn FnOnce() + Send + 'static>) {
        job();
    }
}

impl RunNowExecutor {
    /// Create a new [run_now](run_now) executor
    pub fn new() -> RunNowExecutor {
        RunNowExecutor {
            active: Arc::new(AtomicBool::new(true)),
        }
    }
}

/// Create an executor running tasks on the invoking thread.
#[cfg(feature = "defaults")]
impl Default for RunNowExecutor {
    fn default() -> Self {
        RunNowExecutor::new()
    }
}

impl CanExecute for RunNowExecutor {
    fn execute_job(&self, job: Box<dyn FnOnce() + Send + 'static>) {
        if self.active.load(Ordering::SeqCst) {
            job();
            #[cfg(feature = "produce-metrics")]
            increment_counter!("executors.jobs_executed", "executor" => std::any::type_name::<Self>());
        } else {
            warn!("Ignoring job as pool is shutting down.");
        }
    }
}

impl Executor for RunNowExecutor {
    // override default implementation to avoid boxing at all
    fn execute<F>(&self, job: F)
    where
        F: FnOnce() + Send + 'static,
    {
        if self.active.load(Ordering::SeqCst) {
            set_local_executor(ThreadLocalRunNow);
            job();
            unset_local_executor();
            #[cfg(feature = "produce-metrics")]
            increment_counter!("executors.jobs_executed", "executor" => std::any::type_name::<Self>());
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
            debug!("Shutting down executor.");
            Result::Ok(())
        } else {
            Result::Err(String::from("Pool was already shut down!"))
        }
    }

    #[cfg(feature = "produce-metrics")]
    fn register_metrics(&self) {
        register_counter!("executors.jobs_executed", "The total number of jobs that were executed", "executor" => std::any::type_name::<Self>());
    }
}

#[cfg(test)]
mod tests {

    use super::*;
    use std::time::Duration;

    const LABEL: &str = "Run Now";

    #[test]
    fn test_debug() {
        let exec = RunNowExecutor::new();
        crate::tests::test_debug(&exec, LABEL);
    }

    #[test]
    fn test_defaults() {
        crate::tests::test_small_defaults::<RunNowExecutor>(LABEL);
    }

    #[test]
    fn test_local() {
        let exec = RunNowExecutor::new();
        crate::tests::test_small_local(exec, LABEL);
    }

    #[test]
    fn run_tasks() {
        let _ = env_logger::try_init();

        let latch = Arc::new(CountdownEvent::new(2));
        let exec = RunNowExecutor::new();
        let latch2 = latch.clone();
        let latch3 = latch.clone();
        exec.execute(move || {
            let _ = latch2.decrement();
        });
        exec.execute(move || {
            let _ = latch3.decrement();
        });
        let res = latch.wait_timeout(Duration::from_secs(5));
        assert_eq!(res, 0);
    }

    #[test]
    fn shutdown_from_worker() {
        let _ = env_logger::try_init();

        let exec = RunNowExecutor::new();
        let exec2 = exec.clone();
        let latch = Arc::new(CountdownEvent::new(2));
        let latch2 = latch.clone();
        let latch3 = latch.clone();
        let stop_latch = Arc::new(CountdownEvent::new(1));
        let stop_latch2 = stop_latch.clone();
        exec.execute(move || {
            let _ = latch2.decrement();
        });
        exec.execute(move || {
            exec2.shutdown_async();
            let _ = stop_latch2.decrement();
        });
        let res = stop_latch.wait_timeout(Duration::from_secs(1));
        assert_eq!(res, 0);
        exec.execute(move || {
            let _ = latch3.decrement();
        });
        let res = latch.wait_timeout(Duration::from_secs(1));
        assert_eq!(res, 1);
    }

    #[test]
    fn shutdown_external() {
        let _ = env_logger::try_init();

        let exec = RunNowExecutor::new();
        let exec2 = exec.clone();
        let latch = Arc::new(CountdownEvent::new(2));
        let latch2 = latch.clone();
        let latch3 = latch.clone();
        exec.execute(move || {
            let _ = latch2.decrement();
        });
        exec.shutdown().expect("pool to shut down");
        exec2.execute(move || {
            let _ = latch3.decrement();
        });
        let res = latch.wait_timeout(Duration::from_secs(1));
        assert_eq!(res, 1);
    }
}
