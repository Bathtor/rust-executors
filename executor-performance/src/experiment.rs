// Copyright 2017 Lars Kroll. See the LICENSE
// file at the top-level directory of this distribution.
//
// Licensed under the MIT license
// <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed
// except according to those terms.

use super::*;
use std::default::Default;
use std::sync::Arc;
use std::thread;
use synchronoise::CountdownEvent;

#[cfg(feature = "nightly")]
use test::black_box;

#[cfg(not(feature = "nightly"))]
fn black_box<T>(dummy: T) -> T {
    dummy
}

#[derive(Clone, Debug)]
pub struct ExperimentSettings {
    num_threads: usize,
    in_parallelism: usize,
    num_messages: u64,
    message_amplification: u64,
    pre_work: u64,
    post_work: u64,
}

impl ExperimentSettings {
    pub fn new() -> ExperimentSettings {
        ExperimentSettings {
            num_threads: 0,
            in_parallelism: 0,
            num_messages: 0,
            message_amplification: 0,
            pre_work: 0,
            post_work: 0,
        }
    }

    pub fn total_messages(&self) -> u64 {
        (self.in_parallelism as u64) * self.num_messages * self.message_amplification
    }

    pub fn num_threads(&self) -> usize {
        self.num_threads
    }

    pub fn set_num_threads(&mut self, n: usize) -> &mut Self {
        self.num_threads = n;
        self
    }

    pub fn in_parallelism(&self) -> usize {
        self.in_parallelism
    }

    pub fn set_in_parallelism(&mut self, n: usize) -> &mut Self {
        self.in_parallelism = n;
        self
    }

    pub fn num_messages(&self) -> u64 {
        self.num_messages
    }

    pub fn set_num_messages(&mut self, n: u64) -> &mut Self {
        self.num_messages = n;
        self
    }

    pub fn message_amplification(&self) -> u64 {
        self.message_amplification
    }

    pub fn set_message_amplification(&mut self, n: u64) -> &mut Self {
        self.message_amplification = n;
        self
    }

    pub fn pre_work(&self) -> u64 {
        self.pre_work
    }

    pub fn set_pre_work(&mut self, n: u64) -> &mut Self {
        self.pre_work = n;
        self
    }

    pub fn post_work(&self) -> u64 {
        self.post_work
    }

    pub fn set_post_work(&mut self, n: u64) -> &mut Self {
        self.post_work = n;
        self
    }
}

impl Default for ExperimentSettings {
    fn default() -> ExperimentSettings {
        ExperimentSettings {
            num_threads: 1,
            in_parallelism: 1,
            num_messages: 1_000_000,
            message_amplification: 1,
            pre_work: 0,
            post_work: 0,
        }
    }
}

pub struct Experiment<'a, E: Executor + 'static> {
    exec: E,
    settings: &'a ExperimentSettings,
    latch: Arc<CountdownEvent>,
    label: String,
}

impl<'a, E: Executor + 'static> Experiment<'a, E> {
    pub fn new<F>(exec_func: F, settings: &'a ExperimentSettings, label: String) -> Self
    where
        F: FnOnce(usize) -> E,
    {
        Experiment {
            exec: exec_func(settings.num_threads()),
            settings,
            latch: Arc::new(CountdownEvent::new(settings.in_parallelism())),
            label,
        }
    }

    pub fn label(&self) -> &String {
        &self.label
    }

    pub fn prepare(&mut self) {}

    pub fn run(&mut self) {
        let pre_work = self.settings.pre_work();
        let post_work = self.settings.post_work();
        for _ in 0..self.settings.in_parallelism() {
            let thread_exec = self.exec.clone();
            let msgs = self.settings.num_messages() - 1;
            let amplification = self.settings.message_amplification();
            let finisher = Finisher::new(self.latch.clone());
            thread::spawn(move || {
                for _ in 0..msgs {
                    let amp_exec = thread_exec.clone();
                    thread_exec
                        .execute(move || amplify(amp_exec, pre_work, post_work, amplification));
                }
                let amp_exec = thread_exec.clone();
                thread_exec.execute(move || {
                    amplify_finish(amp_exec, pre_work, post_work, finisher, amplification)
                });
            });
        }
        self.latch.wait();
    }

    pub fn cleanup(self) {
        self.exec
            .shutdown()
            .expect("Executor didn't shut down properly");
    }
}

#[inline(never)]
fn do_work(n: u64) -> u64 {
    //println!("Doing work");
    let mut sum = 0u64;
    for i in 0..n {
        sum += i;
    }
    //println!("Work was {}", sum);
    sum
}

fn amplify<E: Executor + 'static>(exec: E, pre_work: u64, post_work: u64, remaining: u64) {
    black_box(do_work(pre_work));
    if (remaining > 0) {
        let amp_exec = exec.clone();
        exec.execute(move || amplify(amp_exec, pre_work, post_work, remaining - 1));
    }
    black_box(do_work(post_work));
}

fn amplify_finish<E: Executor + 'static>(
    exec: E,
    pre_work: u64,
    post_work: u64,
    finisher: Finisher,
    remaining: u64,
) {
    black_box(do_work(pre_work));
    if (remaining > 0) {
        let amp_exec = exec.clone();
        exec.execute(move || {
            amplify_finish(amp_exec, pre_work, post_work, finisher, remaining - 1)
        });
        black_box(do_work(post_work));
    } else {
        black_box(do_work(post_work));
        finisher.execute();
    }
}

#[derive(Clone)]
struct Finisher {
    latch: Arc<CountdownEvent>,
}

impl Finisher {
    fn new(latch: Arc<CountdownEvent>) -> Finisher {
        Finisher { latch }
    }
    fn execute(self) {
        self.latch.decrement().expect("Latch didn't decrement");
    }
}
