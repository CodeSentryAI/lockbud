#!/bin/env bash

# Test script for stable-mir lock deadlock detection
#
# Usage: ./test-lock-detection.sh [test_case] [log_level]
# Examples:
#   ./test-lock-detection.sh            # Test all cases (info level)
#   ./test-lock-detection.sh intra      # Test single case (info level)
#   ./test-lock-detection.sh intra debug # Test with debug logging
#
# Log levels: info, debug, trace, warn, error
# Debug mode shows detailed type parsing information

DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" && pwd )"

# List of test cases
TEST_CASES=(
    "intra"
    "inter"
    "conflict"
    "conflict-inter"
    "lock-closure"
    "static-ref"
    "tikv-wrapper"
)

if [ ! -z "$1" ]; then
    TEST_CASES=("$1")
fi

# Set log level (default: info)
LOG_LEVEL="info"
if [ ! -z "$2" ]; then
    LOG_LEVEL="$2"
fi

# Build stable-demo
echo "Building stable-demo..."
cargo build --bin stable-demo

# Set up the environment
export RUSTC=${PWD}/target/debug/stable-demo
export RUST_BACKTRACE=full
export LOCKBUD_LOG=$LOG_LEVEL

echo ""
echo "=== Lock Deadlock Detection Test ==="
echo ""

for test_case in "${TEST_CASES[@]}"; do
    TEST_DIR="toys/$test_case"

    if [ ! -d "$TEST_DIR" ]; then
        echo "⚠ Skipping $test_case (directory not found)"
        echo ""
        continue
    fi

    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "Testing: $test_case"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

    pushd "$TEST_DIR" > /dev/null
    cargo clean > /dev/null 2>&1

    # Run analysis and capture all output
    cargo check 2>&1 | tee /tmp/lock-test-output.txt > /dev/null

    # Show log output if log level is debug or trace
    if [ "$LOG_LEVEL" = "debug" ] || [ "$LOG_LEVEL" = "trace" ]; then
        echo ""
        echo "🐛 DEBUG OUTPUT (showing first 50 lines):"
        echo "────────────────────────────────────────────────────"
        grep -A 3 "=== DEBUG LockGuardTy::from_ty ===" /tmp/lock-test-output.txt | head -50 || echo "No debug output found"
        echo "────────────────────────────────────────────────────"
        echo ""
    fi

    # Show lock type analysis results
    echo "📊 LOCK TYPE ANALYSIS:"
    echo "────────────────────────────────────────────────────"
    grep -A 20 "=== Lock Type Analysis ===" /tmp/lock-test-output.txt | head -25 || echo "No lock type analysis found"
    echo "────────────────────────────────────────────────────"
    echo ""

    # Show lock detection results
    echo "🔒 LOCK DETECTION RESULTS:"
    echo "────────────────────────────────────────────────────"
    # Extract the deadlock detection section and show up to 200 lines
    # This should be enough for most test cases
    grep -A 200 "=== Lock Deadlock Detection ===" /tmp/lock-test-output.txt | head -200 || echo "No lock detection output found"
    echo "────────────────────────────────────────────────────"
    echo ""

    # Extract deadlock count
    DEADLOCK_COUNT=$(grep "Total potential deadlocks found" /tmp/lock-test-output.txt 2>/dev/null | grep -o "[0-9]*" || echo "0")

    if [ "$DEADLOCK_COUNT" -gt 0 ]; then
        echo "✓ Found $DEADLOCK_COUNT potential deadlock(s)"
    else
        echo "✗ No deadlocks detected"
    fi

    popd > /dev/null
    echo ""
done

echo "=== Test Complete ==="
