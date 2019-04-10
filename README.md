Executors
=========
A library with high-performace task executors for Rust.

[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/Bathtor/rust-executors)
[![Cargo](https://img.shields.io/crates/v/executors.svg)](https://crates.io/crates/executors)
[![Documentation](https://docs.rs/executors/badge.svg)](https://docs.rs/executors)
[![Build Status](https://travis-ci.org/Bathtor/rust-executors.svg?branch=master)](https://travis-ci.org/Bathtor/rust-executors)

## Usage

Add this to your `Cargo.toml`:

```toml
[dependencies]
executors = "0.4"
```

You can use, for example, the [crossbeam_workstealing_pool](https://docs.rs/executors/latest/executors/crossbeam_workstealing_pool/index.html) to schedule a number `n_jobs` over a number `n_workers` threads, and collect the results via an `mpsc::channel`.

```rust
use executors::*;
use executors::crossbeam_workstealing_pool::ThreadPool;
use std::sync::mpsc::channel;

let n_workers = 4;
let n_jobs = 8;
let pool = ThreadPool::new(n_workers);

let (tx, rx) = channel();
for _ in 0..n_jobs {
    let tx = tx.clone();
    pool.execute(move|| {
        tx.send(1).expect("channel will be there waiting for the pool");
    });
}

assert_eq!(rx.iter().take(n_jobs).fold(0, |a, b| a + b), 8);
```

## Rust Version

Requires at least Rust `1.26`, due to [crossbeam-channel](https://github.com/crossbeam-rs/crossbeam-channel) requirements.

## Deciding on an Implementation

To select an `Executor` implementation, it is best to test the exact requirements on the target hardware.
The crate `executor-performance` provides a performance testing suite for the provided implementations. To use it *clone* this repository, run `cargo build --release`, and then check `target/release/executor-performance --help` to see the available options. There is also a small [script](executor-performance/threadinc.sh) to test thread-scaling with some reasonable default options.

In general, [crossbeam_workstealing_pool](https://docs.rs/executors/latest/executors/crossbeam_workstealing_pool/index.html) works best for workloads where the tasks on the worker threads spawn more and more tasks. If all tasks a spawned from a single thread that isn't part of the threadpool, then the [threadpool_executor](https://docs.rs/executors/latest/executors/threadpool_executor/index.html) tends perform best with single worker and [crossbeam_channel_pool](https://docs.rs/executors/latest/executors/crossbeam_channel_pool/index.html) performs best for a larger number of workers.

If you don't know what hardware your code is going to run on, use the [crossbeam_workstealing_pool](https://docs.rs/executors/latest/executors/crossbeam_workstealing_pool/index.html). It tends to perform best on all the hardware I have tested (which is pretty much Intel processors like i7 and Xeon).

## License

Licensed under the terms of MIT license.

See [LICENSE](LICENSE) for details.
