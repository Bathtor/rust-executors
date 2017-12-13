// Copyright 2017 Lars Kroll. See the LICENSE
// file at the top-level directory of this distribution.
//
// Licensed under the MIT license
// <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed
// except according to those terms.

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
    fn shutdown(self) -> Result<(), String>;   
}

/// A simple method to explicitly throw away return parameters.
///
/// # Examples
///
/// Ignoring a Result.
///
/// ```
/// use executors::common::*;
/// let res: Result<(), String> = Ok(());
/// ignore(res);
/// ```
#[inline(always)]
pub fn ignore<V>(_: V) -> () {
    ()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignore_primitives() {
        assert_eq!(ignore(2 + 2), ());
    }

    struct SomeStruct {
        a: u32,
        b: f64,
        c: bool,
    }

    #[test]
    fn ignore_objects() {
        let v = SomeStruct {
            a: 1,
            b: 2.0,
            c: true,
        };
        assert_eq!(ignore(v), ());
    }
}
