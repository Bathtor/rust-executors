// Copyright 2017 Lars Kroll. See the LICENSE
// file at the top-level directory of this distribution.
//
// Licensed under the MIT license
// <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed
// except according to those terms.
#![cfg_attr(feature = "nightly", feature(test))]
#![allow(unused_parens)]
extern crate executors;
extern crate synchronoise;
extern crate time;

#[cfg(feature = "nightly")]
extern crate test;

#[macro_use]
extern crate clap;

pub mod experiment;
pub mod latency_experiment;
mod stats;

use crate::stats::*;
use clap::{App, Arg, SubCommand};
use executors::crossbeam_channel_pool::ThreadPool as CCExecutor;
use executors::crossbeam_workstealing_pool::ThreadPool as CWSExecutor;
use executors::threadpool_executor::ThreadPoolExecutor as TPExecutor;
use executors::*;
use std::error::Error;
use std::fs::File;
use std::fs::OpenOptions;
use std::io::prelude::*;
use std::path::Path;

fn main() {
    let throughput = SubCommand::with_name("throughput")
        .about("Runs a throughput performance evaluation of all Executor implementations.")
        .arg(
            Arg::with_name("pre-work")
            .long("pre")
            .help("Amount of work (#u64-additions) to perform before spawning the next job (in amplification).")
            .takes_value(true),
        ).arg(
            Arg::with_name("post-work")
            .long("post")
            .help("Amount of work (#u64-additions) to perform after spawning the next job (in amplification).")
            .takes_value(true),
        );

    let latency = SubCommand::with_name("latency")
        .about("Runs a bursty scheduling evaluation of all Executor implementations.")
        .arg(
            Arg::with_name("sleep-time")
            .short("s")
            .help("Time (in ms) to sleep between bursts of work. Can be used to simulate network latency, for example.")
            .takes_value(true),
        );

    let app = App::new("executor-performance")
        .version("v0.1.1")
        .author("Lars Kroll <lkroll@kth.se>")
        .about(
            "Runs performance tests for different Executor implementations.",
        )
        .subcommand(throughput)
        .subcommand(latency)
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

    if let Some(sc_opts) = opts.subcommand_matches("throughput") {
        use crate::experiment::*;
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
        if sc_opts.is_present("pre-work") {
            let n = value_t!(sc_opts, "pre-work", u64).unwrap();
            settings.set_pre_work(n);
        }
        if sc_opts.is_present("post-work") {
            let n = value_t!(sc_opts, "post-work", u64).unwrap();
            settings.set_post_work(n);
        }
        println!(
            "Running with settings:\n{:?}\nThe total number of messages is: {}",
            settings,
            settings.total_messages()
        );

        if opts.is_present("skip-threadpool-executor") {
            println!("Skipping threadpool_executor.");
            run_throughput_experiments(&settings, f, true, format!("{}", settings.num_threads()));
        } else {
            run_throughput_experiments(&settings, f, false, format!("{}", settings.num_threads()));
        }
    } else if let Some(sc_opts) = opts.subcommand_matches("latency") {
        use crate::latency_experiment::*;
        use std::time::Duration;

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
        if sc_opts.is_present("sleep-time") {
            let t = value_t!(sc_opts, "sleep-time", u64).unwrap();
            settings.set_sleep_time(Duration::from_millis(t));
        }

        println!(
            "Running with settings:\n{:?}\nThe total number of messages is: {}",
            settings,
            settings.total_messages()
        );

        if opts.is_present("skip-threadpool-executor") {
            println!("Skipping threadpool_executor.");
            run_latency_experiments(&settings, f, true, format!("{}", settings.num_threads()));
        } else {
            run_latency_experiments(&settings, f, false, format!("{}", settings.num_threads()));
        }
    } else {
        eprintln!("ERROR: Invalid test specification!");
        std::process::exit(1);
    }
}

const NS_TO_S: f64 = 1.0 / (1000.0 * 1000.0 * 1000.0);
const NS_TO_MS: f64 = 1.0 / (1000.0 * 1000.0);

fn run_throughput_experiments(
    settings: &crate::experiment::ExperimentSettings,
    out: Option<File>,
    skip_tpe: bool,
    extra_infos: String,
) {
    use crate::experiment::*;

    let total_messages = settings.total_messages() as f64;

    let tpe_res = if !skip_tpe {
        let exp = Experiment::new(
            |nt| TPExecutor::new(nt),
            settings,
            String::from("threadpool_executor"),
        );
        run_throughput_experiment(exp, total_messages)
    } else {
        0.0f64
    };
    let cc_res = {
        let exp = Experiment::new(
            |nt| CCExecutor::new(nt),
            settings,
            String::from("crossbeam_channel_pool"),
        );
        run_throughput_experiment(exp, total_messages)
    };
    let cws_res = {
        let exp = Experiment::new(
            |nt| CWSExecutor::new(nt, parker::small()),
            settings,
            String::from("crossbeam_workstealing_pool"),
        );
        run_throughput_experiment(exp, total_messages)
    };
    match out {
        Some(mut f) => {
            let csv = format!(
                "{},{},{},{},{}\n",
                total_messages, extra_infos, tpe_res, cc_res, cws_res
            );
            f.write_all(csv.as_bytes())
                .expect("Output could not be written");
            f.flush().expect("Output could not be flushed");
        }
        None => (),
    }
}

fn run_throughput_experiment<'a, E: Executor + 'static>(
    mut exp: crate::experiment::Experiment<'a, E>,
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

fn run_latency_experiments(
    settings: &crate::latency_experiment::ExperimentSettings,
    out: Option<File>,
    skip_tpe: bool,
    extra_infos: String,
) {
    use crate::latency_experiment::*;

    let total_messages = settings.total_messages();

    let tpe_res = if !skip_tpe {
        let exp = Experiment::new(
            |nt| TPExecutor::new(nt),
            settings,
            String::from("threadpool_executor"),
        );
        run_latency_experiment(exp, total_messages)
    } else {
        Stats::new()
    };
    let cc_res = {
        let exp = Experiment::new(
            |nt| CCExecutor::new(nt),
            settings,
            String::from("crossbeam_channel_pool"),
        );
        run_latency_experiment(exp, total_messages)
    };
    let cws_res = {
        let exp = Experiment::new(
            |nt| CWSExecutor::new(nt, parker::small()),
            settings,
            String::from("crossbeam_workstealing_pool"),
        );
        run_latency_experiment(exp, total_messages)
    };
    match out {
        Some(mut f) => {
            let csv = format!(
                "{},{},{},{},{}\n",
                total_messages,
                extra_infos,
                tpe_res.summary(),
                cc_res.summary(),
                cws_res.summary()
            );
            f.write_all(csv.as_bytes())
                .expect("Output could not be written");
            f.flush().expect("Output could not be flushed");
        }
        None => (),
    }
}

fn run_latency_experiment<'a, E: Executor + 'static>(
    mut exp: crate::latency_experiment::Experiment<'a, E>,
    total_messages: u64,
) -> Stats {
    exp.prepare();
    println!("Starting run for {}", exp.label());
    let stats = exp.run();
    println!("Finished run for {}", exp.label());

    println!(
        "Experiment {} took {:.3} ms (±{:.3} ms; RSE {:.3}%) on average for {} messages on each of {} runs.",
        exp.label(),
        stats.sample_mean() * NS_TO_MS,
        stats.absolute_bound() * NS_TO_MS,
        stats.relative_error_mean_percent(),
        total_messages,
        stats.data().len(),
    );
    exp.cleanup();
    stats
}
