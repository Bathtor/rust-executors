// Copyright 2017 Lars Kroll. See the LICENSE
// file at the top-level directory of this distribution.
//
// Licensed under the MIT license
// <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed
// except according to those terms.
#![doc(html_root_url = "https://docs.rs/executors/0.4.3")]
#![allow(unused_parens)]

//! This crate provides a number of task executors all implementing the
//! [`Executor`](common/trait.Executor.html) trait.
//!
//! General examples can be found in the [`Executor`](common/trait.Executor.html) trait
//! documentation, and implementation specific examples with each implementation module.

#[macro_use]
extern crate log;

pub mod common;
pub mod bichannel;
pub mod run_now;
#[cfg(feature = "cb-channel-exec")]
pub mod crossbeam_channel_pool;
#[cfg(feature = "workstealing-exec")]
pub mod crossbeam_workstealing_pool;
#[cfg(feature = "threadpool-exec")]
pub mod threadpool_executor;
mod timeconstants;

pub use crate::common::Executor;
use crate::common::ignore;

//use bichannel::*;
use synchronoise::CountdownEvent;

#[cfg(test)]
pub(crate) mod tests {
	use super::*;
	use std::sync::Arc;
	use std::time::Duration;

	pub const N_DEPTH: usize = 32;
	pub const N_WIDTH: usize = 8;

	pub fn test_debug<E>(exec: &E, label: &str) where E: Executor+std::fmt::Debug {
		println!("Debug output for {}: {:?}", label, exec);
	}

	pub fn test_defaults<E>(label: &str) where E: Executor+std::default::Default+'static {
		let pool = E::default();

		let latch = Arc::new(CountdownEvent::new(N_DEPTH * N_WIDTH));
        for _ in 0..N_WIDTH {
        	let pool2 = pool.clone();
        	let latch2 = latch.clone();
        	pool.execute(move || {
        		do_step(latch2, pool2, N_DEPTH);
        	});
        }
        let res = latch.wait_timeout(Duration::from_secs(5));
        assert_eq!(res, 0);
        pool.shutdown().unwrap_or_else(|e| error!("Error during pool shutdown {:?} at {}",e, label));
	}

	fn do_step<E>(latch: Arc<CountdownEvent>, pool: E, depth: usize) where E: Executor+'static {
		let new_depth = depth - 1;
		ignore(latch.decrement());
		if (new_depth > 0) {
			let pool2 = pool.clone();
			pool.execute(move || {do_step(latch, pool2, new_depth)})
		}
	}
}
