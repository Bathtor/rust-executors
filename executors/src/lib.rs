// Copyright 2017-2020 Lars Kroll. See the LICENSE
// file at the top-level directory of this distribution.
//
// Licensed under the MIT license
// <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed
// except according to those terms.
#![doc(html_root_url = "https://docs.rs/executors/0.9.0")]
#![deny(missing_docs)]
#![allow(unused_parens)]
#![allow(clippy::unused_unit)]

//! This crate provides a number of task executors all implementing the
//! [`Executor`](common/trait.Executor.html) trait.
//!
//! General examples can be found in the [`Executor`](common/trait.Executor.html) trait
//! documentation, and implementation specific examples with each implementation module.
//!
//! # Crate Feature Flags
//!
//! The following crate feature flags are available. They are configured in your `Cargo.toml`.
//!
//! - `threadpool-exec` (default)
//!     - Provides a wrapper implementation around the [threadpool](https://crates.io/crates/threadpool) crate.
//!     - See [threadpool_executor](threadpool_executor).
//! - `cb-channel-exec` (default)
//!     - Provides a thread pool executor with a single global queue.
//!     - See [crossbeam_channel_pool](crossbeam_channel_pool).
//! - `workstealing-exec` (default)
//!     - Provides a thread pool executor with thread-local queues in addition to a global injector queue.
//!     - See [crossbeam_workstealing_pool](crossbeam_workstealing_pool).
//! - `defaults` (default)
//!     - Produces [Default](std::default::Default) implementations using [num_cpus](https://crates.io/crates/num_cpus) to determine pool sizes.
//! - `ws-timed-fairness` (default)
//!     - This feature flag determines the fairness mechanism between local and global queues in the [crossbeam_workstealing_pool](crossbeam_workstealing_pool).
//!     - If the flag is enabled the fairness is time-based. The global queue will be checked every 100ms.
//!     - If the flags is absent the fairness is count-based. The global queue will be checked every 100 local jobs.
//!     - Which one you should pick depends on your application.
//!     - Time-based fairness is a compromise between latency of externally scheduled jobs and overall throughput.
//!     - Count-based is going to depend heavily on how long your jobs typically are, but counting is cheaper than checking time, so it can lead to higher throughput.
//! - `ws-no-park`
//!     - Disable thread parking for the [crossbeam_workstealing_pool](crossbeam_workstealing_pool).
//!     - This is generally detrimental to performance, as idle threads will unnecessarily hang on to CPU resources.
//!     - However, for very latency sensitive interactions with external resources (e.g., I/O), this can reduce overall job latency.
//! - `thread-pinning`
//!     - Allows pool threads to be pinned to specific cores.
//!     - This can reduce cache invalidation overhead when threads sleep and then are woken up later.
//!     - However, if your cores are needed by other processes, it can also introduce additional scheduling delay, if the pinned core isn't available immediately at wake time.
//!     - Use with care.
//! - `numa-aware`
//!     - Make memory architecture aware decisions.
//!     - Concretely this setting currently only affects [crossbeam_workstealing_pool](crossbeam_workstealing_pool).
//!     - When it is enabled, work-stealing will happen by memory proximity.
//!     - That is threads with too little work will try to steal work from memory-close other threads first, before trying further away threads.
//! - `produce-metrics`
//!     - Every executor provided in this crate can produce metrics using the [metrics](https://crates.io/crates/metrics) crate.
//!     - The metrics are `executors.jobs_executed` (*"How many jobs were executed in total?"*) and `executors.jobs_queued` (*"How many jobs are currently waiting to be executed?"*).
//!     - Not all executors produce all metrics.
//!     - **WARNING**: Collecting these metrics typically has a serious performance impact. You should only consider using this in production if your jobs are fairly large anyway (say in the millisecond range).

#[macro_use]
extern crate log;

pub mod bichannel;
pub mod common;
#[cfg(feature = "cb-channel-exec")]
pub mod crossbeam_channel_pool;
#[cfg(feature = "workstealing-exec")]
pub mod crossbeam_workstealing_pool;
#[cfg(feature = "numa-aware")]
pub mod numa_utils;
pub mod parker;
pub mod run_now;
#[cfg(feature = "threadpool-exec")]
pub mod threadpool_executor;
mod timeconstants;

pub use crate::common::{CanExecute, Executor};

#[cfg(any(feature = "cb-channel-exec", feature = "workstealing-exec"))]
pub mod futures_executor;
#[cfg(any(feature = "cb-channel-exec", feature = "workstealing-exec"))]
pub use crate::futures_executor::{FuturesExecutor, JoinHandle};

