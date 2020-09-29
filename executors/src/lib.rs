// Copyright 2017-2020 Lars Kroll. See the LICENSE
// file at the top-level directory of this distribution.
//
// Licensed under the MIT license
// <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed
// except according to those terms.
#![doc(html_root_url = "https://docs.rs/executors/0.8.0")]
#![deny(missing_docs)]
#![allow(unused_parens)]
#![allow(clippy::unused_unit)]

//! This crate provides a number of task executors all implementing the
//! [`Executor`](common/trait.Executor.html) trait.
//!
//! General examples can be found in the [`Executor`](common/trait.Executor.html) trait
//! documentation, and implementation specific examples with each implementation module.

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

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::fmt::Debug;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    pub const N_DEPTH_SMALL: usize = 1024;
    pub const N_DEPTH: usize = 8192; // run_now can't do this, but it's a good test for the others
    pub const N_WIDTH: usize = 128;
    pub const N_SLEEP_ROUNDS: usize = 10;

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
        let pool = E::default();

        let latch = Arc::new(CountdownEvent::new(N_DEPTH_SMALL * N_WIDTH));
        for _ in 0..N_WIDTH {
            let pool2 = pool.clone();
            let latch2 = latch.clone();
            pool.execute(move || {
                do_step(latch2, pool2, N_DEPTH_SMALL);
            });
        }
        let res = latch.wait_timeout(Duration::from_secs(30));
        assert_eq!(res, 0);
        pool.shutdown()
            .unwrap_or_else(|e| error!("Error during pool shutdown {:?} at {}", e, label));
    }

    pub fn test_defaults<E>(label: &str)
    where
        E: Executor + Debug + std::default::Default + 'static,
    {
        let pool = E::default();

        let latch = Arc::new(CountdownEvent::new(N_DEPTH * N_WIDTH));
        for _ in 0..N_WIDTH {
            let pool2 = pool.clone();
            let latch2 = latch.clone();
            pool.execute(move || {
                do_step(latch2, pool2, N_DEPTH);
            });
        }
        let res = latch.wait_timeout(Duration::from_secs(60));
        assert_eq!(res, 0);
        pool.shutdown()
            .unwrap_or_else(|e| error!("Error during pool shutdown {:?} at {}", e, label));
    }

    pub fn test_custom<E>(exec: E, label: &str)
    where
        E: Executor + Debug + 'static,
    {
        let pool = exec;

        let latch = Arc::new(CountdownEvent::new(N_DEPTH * N_WIDTH));
        for _ in 0..N_WIDTH {
            let pool2 = pool.clone();
            let latch2 = latch.clone();
            pool.execute(move || {
                do_step(latch2, pool2, N_DEPTH);
            });
        }
        let res = latch.wait_timeout(Duration::from_secs(30));
        assert_eq!(res, 0);
        pool.shutdown()
            .unwrap_or_else(|e| error!("Error during pool shutdown {:?} at {}", e, label));
    }

    pub fn test_sleepy<E>(pool: E, label: &str)
    where
        E: Executor + 'static,
    {
        info!("Running sleepy test for {}", label);
        let latch = Arc::new(CountdownEvent::new(N_SLEEP_ROUNDS * N_WIDTH));
        for _round in 0..N_SLEEP_ROUNDS {
            // let threads go to sleep
            std::thread::sleep(Duration::from_secs(1));
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
        let pool = exec;

        let latch = Arc::new(CountdownEvent::new(N_DEPTH * N_WIDTH));
        let failed = Arc::new(AtomicBool::new(false));
        for _ in 0..N_WIDTH {
            let latch2 = latch.clone();
            let failed2 = failed.clone();
            pool.execute(move || {
                do_step_local(latch2, failed2, N_DEPTH);
            });
        }
        let res = latch.wait_timeout(Duration::from_secs(30));
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
