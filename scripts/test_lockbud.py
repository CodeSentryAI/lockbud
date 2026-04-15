#!/usr/bin/env python3
"""
One-click test script for Lockbud-ULLBC.

Usage:
    python3 scripts/test_lockbud.py           # Run deadlock toy benchmarks
    python3 scripts/test_lockbud.py --condvar # Also run condvar smoke tests
    python3 scripts/test_lockbud.py --help    # Show this message

Environment:
    LOCKBUD_DIR   Path to original lockbud repo (default: ~/lockbud)
    OBOL_DIR      Path to obol repo (default: ~/obol)
"""

import argparse
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

LOCKBUD_DIR = Path(os.environ.get("LOCKBUD_DIR", Path.home() / "lockbud"))
OBOL_DIR = Path(os.environ.get("OBOL_DIR", Path.home() / "obol"))
TOYS_DIR = LOCKBUD_DIR / "toys"

DEADLOCK_TOYS = [
    "inter",
    "intra",
    "conflict",
    "conflict-inter",
    "call-no-deadlock",
    "recursive-no-deadlock",
    "wait-lock-no-deadlock",
]


def run_original(toy_dir: Path):
    env = os.environ.copy()
    env["RUSTC_WRAPPER"] = str(LOCKBUD_DIR / "target" / "debug" / "lockbud")
    env["LOCKBUD_LOG"] = "warn"
    env["LOCKBUD_FLAGS"] = f"-k deadlock -l {toy_dir.name.replace('-', '_')}"
    env.pop("LD_LIBRARY_PATH", None)
    subprocess.run(
        ["cargo", "+nightly-2025-10-02", "clean"],
        cwd=toy_dir, env=env, capture_output=True,
    )
    result = subprocess.run(
        ["cargo", "+nightly-2025-10-02", "build"],
        cwd=toy_dir, env=env, capture_output=True, text=True,
    )
    out = result.stdout + result.stderr
    dl = out.count('"bug_kind": "DoubleLock"')
    cl = out.count('"bug_kind": "ConflictLock"')
    return dl, cl


def run_ullbc(toy_dir: Path):
    env = os.environ.copy()
    env["RUSTC_WRAPPER"] = str(OBOL_DIR / "target" / "debug" / "lockbud-ullbc-driver")
    env["RUST_LOG"] = "warn"
    env["OBOL_USING_CARGO"] = "1"
    env.pop("LD_LIBRARY_PATH", None)
    subprocess.run(
        ["cargo", "clean"],
        cwd=toy_dir, env=env, capture_output=True,
    )
    obol_toolchain = (OBOL_DIR / "rust-toolchain.toml").read_text()
    toolchain_path = toy_dir / "rust-toolchain.toml"
    toolchain_path.write_text(obol_toolchain)
    report_path = toy_dir / f"{toy_dir.name}.lockbud.json"
    report_path.unlink(missing_ok=True)
    opts = {"report_file": str(report_path)}
    env["OBOL_ARGS"] = json.dumps(opts)
    try:
        result = subprocess.run(
            ["cargo", "build", "--target", "x86_64-unknown-linux-gnu"],
            cwd=toy_dir, env=env, capture_output=True, text=True,
        )
    finally:
        toolchain_path.unlink(missing_ok=True)

    dl = cl = 0
    if report_path.exists():
        try:
            with open(report_path) as f:
                data = json.load(f)
            dl = sum(1 for r in data if r.get("bug_kind") == "DoubleLock")
            cl = sum(1 for r in data if r.get("bug_kind") == "ConflictLock")
        except Exception:
            pass
    return dl, cl


def run_deadlock_benchmarks():
    print(f"{'Toy':<25} {'Orig DL':>8} {'Orig CL':>8} {'ULLBC DL':>8} {'ULLBC CL':>8} {'Match':>6}")
    print("-" * 70)
    all_match = True
    for toy in DEADLOCK_TOYS:
        toy_dir = TOYS_DIR / toy
        if not toy_dir.exists():
            print(f"{toy:<25} MISSING")
            all_match = False
            continue
        odl, ocl = run_original(toy_dir)
        udl, ucl = run_ullbc(toy_dir)
        match = "YES" if (odl == udl and ocl == ucl) else "NO"
        if match == "NO":
            all_match = False
        print(f"{toy:<25} {odl:>8} {ocl:>8} {udl:>8} {ucl:>8} {match:>6}")
    print()
    return all_match


