// Copyright 2017 Lars Kroll. See the LICENSE
// file at the top-level directory of this distribution.
//
// Licensed under the MIT license
// <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed
// except according to those terms.
#![doc(html_root_url = "https://docs.rs/executors/0.1.0")]

extern crate crossbeam_channel;
extern crate synchronoise;
#[macro_use]
extern crate log;

pub mod common;
pub mod run_now;
pub mod crossbeam_channel_pool;

pub use common::Executor;
use common::ignore;
use synchronoise::CountdownEvent;

#[cfg(test)]
mod tests {
    #[test]
    fn it_works() {
        assert_eq!(2 + 2, 4);
    }
}
