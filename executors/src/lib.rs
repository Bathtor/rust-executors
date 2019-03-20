// Copyright 2017 Lars Kroll. See the LICENSE
// file at the top-level directory of this distribution.
//
// Licensed under the MIT license
// <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed
// except according to those terms.
#![doc(html_root_url = "https://docs.rs/executors/0.4.2")]
#![allow(unused_parens)]

//! This crate provides a number of task executors all implementing the
//! [`Executor`](common/trait.Executor.html) trait.
//!
//! General examples can be found in the [`Executor`](common/trait.Executor.html) trait
//! documentation, and implementation specific examples with each implementation module.

#[cfg(any(feature = "cb-channel-exec", feature = "workstealing-exec"))]
#[macro_use]
extern crate crossbeam_channel;
#[cfg(feature = "workstealing-exec")]
extern crate crossbeam_deque;

#[cfg(feature = "threadpool-exec")]
extern crate threadpool;
#[cfg(feature = "ws-timed-fairness")]
extern crate time;
#[cfg(feature = "workstealing-exec")]
extern crate rand;
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
mod tests {
}
