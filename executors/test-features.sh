#!/bin/bash
set -e
set -o xtrace

echo "%%%%%% Testing default features %%%%%%"
cargo clippy --all-targets -- -D warnings
cargo test -- "$@" --test-threads=1
echo "%%%%%% Finished testing default features %%%%%%"

echo "%%%%%% Testing feature ws-no-park %%%%%%"
cargo clippy --all-targets --features ws-no-park -- -D warnings
cargo test --features ws-no-park -- "$@" --test-threads=1
echo "%%%%%% Finished testing feature ws-no-park %%%%%%"

echo "%%%%%% Testing feature !ws-timed-fairness %%%%%%"
cargo clippy --all-targets --no-default-features  --features threadpool-exec,cb-channel-exec,workstealing-exec,defaults,thread-pinning -- -D warnings
cargo test --no-default-features  --features threadpool-exec,cb-channel-exec,workstealing-exec,defaults,thread-pinning -- "$@" --test-threads=1
echo "%%%%%% Finished testing feature !ws-timed-fairness %%%%%%"

echo "%%%%%% Testing feature thread-pinning %%%%%%"
cargo clippy --all-targets --features thread-pinning -- -D warnings
cargo test --features thread-pinning -- "$@" --test-threads=1
echo "%%%%%% Finished testing feature thread-pinning %%%%%%"

echo "%%%%%% Testing feature numa-aware %%%%%%"
cargo clippy --all-targets --features numa-aware -- -D warnings
cargo test --features numa-aware -- "$@" --test-threads=1
echo "%%%%%% Finished testing feature numa-aware %%%%%%"

echo "%%%%%% Testing feature produce-metrics %%%%%%"
cargo clippy --all-targets --features produce-metrics -- -D warnings
cargo test --features produce-metrics -- "$@" --test-threads=1
echo "%%%%%% Finished testing featureproduce-metrics %%%%%%"
