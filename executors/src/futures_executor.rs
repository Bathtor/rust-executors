// Copyright 2017-2020 Lars Kroll. See the LICENSE
// file at the top-level directory of this distribution.
//
// Licensed under the MIT license
// <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed
// except according to those terms.

use super::*;
use futures::{
    future::{BoxFuture, FutureExt},
    task::{waker_ref, ArcWake},
};
use std::{
    cell::UnsafeCell,
    future::Future,
    sync::Arc,
    task::{Context, Poll},
};

pub trait FuturesExecutor: Executor + Sync + 'static {
    fn spawn(&self, future: impl Future<Output = ()> + 'static + Send) -> ();
}
impl<E> FuturesExecutor for E
where
    E: Executor + Sync + 'static,
{
    fn spawn(&self, future: impl Future<Output = ()> + 'static + Send) -> () {
        let future = future.boxed();
        let task = FunTask {
            future: UnsafeCell::new(Some(future)),
            executor: self.clone(),
        };
        let task = Arc::new(task);
        self.execute(move || FunTask::run(task));
    }
}

struct FunTask<E>
where
    E: Executor + Sync + 'static,
{
    future: UnsafeCell<Option<BoxFuture<'static, ()>>>,
    executor: E,
}

impl<E> FunTask<E>
where
    E: Executor + Sync + 'static,
{
    fn run(task: Arc<Self>) -> () {
        let res = unsafe {
            let src = task.future.get();
            src.as_mut().map(|opt| opt.take()).flatten()
        };
        if let Some(mut f) = res {
            let waker = waker_ref(&task);
            let context = &mut Context::from_waker(&*waker);
            // `BoxFuture<T>` is a type alias for
            // `Pin<Box<dyn Future<Output = T> + Send + 'static>>`.
            // We can get a `Pin<&mut dyn Future + Send + 'static>`
            // from it by calling the `Pin::as_mut` method.
            if let Poll::Pending = f.as_mut().poll(context) {
                // We're not done processing the future, so put it
                // back in its task to be run again in the future.
                unsafe {
                    let dst = task.future.get();
                    if let Some(slot) = dst.as_mut() {
                        *slot = Some(f);
                    }
                }
            }
        } // else the future is already done with
    }
}

impl<E> ArcWake for FunTask<E>
where
    E: Executor + Sync + 'static,
{
    fn wake_by_ref(arc_self: &Arc<Self>) {
        // Implement `wake` by sending this task back onto the task channel
        // so that it will be polled again by the executor.
        let cloned = arc_self.clone();
        arc_self.executor.execute(move || FunTask::run(cloned));
    }
}

// I'm making sure only one thread at a time has access to the contents here
unsafe impl<E> Sync for FunTask<E> where E: Executor + Sync + 'static {}

#[cfg(test)]
mod tests {
    use super::*;

    use futures::channel::oneshot::*;
    use std::sync::atomic::{AtomicBool, Ordering};
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
