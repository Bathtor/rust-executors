// Copyright 2017 Lars Kroll. See the LICENSE
// file at the top-level directory of this distribution.
//
// Licensed under the MIT license
// <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed
// except according to those terms.
#![allow(unused_parens)]
extern crate executors;
extern crate synchronoise;
extern crate time;
extern crate test;
#[macro_use]
extern crate clap;

pub mod experiment;

use experiment::*;
use executors::*;
use executors::threadpool_executor::ThreadPoolExecutor as TPExecutor;
use executors::crossbeam_channel_pool::ThreadPool as CCExecutor;
use executors::crossbeam_workstealing_pool::ThreadPool as CWSExecutor;
use clap::{Arg, App};
use std::error::Error;
use std::fs::File;
use std::io::prelude::*;
use std::path::Path;
use std::fs::OpenOptions;

fn main() {
    let app = App::new("executor-performance")
        .version("v0.1.0")
        .author("Lars Kroll <lkroll@kth.se>")
        .about(
            "Runs performance tests for different Executor implementations.",
        )
        .arg(
            Arg::with_name("num-threads")
                .short("t")
                .help("Number of worker threads for pooled Executors.")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("in-parallelism")
                .short("p")
                .help(
                    "Number of external 'in'-threads scheduling on the Executor.",
                )
                .takes_value(true),
        )
        .arg(
            Arg::with_name("num-messages")
                .short("m")
                .help("Number of job messages per 'in'-thread to schedule.")
                .takes_value(true),
        ).arg(
            Arg::with_name("message-amplification")
                .short("a")
                .help("Number of additional job messages spawned from a worker thread for each original job messages.")
                .takes_value(true),
        ).arg(
            Arg::with_name("pre-work")
            .long("pre")
            .help("Amount of work (#u64-additions) to perform before spawning the next job (in amplification).")
            .takes_value(true),
        ).arg(
            Arg::with_name("post-work")
            .long("post")
            .help("Amount of work (#u64-additions) to perform after spawning the next job (in amplification).")
            .takes_value(true),
        ).arg(
            Arg::with_name("skip-threadpool-executor")
            .long("skip-tpe")
            .help("Skip the test for the threadpool_executor as it can be VERY slow with multiple worker threads.")
            .takes_value(false)            
        ).arg(
            Arg::with_name("csv-file")
            .short("o")
            .long("output-csv")
            .help("Output results into the given CSV file as '<total #messages>,<threadpool result>,<cb-channel result>,<workstealing result>'")
            .takes_value(true)            
        );
    let opts = app.get_matches();
    let mut settings = ExperimentSettings::default();
    if opts.is_present("num-threads") {
        let n = value_t!(opts, "num-threads", usize).unwrap();
        settings.set_num_threads(n);
    }
    if opts.is_present("in-parallelism") {
        let n = value_t!(opts, "in-parallelism", usize).unwrap();
        settings.set_in_parallelism(n);
    }
    if opts.is_present("num-messages") {
        let n = value_t!(opts, "num-messages", u64).unwrap();
        settings.set_num_messages(n);
    }
    if opts.is_present("message-amplification") {
        let n = value_t!(opts, "message-amplification", u64).unwrap();
        settings.set_message_amplification(n);
    }
    if opts.is_present("pre-work") {
        let n = value_t!(opts, "pre-work", u64).unwrap();
        settings.set_pre_work(n);
    }
    if opts.is_present("post-work") {
        let n = value_t!(opts, "post-work", u64).unwrap();
        settings.set_post_work(n);
    }
    println!(
        "Running with settings:\n{:?}\nThe total number of messages is: {}",
        settings,
        settings.total_messages()
    );
    let f: Option<File> = if opts.is_present("csv-file") {
        let path = Path::new(opts.value_of("csv-file").unwrap());
        let display = path.display();
        let file = match OpenOptions::new().append(true).create(true).open(&path) {
            Err(why) => panic!("couldn't open {}: {}", display, why.description()),
            Ok(file) => file,
        };
        println!("Results will be added to {}", display);
        Some(file)
    } else {
        None
    };
    if opts.is_present("skip-threadpool-executor") {
        println!("Skipping threadpool_executor.");
        run_experiments(&settings, f, true, format!("{}", settings.num_threads()));
    } else {
        run_experiments(&settings, f, false, format!("{}", settings.num_threads()));
    }
}

const NS_TO_S: f64 = 1.0 / (1000.0 * 1000.0 * 1000.0);

fn run_experiments(settings: &ExperimentSettings, out: Option<File>, skip_tpe: bool, extra_infos: String) {
    let total_messages = settings.total_messages() as f64;

    let tpe_res = if !skip_tpe {
        let exp = Experiment::new(
            |nt| TPExecutor::new(nt),
            settings,
            String::from("threadpool_executor"),
        );
        run_experiment(exp, total_messages)
    } else {
        0.0f64
    };
    let cc_res = {
        let exp = Experiment::new(
            |nt| CCExecutor::new(nt),
            settings,
            String::from("crossbeam_channel_pool"),
        );
        run_experiment(exp, total_messages)
    };
    let cws_res = {
        let exp = Experiment::new(
            |nt| CWSExecutor::new(nt),
            settings,
            String::from("crossbeam_workstealing_pool"),
        );
        run_experiment(exp, total_messages)
    };
    match out {
        Some(mut f) => {
            let csv = format!("{},{},{},{},{}\n", total_messages,extra_infos,tpe_res, cc_res, cws_res);
            f.write_all(csv.as_bytes()).expect("Output could not be written");
            f.flush().expect("Output could not be flushed");
        }
        None => ()
    }
}

fn run_experiment<'a, E: Executor + 'static>(
    mut exp: Experiment<'a, E>,
    total_messages: f64,
) -> f64 {
    exp.prepare();
    println!("Starting run for {}", exp.label());
    let startt = time::precise_time_ns();
    exp.run();
    let endt = time::precise_time_ns();
    println!("Finished run for {}", exp.label());
    let difft = (endt - startt) as f64;
    let diffts = difft * NS_TO_S;
    let events_per_second = total_messages / diffts;
    println!(
        "Experiment {} ran {:.1}schedulings/s",
        exp.label(),
        events_per_second
    );
    exp.cleanup();
    events_per_second
}
