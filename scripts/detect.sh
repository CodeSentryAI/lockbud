#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
OBOL_DIR="$(dirname "$SCRIPT_DIR")"
TOOLCHAIN="nightly-2026-02-07"
LOCKBUD_BIN="$OBOL_DIR/target/release/lockbud"

usage() {
    cat <<'EOF'
Usage: detect.sh -k <kind> <project-path> [-- cargo-args...]

Detect concurrency bugs in a Rust project using Lockbud (ULLBC-based).

Arguments:
  -k KIND     Detector kind to run. Must be one of:
                deadlock              Deadlock detection (DoubleLock, ConflictLock, CondvarDeadlock)
                atomicity_violation   Atomicity violations (load-store dependency on Atomics)
                memory                Invalid-free and use-after-free detection
                panic                 Panic location detection (unwrap, expect, panic!, array indexing)
                channel               Channel deadlock and orphan sender detection

Options:
  -b             Build lockbud before running (requires lockbud binary to be pre-built otherwise)
  -r FILE        Custom report file path (default: <project-path>/lockbud_report.json)
  -h, --help     Show this message

Project path must contain a Cargo.toml.

Examples:
  ./scripts/detect.sh -k deadlock /path/to/project
  ./scripts/detect.sh -k channel -b /path/to/project
  ./scripts/detect.sh -k deadlock /path/to/project -- --release --features foo
EOF
    exit 0
}

# Parse args
KIND=""
BUILD=false
REPORT_FILE=""
PROJECT_PATH=""
CARGO_ARGS=()

while [[ $# -gt 0 ]]; do
    case "$1" in
        -k)
            KIND="$2"
            shift 2
            ;;
        -b)
            BUILD=true
            shift
            ;;
        -r)
            REPORT_FILE="$2"
            shift 2
            ;;
        -h|--help)
            usage
            ;;
        --)
            shift
            CARGO_ARGS=("$@")
            break
            ;;
        -*)
            echo "ERROR: Unknown option: $1"
            usage
            ;;
        *)
            if [[ -z "$PROJECT_PATH" ]]; then
                PROJECT_PATH="$1"
                shift
            else
                echo "ERROR: Unexpected argument: $1"
                usage
            fi
            ;;
    esac
done

# Validate kind
VALID_KINDS="deadlock atomicity_violation memory panic channel"
if [[ -z "$KIND" ]]; then
    echo "ERROR: -k <kind> is required. Must be one of: $VALID_KINDS"
    echo "Run with -h for details."
    exit 1
fi

valid=false
for k in $VALID_KINDS; do
    [[ "$KIND" == "$k" ]] && valid=true
done
if ! $valid; then
    echo "ERROR: Unknown detector kind '$KIND'. Must be one of: $VALID_KINDS"
    exit 1
fi

# Validate project path
if [[ -z "$PROJECT_PATH" ]]; then
    echo "ERROR: Project path is required."
    echo "Usage: detect.sh -k <kind> <project-path>"
    exit 1
fi

PROJECT_PATH="$(cd "$PROJECT_PATH" 2>/dev/null && pwd)" || {
    echo "ERROR: Project path '$PROJECT_PATH' does not exist."
    exit 1
}

if [[ ! -f "$PROJECT_PATH/Cargo.toml" ]]; then
    echo "ERROR: No Cargo.toml found in '$PROJECT_PATH'."
    exit 1
fi

# Default report file
REPORT_FILE="${REPORT_FILE:-$PROJECT_PATH/lockbud_report.json}"

# Build lockbud if requested (explicit -b flag only; no auto-build)
if [[ "$BUILD" = true ]]; then
    echo "Building lockbud..."
    rustup run "$TOOLCHAIN" cargo build --bin lockbud --manifest-path "$OBOL_DIR/Cargo.toml"
fi

if [[ ! -f "$LOCKBUD_BIN" ]]; then
    echo "ERROR: lockbud binary not found at $LOCKBUD_BIN"
    echo "  Build with: cargo build --bin lockbud"
    echo "  Or use: ./scripts/detect.sh -b -k $KIND <project-path>"
    exit 1
fi

