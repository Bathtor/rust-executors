#!/bin/bash
set +e

echo "%%%%%% Testing default features %%%%%%"
cargo test
echo "%%%%%% Finished testing default features %%%%%%"

echo "%%%%%% Testing feature ws-no-park %%%%%%"
cargo test --features ws-no-park
echo "%%%%%% Finished testing feature ws-no-park %%%%%%"

echo "%%%%%% Testing feature !ws-timed-fairness %%%%%%"
cargo test --no-default-features  --features threadpool-exec,cb-channel-exec,workstealing-exec,defaults
echo "%%%%%% Finished testing feature !ws-timed-fairness %%%%%%"
