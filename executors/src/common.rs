// Copyright 2017 Lars Kroll. See the LICENSE
// file at the top-level directory of this distribution.
//
// Licensed under the MIT license
// <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed
// except according to those terms.

use std::fmt::Debug;

/// A common trait for task executors.
/// 
/// All implementations need to allow cloning to create new handles to 
/// the same executor, and they need to be safe to pass to threads.
pub trait Executor : Clone+Send {
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
        F: FnOnce() + Send + 'static;
        
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
    /// 		println!("Shutting down");
    ///		executor2.shutdown_async();
    ///	});
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
}

/// A simple method to explicitly throw away return parameters.
///
/// # Examples
///
/// Ignoring a `Result`.
///
/// ```ignore
/// use executors::common::*;
/// let res: Result<(), String> = Ok(());
/// ignore(res);
/// ```
#[inline(always)]
pub(crate) fn ignore<V>(_: V) -> () {
    ()
}

/// A trait to log errors when ignoring results.
///
/// # Examples
/// 
/// Log to warn when ignoring a `Result`
/// 
/// ```ignore
/// use executors::common::*;
/// let res: Result<(), String> = Err(String::from("Test error please ignore."));
/// res.log_warn("Result was an error");
/// ```
pub(crate) trait LogErrors {
    fn log_error(self, msg: &str);
    fn log_warn(self, msg: &str);
    fn log_info(self, msg: &str);
    fn log_debug(self, msg: &str);
}

impl<V, E: Debug> LogErrors for Result<V, E> {
    fn log_error(self, msg: &str) {
        ignore(self.map_err(|e| error!("{}: {:?}", msg, e)));
    }
    fn log_warn(self, msg: &str) {
        ignore(self.map_err(|e| warn!("{}: {:?}", msg, e)));
    }
    fn log_info(self, msg: &str) {
        ignore(self.map_err(|e| info!("{}: {:?}", msg, e)));
    }
    fn log_debug(self, msg: &str) {
        ignore(self.map_err(|e| debug!("{}: {:?}", msg, e)));
    }
}

//impl<V, E: Display> LogErrors for Result<V, E> {
//    fn log_error(self, msg: &str) {
//        ignore(self.map_err(|e| error!("{}: {:?}", msg, e)));
//    }
//    fn log_warn(self, msg: &str) {
//        ignore(self.map_err(|e| warn!("{}: {:?}", msg, e)));
//    }
//    fn log_info(self, msg: &str) {
//        ignore(self.map_err(|e| info!("{}: {:?}", msg, e)));
//    }
//    fn log_debug(self, msg: &str) {
//        ignore(self.map_err(|e| debug!("{}: {:?}", msg, e)));
//    }
//}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignore_primitives() {
        assert_eq!(ignore(2 + 2), ());
    }

    struct SomeStruct {
        _a: u32,
        _b: f64,
        _c: bool,
    }

    #[test]
    fn ignore_objects() {
        let v = SomeStruct {
            _a: 1,
            _b: 2.0,
            _c: true,
        };
        assert_eq!(ignore(v), ());
    }
}
