#!/usr/bin/env bash

# Test script for wrapper-lockbud using StableMIR wrapper

DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" && pwd )"

if [ -z "$1" ]; then
    echo "No test directory provided"
    echo "Usage: ./test-wrapper.sh DIRNAME"
    exit 1
fi

# Build wrapper-lockbud
cargo build --bin wrapper-lockbud

# Set up the environment
export RUSTC=${PWD}/target/debug/wrapper-lockbud
export RUST_BACKTRACE=full

echo "Testing wrapper-lockbud on $1"

pushd "$1" > /dev/null
cargo clean
cargo build
popd > /dev/null

echo "Test complete!"
