// Copyright 2017-2020 Lars Kroll. See the LICENSE
// file at the top-level directory of this distribution.
//
// Licensed under the MIT license
// <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed
// except according to those terms.

//! Support for Rust's futures and async/await APIs
//!
//! This functionality is provided via the [FuturesExecutor](FuturesExecutor)
//! trait, which is implemented for all executors in this crate that can efficiently support it.

use super::*;
use std::future::Future;

/// A future that can be used to await the result of a spawned future
pub type JoinHandle<R> = async_task::Task<R>;

/// A trait for spawning futures on an Executor
pub trait FuturesExecutor: Executor + Sync + 'static {
    /// Spawn `future` on the pool and return a handle to the result
    ///
    /// Handles can be awaited like any other future.
    ///
    /// # Examples
    ///
    /// Execute an "expensive" computation on the pool and
    /// block the main thread waiting for the result to become available.
    ///
    /// ```
    /// use executors::*;
    /// use futures::executor::block_on;
    /// # use executors::crossbeam_channel_pool::ThreadPool;
    /// // initialise some executor
    /// # let executor = ThreadPool::new(2);
    /// let handle = executor.spawn(async move { 2*2 });
    /// let result = block_on(handle);
    /// assert_eq!(4, result);
    /// # executor.shutdown().expect("shutdown");
    /// ```
    fn spawn<R: Send + 'static>(
        &self,
        future: impl Future<Output = R> + 'static + Send,
    ) -> JoinHandle<R>;
}

#[cfg(test)]
mod tests {
    use super::*;

    use futures::channel::oneshot::*;
    use std::{
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        },
        thread,
        time::Duration,
    };

    async fn just_succeed(barrier: Arc<AtomicBool>) -> () {
        let res = barrier.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst);
        assert!(res.is_ok());
    }

    #[test]
    fn test_async_ready_ccp() {
        let exec = crate::crossbeam_channel_pool::ThreadPool::new(2);
        test_async_ready_executor(&exec);
        exec.shutdown().expect("shutdown");
    }

    #[test]
    fn test_async_ready_cwp() {
        let exec = crate::crossbeam_workstealing_pool::small_pool(2);
        test_async_ready_executor(&exec);
        exec.shutdown().expect("shutdown");
    }

    fn test_async_ready_executor<E>(exec: &E)
    where
        E: FuturesExecutor,
    {
        let barrier = Arc::new(AtomicBool::new(false));
        let f = just_succeed(barrier.clone());
        exec.spawn(f).detach();
        let mut done = false;
        while !done {
            thread::sleep(Duration::from_millis(100));
            done = barrier.load(Ordering::SeqCst);
        }
    }

    async fn wait_for_channel(receiver: Receiver<()>, barrier: Arc<AtomicBool>) -> () {
        let _ok = receiver.await.expect("message");
        let res = barrier.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst);
        assert!(res.is_ok());
    }

    // Does not implement Sync
    // #[test]
    // fn test_async_pending_tpe() {
    // 	let exec = crate::threadpool_executor::ThreadPoolExecutor::new(2);
    // 	test_async_pending_executor(&exec);
    // 	exec.shutdown().expect("shutdown");
    // }

    #[test]
    fn test_async_pending_ccp() {
        let exec = crate::crossbeam_channel_pool::ThreadPool::new(2);
        test_async_pending_executor(&exec);
        exec.shutdown().expect("shutdown");
    }

    #[test]
    fn test_async_pending_cwp() {
        let exec = crate::crossbeam_workstealing_pool::small_pool(2);
        test_async_pending_executor(&exec);
        exec.shutdown().expect("shutdown");
    }

    fn test_async_pending_executor<E>(exec: &E)
    where
        E: FuturesExecutor,
    {
        let barrier = Arc::new(AtomicBool::new(false));
        let (tx, rx) = channel();
        let f = wait_for_channel(rx, barrier.clone());
        exec.spawn(f).detach();
        thread::sleep(Duration::from_millis(100));
        tx.send(()).expect("sent");
        let mut done = false;
        while !done {
            thread::sleep(Duration::from_millis(100));
            done = barrier.load(Ordering::SeqCst);
        }
    }

    #[test]
    fn test_async_result_ccp() {
        let exec = crate::crossbeam_channel_pool::ThreadPool::new(2);
        test_async_result_executor(&exec);
        exec.shutdown().expect("shutdown");
    }

    #[test]
    fn test_async_result_cwp() {
        let exec = crate::crossbeam_workstealing_pool::small_pool(2);
        test_async_result_executor(&exec);
        exec.shutdown().expect("shutdown");
    }

    fn test_async_result_executor<E>(exec: &E)
    where
        E: FuturesExecutor,
    {
        let test_string = "test".to_string();
        let (tx, rx) = channel::<String>();
        let handle = exec.spawn(async move { rx.await.expect("message") });
        thread::sleep(Duration::from_millis(100));
        tx.send(test_string.clone()).expect("sent");
        let res = futures::executor::block_on(handle);
        assert_eq!(res, test_string)
    }
}
