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
executors = "0.2"
```

and this to your crate root:

```rust
extern crate executors;
```

## Rust Version

Requires at least Rust `1.22.0`, as [crossbeam-deque](https://github.com/crossbeam-rs/crossbeam-deque) expects the `ord_max_min` feature to be stable.

## Deciding on an Implementation

To select an `Executor` implementation, it is best to test the exact requirements on the target hardware.
The crate [executor-performance](tree/master/executor-performance) provides a performance testing suite for the provided implementations. To use it *clone* this repository, run `cargo build --release`, and then check `target/release/executor-performance --help` to see the available options. There is also a small [script](blob/master/executor-performance/threadinc.sh) to test thread-scaling with some reasonable default options.

If you don't know what hardware your code is going to run on, use the [crossbeam_workstealing_pool](https://docs.rs/executors/0.2.0/executors/crossbeam_workstealing_pool/index.html). It tends to perform best on all the hardware I have tested (which is pretty much Intel processors like i7 and Xeon).

## License

Licensed under the terms of MIT license.

See [LICENSE](LICENSE) for details.