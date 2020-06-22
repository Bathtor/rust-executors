// Copyright 2017-2020 Lars Kroll. See the LICENSE
// file at the top-level directory of this distribution.
//
// Licensed under the MIT license
// <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed
// except according to those terms.

use super::*;
use std::future::Future;

pub trait FuturesExecutor: Executor + Sync + 'static {
    fn spawn(&self, future: impl Future<Output = ()> + 'static + Send) -> ();
}

#[cfg(test)]
mod tests {
    use super::*;

    use futures::channel::oneshot::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    async fn just_succeed(barrier: Arc<AtomicBool>) -> () {
        let res = barrier.compare_and_swap(false, true, Ordering::SeqCst);
        assert!(!res); //  i.e. assert that the old value was false
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
        exec.spawn(f);
        let mut done = false;
        while !done {
            thread::sleep(Duration::from_millis(100));
            done = barrier.load(Ordering::SeqCst);
        }
    }

    async fn wait_for_channel(receiver: Receiver<()>, barrier: Arc<AtomicBool>) -> () {
        let _ok = receiver.await.expect("message");
        let res = barrier.compare_and_swap(false, true, Ordering::SeqCst);
        assert!(!res); //  i.e. assert that the old value was false
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
        exec.spawn(f);
        thread::sleep(Duration::from_millis(100));
        tx.send(()).expect("sent");
        let mut done = false;
        while !done {
            thread::sleep(Duration::from_millis(100));
            done = barrier.load(Ordering::SeqCst);
        }
    }
}
