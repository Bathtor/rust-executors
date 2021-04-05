// Copyright 2017-2020 Lars Kroll. See the LICENSE
// file at the top-level directory of this distribution.
//
// Licensed under the MIT license
// <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed
// except according to those terms.

//! The core traits and reusable functions of this crate.

use std::fmt::Debug;

/// A minimal trait for task executors.
///
/// This is mostly useful as a narrowed view for dynamic executor references.
pub trait CanExecute {
    /// Executes the function `job` on the `Executor`.
    ///
    /// This is the same as [execute](Executor::execute), but already boxed up.
    fn execute_job(&self, job: Box<dyn FnOnce() + Send + 'static>);
}

/// A common trait for task executors.
///
/// All implementations need to allow cloning to create new handles to
/// the same executor, and they need to be safe to pass to threads.
pub trait Executor: CanExecute + Clone + Send {
    /// Executes the function `job` on the `Executor`.
    ///
    /// # Examples
    ///
    /// Execute four jobs on an `Executor`:
    ///
    /// ```
    /// use executors::*;
    /// # use executors::crossbeam_channel_pool::ThreadPool;
    /// // initialise some executor
    /// # let executor = ThreadPool::new(2);
    /// executor.execute(|| println!("hello"));
    /// executor.execute(|| println!("world"));
    /// executor.execute(|| println!("foo"));
    /// executor.execute(|| println!("bar"));
    /// // wait for jobs to be executed
    /// std::thread::sleep(std::time::Duration::from_secs(1));
    /// ```
    fn execute<F>(&self, job: F)
    where
        F: FnOnce() + Send + 'static,
    {
        self.execute_job(Box::new(job));
    }

    /// Shutdown the `Executor` without waiting.
    ///
    /// This method can be used from one of the worker threads
    /// (if the `Executor` uses threads)
    /// without risk of deadlocking.
    ///
    /// # Examples
    ///
    /// Shutdown an `Executor` with threads from within a worker.
    ///
    /// ```
    /// use executors::*;
    /// # use executors::crossbeam_channel_pool::ThreadPool;
    /// // initialise some executor
    /// # let executor = ThreadPool::new(1);
    /// let executor2 = executor.clone();
    /// executor.execute(|| println!("Hello!"));
    /// executor.execute(move || {
    ///     println!("Shutting down");
    ///     executor2.shutdown_async();
    /// });
    /// std::thread::sleep(std::time::Duration::from_secs(1)); // or wait with a barrier
    /// executor.execute(|| println!("doesn't work!"));
    /// ```
    fn shutdown_async(&self);

    /// Shutdown an `Executor` and wait for it to shut down all workers.
    ///
    /// This method can be ONLY be used from *outside* the workers
    /// without risk of deadlocking.
    ///
    ///
    /// # Examples
    ///
    /// Shutdown an `Executor` with threads from an external thread.
    ///
    /// ```
    /// use executors::*;
    /// # use executors::crossbeam_channel_pool::ThreadPool;
    /// // initialise some executor
    /// # let executor = ThreadPool::new(1);
    /// let executor2 = executor.clone();
    /// executor.execute(|| println!("Hello!"));
    /// executor.shutdown().expect("pool to shut down");
    /// executor2.execute(|| println!("doesn't work!"));
    /// ```
    fn shutdown(self) -> Result<(), String> {
        self.shutdown_borrowed()
    }

    /// Same as `shutdown` but without consuming self
    ///
    /// Meant for use with trait object wrappers.
    fn shutdown_borrowed(&self) -> Result<(), String>;

    /// Register all the metrics this executor exposes.
    #[cfg(feature = "produce-metrics")]
    fn register_metrics(&self);
}

// A trait to log errors when ignoring results.
//
// # Examples
//
// Log to warn when ignoring a `Result`
//
// ```
// use executors::common::*;
// let res: Result<(), String> = Err(String::from("Test error please ignore."));
// res.log_warn("Result was an error");
// ```
// NOTE: Don't generate docs for this, so the test is never run
pub(crate) trait LogErrors {
    fn log_error(self, msg: &str);
    fn log_warn(self, msg: &str);
    fn log_info(self, msg: &str);
    fn log_debug(self, msg: &str);
}

impl<V, E: Debug> LogErrors for Result<V, E> {
    fn log_error(self, msg: &str) {
        let _ = self.map_err(|e| error!("{}: {:?}", msg, e));
    }

    fn log_warn(self, msg: &str) {
        let _ = self.map_err(|e| warn!("{}: {:?}", msg, e));
    }

    fn log_info(self, msg: &str) {
        let _ = self.map_err(|e| info!("{}: {:?}", msg, e));
    }

    fn log_debug(self, msg: &str) {
        let _ = self.map_err(|e| debug!("{}: {:?}", msg, e));
    }
}