def run_condvar_smoke_tests():
    passed = 0
    failed = 0

    # ---- Test 1: std::sync::Condvar with extra lock (should report) ----
    with tempfile.TemporaryDirectory() as tmp:
        crate_dir = Path(tmp) / "condvar_std_test"
        crate_dir.mkdir()
        (crate_dir / "Cargo.toml").write_text(
            '[package]\nname = "condvar_std_test"\nversion = "0.1.0"\nedition = "2021"\n'
        )
        src_dir = crate_dir / "src"
        src_dir.mkdir()
        (src_dir / "main.rs").write_text(
            '''
use std::sync::{Arc, Condvar, Mutex};
fn same_function(pair: Arc<((Mutex<bool>, Condvar), Mutex<i32>)>) {
    let ((lock, cvar), extra) = &*pair;
    let _g = extra.lock().unwrap();
    let started = lock.lock().unwrap();
    cvar.notify_one();
    drop(started);
    let _g2 = extra.lock().unwrap();
    let started2 = lock.lock().unwrap();
    let returned = cvar.wait(started2).unwrap();
    drop(returned);
}
fn main() {
    let pair = Arc::new(((Mutex::new(false), Condvar::new()), Mutex::new(0)));
    same_function(pair);
}
'''
        )
        env = os.environ.copy()
        env["RUSTC_WRAPPER"] = str(OBOL_DIR / "target" / "debug" / "lockbud-ullbc-driver")
        env["RUST_LOG"] = "warn"
        env["OBOL_USING_CARGO"] = "1"
        env.pop("LD_LIBRARY_PATH", None)
        (crate_dir / "rust-toolchain.toml").write_text(
            (OBOL_DIR / "rust-toolchain.toml").read_text()
        )
        report_path = crate_dir / "report.json"
        report_path.unlink(missing_ok=True)
        env["OBOL_ARGS"] = json.dumps({"report_file": str(report_path)})
        subprocess.run(
            ["cargo", "build", "--target", "x86_64-unknown-linux-gnu"],
            cwd=crate_dir, env=env, capture_output=True, text=True,
        )

        has_cd = False
        if report_path.exists():
            with open(report_path) as f:
                data = json.load(f)
            has_cd = any(r.get("bug_kind") == "CondvarDeadlock" for r in data)

        if has_cd:
            print("[PASS] std::sync::Condvar extra-lock  -> CondvarDeadlock reported")
            passed += 1
        else:
            print("[FAIL] std::sync::Condvar extra-lock  -> expected CondvarDeadlock")
            failed += 1

    # ---- Test 2: std::sync::Condvar without extra lock (should NOT report) ----
    with tempfile.TemporaryDirectory() as tmp:
        crate_dir = Path(tmp) / "condvar_std_ok"
        crate_dir.mkdir()
        (crate_dir / "Cargo.toml").write_text(
            '[package]\nname = "condvar_std_ok"\nversion = "0.1.0"\nedition = "2021"\n'
        )
        src_dir = crate_dir / "src"
        src_dir.mkdir()
        (src_dir / "main.rs").write_text(
            '''
use std::sync::{Arc, Condvar, Mutex};
fn worker(pair: Arc<(Mutex<bool>, Condvar)>) {
    let (lock, cvar) = &*pair;
    let started = lock.lock().unwrap();
    cvar.notify_one();
    drop(started);
}
fn main() {
    let pair = Arc::new((Mutex::new(false), Condvar::new()));
    let pair2 = pair.clone();
    std::thread::spawn(move || worker(pair2));
    let (lock, cvar) = &*pair;
    let started = lock.lock().unwrap();
    let returned = cvar.wait(started).unwrap();
    drop(returned);
}
'''
        )
        env = os.environ.copy()
        env["RUSTC_WRAPPER"] = str(OBOL_DIR / "target" / "debug" / "lockbud-ullbc-driver")
        env["RUST_LOG"] = "warn"
        env["OBOL_USING_CARGO"] = "1"
        env.pop("LD_LIBRARY_PATH", None)
        (crate_dir / "rust-toolchain.toml").write_text(
            (OBOL_DIR / "rust-toolchain.toml").read_text()
        )
        report_path = crate_dir / "report.json"
        report_path.unlink(missing_ok=True)
        env["OBOL_ARGS"] = json.dumps({"report_file": str(report_path)})
        subprocess.run(
            ["cargo", "build", "--target", "x86_64-unknown-linux-gnu"],
            cwd=crate_dir, env=env, capture_output=True, text=True,
        )

        has_any = False
        if report_path.exists():
            with open(report_path) as f:
                data = json.load(f)
            has_any = len(data) > 0

        if not has_any:
            print("[PASS] std::sync::Condvar correct-usage -> zero findings")
            passed += 1
        else:
            print("[FAIL] std::sync::Condvar correct-usage -> unexpected findings")
            failed += 1

    # ---- Test 3: parking_lot::Condvar with extra lock (should report) ----
    with tempfile.TemporaryDirectory() as tmp:
        crate_dir = Path(tmp) / "condvar_pl_test"
        crate_dir.mkdir()
        (crate_dir / "Cargo.toml").write_text(
            '[package]\nname = "condvar_pl_test"\nversion = "0.1.0"\nedition = "2021"\n\n[dependencies]\nparking_lot = "0.12"\n'
        )
        src_dir = crate_dir / "src"
        src_dir.mkdir()
        (src_dir / "main.rs").write_text(
            '''
use parking_lot::{Condvar, Mutex};
use std::sync::Arc;
fn same_function(pair: Arc<((Mutex<bool>, Condvar), Mutex<i32>)>) {
    let ((lock, cvar), extra) = &*pair;
    let _g = extra.lock();
    let started = lock.lock();
    cvar.notify_one();
    drop(started);
    let _g2 = extra.lock();
    let mut started2 = lock.lock();
    cvar.wait(&mut started2);
    drop(started2);
}
fn main() {
    let pair = Arc::new(((Mutex::new(false), Condvar::new()), Mutex::new(0)));
    same_function(pair);
}
'''
        )
        env = os.environ.copy()
        env["RUSTC_WRAPPER"] = str(OBOL_DIR / "target" / "debug" / "lockbud-ullbc-driver")
        env["RUST_LOG"] = "warn"
        env["OBOL_USING_CARGO"] = "1"
        env.pop("LD_LIBRARY_PATH", None)
        (crate_dir / "rust-toolchain.toml").write_text(
            (OBOL_DIR / "rust-toolchain.toml").read_text()
        )
        report_path = crate_dir / "report.json"
        report_path.unlink(missing_ok=True)
        env["OBOL_ARGS"] = json.dumps({"report_file": str(report_path)})
        subprocess.run(
            ["cargo", "build", "--target", "x86_64-unknown-linux-gnu"],
            cwd=crate_dir, env=env, capture_output=True, text=True,
        )

        has_cd = False
        if report_path.exists():
            with open(report_path) as f:
                data = json.load(f)
            has_cd = any(r.get("bug_kind") == "CondvarDeadlock" for r in data)

        if has_cd:
            print("[PASS] parking_lot::Condvar extra-lock -> CondvarDeadlock reported")
            passed += 1
        else:
            print("[FAIL] parking_lot::Condvar extra-lock -> expected CondvarDeadlock")
            failed += 1

    # ---- Test 4: parking_lot::Condvar without extra lock (should NOT report) ----
    with tempfile.TemporaryDirectory() as tmp:
        crate_dir = Path(tmp) / "condvar_pl_ok"
        crate_dir.mkdir()
        (crate_dir / "Cargo.toml").write_text(
            '[package]\nname = "condvar_pl_ok"\nversion = "0.1.0"\nedition = "2021"\n\n[dependencies]\nparking_lot = "0.12"\n'
        )
        src_dir = crate_dir / "src"
        src_dir.mkdir()
        (src_dir / "main.rs").write_text(
            '''
use parking_lot::{Condvar, Mutex};
use std::sync::Arc;
fn worker(pair: Arc<(Mutex<bool>, Condvar)>) {
    let (lock, cvar) = &*pair;
    let started = lock.lock();
    cvar.notify_one();
    drop(started);
}
fn main() {
    let pair = Arc::new((Mutex::new(false), Condvar::new()));
    let pair2 = pair.clone();
    std::thread::spawn(move || worker(pair2));
    let (lock, cvar) = &*pair;
    let mut started = lock.lock();
    cvar.wait(&mut started);
    drop(started);
}
'''
        )
        env = os.environ.copy()
        env["RUSTC_WRAPPER"] = str(OBOL_DIR / "target" / "debug" / "lockbud-ullbc-driver")
        env["RUST_LOG"] = "warn"
        env["OBOL_USING_CARGO"] = "1"
        env.pop("LD_LIBRARY_PATH", None)
        (crate_dir / "rust-toolchain.toml").write_text(
            (OBOL_DIR / "rust-toolchain.toml").read_text()
        )
        report_path = crate_dir / "report.json"
        report_path.unlink(missing_ok=True)
        env["OBOL_ARGS"] = json.dumps({"report_file": str(report_path)})
        subprocess.run(
            ["cargo", "build", "--target", "x86_64-unknown-linux-gnu"],
            cwd=crate_dir, env=env, capture_output=True, text=True,
        )

        has_any = False
        if report_path.exists():
            with open(report_path) as f:
                data = json.load(f)
            has_any = len(data) > 0

        if not has_any:
            print("[PASS] parking_lot::Condvar correct-usage -> zero findings")
            passed += 1
        else:
            print("[FAIL] parking_lot::Condvar correct-usage -> unexpected findings")
            failed += 1

    print(f"\nCondvar smoke tests: {passed} passed, {failed} failed\n")
    return failed == 0


def main():
    parser = argparse.ArgumentParser(
        description="One-click test script for Lockbud-ULLBC",
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    parser.add_argument(
        "--condvar",
        action="store_true",
        help="Also run Condvar smoke tests (requires parking_lot crate)",
    )
    args = parser.parse_args()

    ok = True
    print("=" * 70)
    print("Lockbud-ULLBC Deadlock Toy Benchmarks")
    print("=" * 70)
    ok = run_deadlock_benchmarks() and ok

    if args.condvar:
        print("=" * 70)
        print("Lockbud-ULLBC Condvar Smoke Tests")
        print("=" * 70)
        ok = run_condvar_smoke_tests() and ok

    if ok:
        print("All tests PASSED ✓")
        sys.exit(0)
    else:
        print("Some tests FAILED ✗")
        sys.exit(1)


if __name__ == "__main__":
    main()
