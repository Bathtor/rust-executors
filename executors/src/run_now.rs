// Copyright 2017 Lars Kroll. See the LICENSE
// file at the top-level directory of this distribution.
//
// Licensed under the MIT license
// <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed
// except according to those terms.

//! A simple `Executor` that simply runs tasks on the current thread.
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

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone)]
pub struct RunNowExecutor {
    active: Arc<AtomicBool>,
}

use super::*;

impl RunNowExecutor {
    pub fn new() -> RunNowExecutor {
        RunNowExecutor { active: Arc::new(AtomicBool::new(true)) }
    }
}

impl Executor for RunNowExecutor {
    fn execute<F>(&self, job: F)
    where
        F: FnOnce() + Send + 'static,
    {
        if self.active.load(Ordering::SeqCst) {
            job();
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
            debug!("Shutting down executor.");
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

    #[test]
    fn run_tasks() {
        let _ = env_logger::try_init();

        let latch = Arc::new(CountdownEvent::new(2));
        let exec = RunNowExecutor::new();
        let latch2 = latch.clone();
        let latch3 = latch.clone();
        exec.execute(move || ignore(latch2.decrement()));
        exec.execute(move || ignore(latch3.decrement()));
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
        exec.execute(move || ignore(latch2.decrement()));
        exec.execute(move || {
            exec2.shutdown_async();
            ignore(stop_latch2.decrement());
        });
        let res = stop_latch.wait_timeout(Duration::from_secs(1));
        assert_eq!(res, 0);
        exec.execute(move || ignore(latch3.decrement()));
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
        exec.execute(move || ignore(latch2.decrement()));
        exec.shutdown().expect("pool to shut down");
        exec2.execute(move || ignore(latch3.decrement()));
        let res = latch.wait_timeout(Duration::from_secs(1));
        assert_eq!(res, 1);
    }
}
