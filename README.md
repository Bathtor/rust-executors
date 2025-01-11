Executors
=========
A library with high-performace task executors for Rust.

[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/Bathtor/rust-executors)
[![Cargo](https://img.shields.io/crates/v/executors.svg)](https://crates.io/crates/executors)
[![Documentation](https://docs.rs/executors/badge.svg)](https://docs.rs/executors)
[![Codecov](https://codecov.io/gh/Bathtor/rust-executors/branch/master/graph/badge.svg)](https://codecov.io/gh/Bathtor/rust-executors)
[![CI](https://github.com/Bathtor/rust-executors/actions/workflows/ci.yml/badge.svg)](https://github.com/Bathtor/rust-executors/actions)

## Usage

Add this to your `Cargo.toml`:

```toml
[dependencies]
executors = "0.10"
```

You can use, for example, the [crossbeam_workstealing_pool](https://docs.rs/executors/latest/executors/crossbeam_workstealing_pool/index.html) to schedule a number `n_jobs` over a number `n_workers` threads, and collect the results via an `mpsc::channel`.

```rust
use executors::*;
use executors::crossbeam_workstealing_pool;
use std::sync::mpsc::channel;

let n_workers = 4;
let n_jobs = 8;
let pool = crossbeam_workstealing_pool::small_pool(n_workers);

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

In general, [crossbeam_workstealing_pool](https://docs.rs/executors/latest/executors/crossbeam_workstealing_pool/index.html) works best for workloads where the tasks on the worker threads spawn more and more tasks. If all tasks a spawned from a single thread that isn't part of the threadpool, then [crossbeam_channel_pool](https://docs.rs/executors/latest/executors/crossbeam_channel_pool/index.html) tends to perform best.

If you don't know what hardware your code is going to run on, use the [crossbeam_workstealing_pool](https://docs.rs/executors/latest/executors/crossbeam_workstealing_pool/index.html). It tends to perform best on all the hardware I have tested (which is pretty much Intel processors like i7 and Xeon).

If you *absolutely need* low response time to bursty workloads, you can compile the crate with the `ws-no-park` feature, which prevents the workers in the [crossbeam_workstealing_pool](https://docs.rs/executors/latest/executors/crossbeam_workstealing_pool/index.html) from parking their threads, when all the task-queues are temporarily empty. This will, of course, not play well with other tasks running on the same system, although the threads are still yielded to the OS scheduler in between queue checks. See latency results below to get an idea of the performance impact of this feature.

## Core Affinity

You can enable support for pinning pool threads to particular CPU cores via the `"thread-pinning"` feature. It will then pin by default as many threads as there are core ids. If you are asking for more threads than that, the rest will be created unpinned. You can also assign only a subset of your core ids to a thread pool by using the `with_affinity(...)` instead of the `new(...)` function.

## Some Numbers

The following are some example results from my desktop machine (Intel i7-4770 @ 3.40Ghz Quad-Core with HT (8 logical cores) with 16GB of RAM).
*Note* that they are all from a single run and thus not particularly scientific and subject to whatever else was going on on my system during the run.

Implementation abbreviations:
- **TP**: `threadpool_executor` [docs](https://docs.rs/executors/latest/executors/threadpool_executor/index.html)
- **CBCP**: `crossbeam_channel_pool` [docs](https://docs.rs/executors/latest/executors/crossbeam_channel_pool/index.html)
- **CBWP**: `crossbeam_workstealing_pool` [docs](https://docs.rs/executors/0.4.4/executors/crossbeam_workstealing_pool/index.html)

### Throughput

These experiments measure the throughput of a fully loaded executor implementation.

#### Low Amplification

Testing command (where `$NUM_THREADS` corresponds to #Threads in the table below): 
```bash
target/release/executor-performance -t $NUM_THREADS -p 2 -m 10000000 -a 1 throughput --pre 10000 --post 10000
```
This corresponds to a IO-handling-server-style workload, where the vast majority of tasks are coming in via the external queue, and only very few are spawned from within the executor's threads.

(Units are in *mio tasks/s*)

| #Threads | TP   | CBCP | CBWP (default-features) | CBWP (`ws-no-park`) | CBWP (no `ws-timed-fairness`) |
|---------:|-----:|-----:|------------------------:|--------------------:|------------------------------:|
| 1        | 2.4  | 3.1  | 3.4                     | 3.5                 | 3.5                           |
| 2        | 1.2  | 2.8  | 3.3                     | 3.4                 | 3.4                           |
| 3        | 1.5  | 3.1  | 4.0                     | 4.0                 | 4.0                           |
| 4        | 1.2  | 3.6  | 4.4                     | 4.3                 | 4.3                           |
| 5        | 1.3  | 3.6  | 4.2                     | 4.2                 | 4.2                           |
| 6        | 1.2  | 3.7  | 4.2                     | 4.2                 | 4.1                           |
| 7        | 1.1  | 3.7  | 3.7                     | 4.1                 | 3.3                           |
| 8        | 1.1  | 3.4  | 2.4                     | 4.0                 | 1.8                           |
| 9        | 1.1  | 3.4  | 1.5                     | 3.6                 | 1.6                           |

#### High Amplification

Testing command (where `$NUM_THREADS` corresponds to #Threads in the table below): 
```bash
target/release/executor-performance -t $NUM_THREADS -p 2 -m 1000 -a 50000 throughput --pre 10000 --post 10000
```
This corresponds to a message-passing-style (or fork-join) workload, where the vast majority of tasks are spawned from within the executor's threads and only relatively few come in via the external queue.

(Units are in *mio tasks/s*)

| #Threads | TP   | CBCP | CBWP (default-features) | CBWP (`ws-no-park`) | CBWP (no `ws-timed-fairness`) |
|---------:|-----:|-----:|------------------------:|--------------------:|------------------------------:|
| 1        | 6.8  | 9.4  | 10.9                    | 10.9                | 10.9                          |
| 2        | 2.9  | 5.6  | 7.4                     | 6.8                 | 7.4                           |
| 3        | 3.3  | 6.7  | 9.0                     | 8.3                 | 8.5                           |
| 4        | 2.9  | 7.4  | 10.0                    | 10.6                | 10.3                          |
| 5        | 2.8  | 8.0  | 11.1                    | 12.4                | 12.0                          |
| 6        | 2.6  | 8.8  | 11.8                    | 12.3                | 12.6                          |
| 7        | 2.5  | 9.0  | 12.8                    | 12.7                | 12.5                          |
| 8        | 2.5  | 8.8  | 14.1                    | 12.9                | 13.2                          |
| 9        | 2.4  | 8.9  | 13.4                    | 12.7                | 12.7                          |


### Latency

These experiments measure the start-to-finish time (latency) of running a bursty workload, while sleeping in between runs. This is what typically happens in a network server, that isn't fully loaded, but waiting for a message and then running a series of tasks based on the message, before responding and waiting for the next.

These experiments report averages collected over multiple measurements with RSE<10%.

#### Small Bursts / Low Amplification

Testing command: 
```bash
target/release/executor-performance -t 6 -p 1 -m 100 -a 1 latency -s 100
```
This corresponds to a very small internal workload in response to every external message. Sleep time between bursts is 100ms.

| Implementation          | Mean     | Max/Min   |
|:------------------------|---------:|----------:|
| **TP**                  | 0.539 ms | ±0.321 ms |
| **CBCP**                | 0.442 ms | ±0.225 ms |
| **CBWP**                | 0.372 ms | ±0.218 ms |
| **CBWP** (`ws-no-park`) | 0.072 ms | ±0.182 ms |

#### Medium Bursts / High Amplification

Testing command: 
```bash
target/release/executor-performance -t 6 -p 1 -m 100 -a 100 latency -s 100
```
This corresponds to a significant internal workload in response to every external message. Sleep time between bursts is 100ms.

| Implementation          | Mean     | Max/Min   |
|:------------------------|---------:|----------:|
| **TP**                  | 9.062 ms | ±3.485 ms |
| **CBCP**                | 4.215 ms | ±2.784 ms |
| **CBWP**                | 3.941 ms | ±2.241 ms |
| **CBWP** (`ws-no-park`) | 0.343 ms | ±3.205 ms |


#### Large Bursts / Very High Amplification

Testing command: 
```bash
target/release/executor-performance -t 6 -p 1 -m 100 -a 10000 latency -s 100
```
This corresponds to a very large internal workload in response to every external message. Sleep time between bursts is 100ms.

| Implementation          | Mean       | Max/Min    |
|:------------------------|-----------:|-----------:|
| **TP**                  | 381.023 ms | ±3.249 ms  |
| **CBCP**                | 124.666 ms | ±2.315 ms  |
| **CBWP**                | 79.532 ms  | ±41.626 ms |
| **CBWP** (`ws-no-park`) | 51.867 ms  | ±43.893 ms |

## License

Licensed under the terms of the MIT license.

See [LICENSE](LICENSE) for details.