# Prepare environment
export RUST_LOG="${RUST_LOG:-warn}"
unset LD_LIBRARY_PATH

# Detect host target triple
RUST_TARGET="$(rustup run "$TOOLCHAIN" rustc -vV | grep '^host:' | awk '{print $2}')"
TARGET_TRIPLE="${RUST_TARGET:-x86_64-unknown-linux-gnu}"

# Copy rust-toolchain.toml to project
TOOLCHAIN_FILE="$PROJECT_PATH/rust-toolchain.toml"
HAD_TOOLCHAIN=false
if [[ -f "$TOOLCHAIN_FILE" ]]; then
    HAD_TOOLCHAIN=true
    cp "$TOOLCHAIN_FILE" "$TOOLCHAIN_FILE.lockbud.bak"
fi
cp "$OBOL_DIR/rust-toolchain.toml" "$TOOLCHAIN_FILE"

cleanup() {
    if [[ "$HAD_TOOLCHAIN" = true ]]; then
        mv "$TOOLCHAIN_FILE.lockbud.bak" "$TOOLCHAIN_FILE"
    else
        rm -f "$TOOLCHAIN_FILE"
    fi
}
trap cleanup EXIT

# Run cargo clean in the project directory
echo "Cleaning project..."
(cd "$PROJECT_PATH" && cargo clean 2>/dev/null || true)

# Run lockbud from the project directory so its internal `cargo build` targets the project.
# Suppress translation noise (matching test_lockbud.py's capture_output).
echo "Running lockbud -k $KIND on $PROJECT_PATH..."
echo ""

set +e
LOCKBUD_LOG=$(cd "$PROJECT_PATH" && rustup run "$TOOLCHAIN" "$LOCKBUD_BIN" \
    -k "$KIND" \
    --report-file "$REPORT_FILE" \
    -- \
    --target "$TARGET_TRIPLE" \
    "${CARGO_ARGS[@]}" 2>&1)
LOCKBUD_EXIT=$?
set -e

if [[ $LOCKBUD_EXIT -ne 0 ]]; then
    echo "WARNING: lockbud exited with code $LOCKBUD_EXIT"
    echo "Last output lines:"
    echo "$LOCKBUD_LOG" | tail -20
    echo ""
fi

echo "Report written to $REPORT_FILE"
echo ""

echo ""

# Display report summary
if [[ -f "$REPORT_FILE" ]]; then
    echo "============================================"
    echo "  Lockbud Report Summary"
    echo "============================================"

    python3 - "$REPORT_FILE" <<'PYEOF'
import json, collections, re, sys

with open(sys.argv[1]) as f:
    data = json.load(f)

print(f"Total findings: {len(data)}")
print()

if not data:
    sys.exit(0)

counts = collections.Counter(r.get("bug_kind", "Unknown") for r in data)
print("Findings by type:")
for kind, count in counts.most_common():
    print(f"  {kind}: {count}")
print()
print("Details:")

for i, r in enumerate(data, 1):
    kind = r.get("bug_kind", "Unknown")
    diag = r.get("diagnosis", {})
    inner = diag.get("diagnosis", diag)
    expl = diag.get("explanation", "")

    loc_strs = []
    for key in ("first_lock_span", "second_lock_span", "span"):
        val = inner.get(key)
        if isinstance(val, str):
            lines = [int(x) for x in re.findall(r"line: (\d+)", val)]
            cols = [int(x) for x in re.findall(r"col: (\d+)", val)]
            if lines:
                label = key.replace("_", " ")
                parts = [f"l{lines[0]}"]
                if cols:
                    parts.append(f"c{cols[0]}")
                if len(lines) > 1:
                    parts.append(f"-> l{lines[1]}:c{cols[1]}" if len(cols) > 1 else f"-> l{lines[1]}")
                loc_strs.append(f"{label}({', '.join(parts)})")
    loc_str = "; ".join(loc_strs) if loc_strs else "see report"

    print(f"  [{i}] {kind}  {loc_str}")
    if expl:
        print(f"      {expl}")
PYEOF

    echo ""
    echo "Full report: $REPORT_FILE"
else
    echo "No report generated."
fi
