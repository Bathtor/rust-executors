// Copyright 2017 Lars Kroll. See the LICENSE
// file at the top-level directory of this distribution.
//
// Licensed under the MIT license
// <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed
// except according to those terms.

use super::*;
use crate::stats::Stats;
use std::default::Default;
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use synchronoise::CountdownEvent;

#[cfg(feature = "nightly")]
use test::black_box;

#[cfg(not(feature = "nightly"))]
fn black_box<T>(dummy: T) -> T {
    dummy
}

const PRE_WORK: u64 = 10;
const POST_WORK: u64 = 10;

#[derive(Clone, Debug)]
pub struct ExperimentSettings {
    num_threads: usize,
    in_parallelism: usize,
    num_messages: u64,
    message_amplification: u64,
    sleep_time: Duration,
}

impl ExperimentSettings {
    pub fn new() -> ExperimentSettings {
        ExperimentSettings {
            num_threads: 0,
            in_parallelism: 0,
            num_messages: 0,
            message_amplification: 0,
            sleep_time: Duration::from_millis(1),
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

    pub fn sleep_time(&self) -> &Duration {
        &self.sleep_time
    }

    pub fn set_sleep_time(&mut self, t: Duration) -> &mut Self {
        self.sleep_time = t;
        self
    }
}

impl Default for ExperimentSettings {
    fn default() -> ExperimentSettings {
        ExperimentSettings {
            num_threads: 1,
            in_parallelism: 1,
            num_messages: 1_000,
            message_amplification: 1,
            sleep_time: Duration::from_millis(50),
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

    pub fn run(&mut self) -> Stats {
        let mut stats: Stats = Stats::new();

        while !stats.is_done() {
            let startt = time::precise_time_ns();
            self.execute_run();
            let endt = time::precise_time_ns();
            let difft = (endt - startt) as f64;
            stats.push(difft);
            std::thread::sleep(*self.settings.sleep_time());
        }
        stats
    }

    fn execute_run(&mut self) {
        for _ in 0..self.settings.in_parallelism() {
            let thread_exec = self.exec.clone();
            let msgs = self.settings.num_messages() - 1;
            let amplification = self.settings.message_amplification();
            let finisher = Finisher::new(self.latch.clone());
            thread::spawn(move || {
                for _ in 0..msgs {
                    let amp_exec = thread_exec.clone();
                    thread_exec.execute(move || amplify(amp_exec, amplification));
                }
                let amp_exec = thread_exec.clone();
                thread_exec.execute(move || amplify_finish(amp_exec, finisher, amplification));
            });
        }
        self.latch.wait();
        self.latch = Arc::new(CountdownEvent::new(self.settings.in_parallelism()));
        // reset for next run
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

fn amplify<E: Executor + 'static>(exec: E, remaining: u64) {
    black_box(do_work(POST_WORK));
    if (remaining > 0) {
        let amp_exec = exec.clone();
        exec.execute(move || amplify(amp_exec, remaining - 1));
    }
    black_box(do_work(PRE_WORK));
}

fn amplify_finish<E: Executor + 'static>(exec: E, finisher: Finisher, remaining: u64) {
    black_box(do_work(PRE_WORK));
    if (remaining > 0) {
        let amp_exec = exec.clone();
        exec.execute(move || amplify_finish(amp_exec, finisher, remaining - 1));
        black_box(do_work(POST_WORK));
    } else {
        black_box(do_work(POST_WORK));
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