//use bichannel::*;
use synchronoise::CountdownEvent;

mod locals;
pub use locals::*;

#[cfg(feature = "produce-metrics")]
use metrics::{
    counter,
    decrement_gauge,
    increment_counter,
    increment_gauge,
    register_counter,
    register_gauge,
};

// #[cfg(feature = "produce-metrics")]
// pub mod metric_keys {
//     /// Counts the total number of jobs that were executed.
//     ///
//     /// This is a metric key.
//     pub const JOBS_EXECUTED: &str = "executors.jobs_executed";

//     pub(crate) const JOBS_EXECUTED_DESCRIPTION: &str =
//         "The total number of jobs that were executed";

//     /// Counts the number of jobs that are currently waiting to be executed.
//     ///
//     /// This is a metric key.
//     pub const JOBS_QUEUED: &str = "executors.jobs_queued";

//     pub(crate) const JOBS_QUEUED_DESCRIPTION: &str =
//         "The number of jobs that are currently waiting to be executed";

//     /// The concrete [Executor](Executor) implementation that logged record.
//     ///
//     /// This is a label key.
//     pub const EXECUTOR: &str = "executor";

//     /// The id of the thread that logged record.
//     ///
//     /// This is only present on multi-threaded executors.
//     ///
//     /// This is a label key.
//     pub const THREAD_ID: &str = "thread_id";
// }

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::{
        fmt::Debug,
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        },
        time::Duration,
    };

    pub const N_DEPTH_SMALL: usize = 1024;
    pub const N_DEPTH: usize = 8192; // run_now can't do this, but it's a good test for the others
    pub const N_WIDTH: usize = 128;
    pub const N_SLEEP_ROUNDS: usize = 11;

    pub const TEST_TIMEOUT: Duration = Duration::from_secs(240);

    pub fn test_debug<E>(exec: &E, label: &str)
    where
        E: Executor + Debug,
    {
        println!("Debug output for {}: {:?}", label, exec);
    }

    pub fn test_small_defaults<E>(label: &str)
    where
        E: Executor + Debug + std::default::Default + 'static,
    {
        let _ = env_logger::try_init();
        #[cfg(feature = "produce-metrics")]
        metrics_printer::init();

        let pool = E::default();
        #[cfg(feature = "produce-metrics")]
        pool.register_metrics();

        let latch = Arc::new(CountdownEvent::new(N_DEPTH_SMALL * N_WIDTH));
        for _ in 0..N_WIDTH {
            let pool2 = pool.clone();
            let latch2 = latch.clone();
            pool.execute(move || {
                do_step(latch2, pool2, N_DEPTH_SMALL);
            });
        }
        let res = latch.wait_timeout(TEST_TIMEOUT);
        assert_eq!(res, 0);
        pool.shutdown()
            .unwrap_or_else(|e| error!("Error during pool shutdown {:?} at {}", e, label));
    }

    pub fn test_defaults<E>(label: &str)
    where
        E: Executor + Debug + std::default::Default + 'static,
    {
        #[cfg(feature = "produce-metrics")]
        metrics_printer::init();

        let pool = E::default();
        #[cfg(feature = "produce-metrics")]
        pool.register_metrics();

        let latch = Arc::new(CountdownEvent::new(N_DEPTH * N_WIDTH));
        for _ in 0..N_WIDTH {
            let pool2 = pool.clone();
            let latch2 = latch.clone();
            pool.execute(move || {
                do_step(latch2, pool2, N_DEPTH);
            });
        }
        let res = latch.wait_timeout(TEST_TIMEOUT);
        assert_eq!(res, 0);
        pool.shutdown()
            .unwrap_or_else(|e| error!("Error during pool shutdown {:?} at {}", e, label));
    }

    pub fn test_custom<E>(exec: E, label: &str)
    where
        E: Executor + Debug + 'static,
    {
        #[cfg(feature = "produce-metrics")]
        metrics_printer::init();

        let pool = exec;
        #[cfg(feature = "produce-metrics")]
        pool.register_metrics();

        let latch = Arc::new(CountdownEvent::new(N_DEPTH * N_WIDTH));
        for _ in 0..N_WIDTH {
            let pool2 = pool.clone();
            let latch2 = latch.clone();
            pool.execute(move || {
                do_step(latch2, pool2, N_DEPTH);
            });
        }
        let res = latch.wait_timeout(TEST_TIMEOUT);
        assert_eq!(res, 0);
        pool.shutdown()
            .unwrap_or_else(|e| error!("Error during pool shutdown {:?} at {}", e, label));
    }

    pub fn test_sleepy<E>(pool: E, label: &str)
    where
        E: Executor + 'static,
    {
        #[cfg(feature = "produce-metrics")]
        metrics_printer::init();

        #[cfg(feature = "produce-metrics")]
        pool.register_metrics();

        info!("Running sleepy test for {}", label);
        let latch = Arc::new(CountdownEvent::new(N_SLEEP_ROUNDS * N_WIDTH));
        for round in 1..=N_SLEEP_ROUNDS {
            // let threads go to sleep
            let sleep_time = 1u64 << round;
            std::thread::sleep(Duration::from_millis(sleep_time));
            for _ in 0..N_WIDTH {
                let latch2 = latch.clone();
                pool.execute(move || {
                    latch2.decrement().expect("Latch didn't decrement!");
                });
            }
        }
        let res = latch.wait_timeout(Duration::from_secs((N_SLEEP_ROUNDS as u64) * 3));
        assert_eq!(res, 0);
        pool.shutdown()
            .unwrap_or_else(|e| error!("Error during pool shutdown {:?} at {}", e, label));
    }

    pub fn test_local<E>(exec: E, label: &str)
    where
        E: Executor + Debug + 'static,
    {
        #[cfg(feature = "produce-metrics")]
        metrics_printer::init();

        let pool = exec;
        #[cfg(feature = "produce-metrics")]
        pool.register_metrics();

        let latch = Arc::new(CountdownEvent::new(N_DEPTH * N_WIDTH));
        let failed = Arc::new(AtomicBool::new(false));
        for _ in 0..N_WIDTH {
            let latch2 = latch.clone();
            let failed2 = failed.clone();
            pool.execute(move || {
                do_step_local(latch2, failed2, N_DEPTH);
            });
        }
        let res = latch.wait_timeout(TEST_TIMEOUT);
        assert_eq!(res, 0);
        assert!(
            !failed.load(Ordering::SeqCst),
            "Executor does not support local scheduling!"
        );
        pool.shutdown()
            .unwrap_or_else(|e| error!("Error during pool shutdown {:?} at {}", e, label));
    }

    pub fn test_small_local<E>(exec: E, label: &str)
    where
        E: Executor + Debug + 'static,
    {
        #[cfg(feature = "produce-metrics")]
        metrics_printer::init();

        let pool = exec;
        #[cfg(feature = "produce-metrics")]
        pool.register_metrics();

        let latch = Arc::new(CountdownEvent::new(N_DEPTH_SMALL * N_WIDTH));
        let failed = Arc::new(AtomicBool::new(false));
        for _ in 0..N_WIDTH {
            let latch2 = latch.clone();
            let failed2 = failed.clone();
            pool.execute(move || {
                do_step_local(latch2, failed2, N_DEPTH_SMALL);
            });
        }
        let res = latch.wait_timeout(TEST_TIMEOUT);
        assert_eq!(res, 0);
        assert!(
            !failed.load(Ordering::SeqCst),
            "Executor does not support local scheduling!"
        );
        pool.shutdown()
            .unwrap_or_else(|e| error!("Error during pool shutdown {:?} at {}", e, label));
    }

    fn do_step<E>(latch: Arc<CountdownEvent>, pool: E, depth: usize)
    where
        E: Executor + Debug + 'static,
    {
        let new_depth = depth - 1;
        latch.decrement().expect("Latch didn't decrement!");
        if (new_depth > 0) {
            let pool2 = pool.clone();
            pool.execute(move || do_step(latch, pool2, new_depth))
        }
    }

    fn do_step_local(latch: Arc<CountdownEvent>, failed: Arc<AtomicBool>, depth: usize) {
        let new_depth = depth - 1;
        match latch.decrement() {
            Ok(_) => {
                if (new_depth > 0) {
                    let failed2 = failed.clone();
                    let latch2 = latch.clone();
                    let res =
                        try_execute_locally(move || do_step_local(latch2, failed2, new_depth));
                    if res.is_err() {
                        error!("do_step_local should have executed locally!");
                        failed.store(true, Ordering::SeqCst);
                        while latch.decrement().is_ok() {
                            () // do nothing, just keep draining, so the main thread wakes up
                        }
                    }
                }
            }
            Err(e) => {
                if failed.load(Ordering::SeqCst) {
                    warn!("Aborting test as it failed");
                // and simply return here
                } else {
                    panic!("Latch didn't decrement! Error: {:?}", e);
                }
            }
        }
    }
}
