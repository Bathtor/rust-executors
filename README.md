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

## License

Licensed under the terms of MIT license.

See [LICENSE](LICENSE) for details.