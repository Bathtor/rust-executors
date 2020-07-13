use super::*;
use std::cell::UnsafeCell;

// UnsafeCell has 10x the performance of RefCell
// and the scoping guarantees that the borrows are exclusive
thread_local!(
    static LOCAL_EXECUTOR: UnsafeCell<Option<Box<dyn CanExecute>>> = UnsafeCell::new(Option::None);
);

pub(crate) fn set_local_executor<E>(executor: E)
where
    E: CanExecute + 'static,
{
    LOCAL_EXECUTOR.with(move |e| unsafe {
        *e.get() = Some(Box::new(executor));
    });
}

pub(crate) fn unset_local_executor() {
    LOCAL_EXECUTOR.with(move |e| unsafe {
        *e.get() = None;
    });
}

/// Tries run the job on the same executor that spawned the thread running the job.
///
/// This is not supported on all executor implementations,
/// and will return `Err` variant with the original job if it can't be scheduled.
pub fn try_execute_locally<F>(f: F) -> Result<(), F>
where
    F: FnOnce() + Send + 'static,
{
    LOCAL_EXECUTOR.with(move |e| {
        if let Some(Some(ref boxed)) = unsafe { e.get().as_ref() } {
            boxed.execute_job(Box::new(f));
            Ok(())
        } else {
            Err(f)
        }
    })
}

// #[cfg(feature = "futures-support")]
// mod futures_locals {
// 	use super::*;
// 	use crate::futures_executor::FuturesExecutor;

// 	// UnsafeCell has 10x the performance of RefCell
// // and the scoping guarantees that the borrows are exclusive
// thread_local!(
//     static LOCAL_FUTURES_EXECUTOR: UnsafeCell<Option<Box<dyn FuturesExecutor>>> = UnsafeCell::new(Option::None);
// );

// pub fn set_local_futures_executor<E>(executor: E) where E: FuturesExecutor {
//     LOCAL_FUTURES_EXECUTOR.with(move |e| unsafe {
//     	*e.get() = Some(Box::new(executor));
//     });
// }
// }
