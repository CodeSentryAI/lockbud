#!/usr/bin/env bash

# Test script for stable-demo callgraph analysis

DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" && pwd )"

if [ -z "$1" ]; then
    echo "No test directory provided"
    echo "Usage: ./test-callgraph.sh DIRNAME"
    exit 1
fi

# Build stable-demo
cargo build --bin stable-demo

# Set up the environment
export RUSTC=${PWD}/target/debug/stable-demo
export RUST_BACKTRACE=full

echo "Testing stable callgraph on $1"

pushd "$1" > /dev/null
cargo clean
# cargo check -Z build-std
cargo check
popd > /dev/null

echo "Test complete!"
