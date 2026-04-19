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

# Auto-detect OBOL_DIR from the script's location (scripts/test_lockbud.py -> repo root).
_SCRIPT_DIR = Path(__file__).resolve().parent
_AUTO_OBOL_DIR = _SCRIPT_DIR.parent

LOCKBUD_DIR = Path(os.environ.get("LOCKBUD_DIR", Path.home() / "lockbud"))
OBOL_DIR = Path(os.environ.get("OBOL_DIR", _AUTO_OBOL_DIR))
TOOLCHAIN = "nightly-2026-02-07"
TOYS_DIR = LOCKBUD_DIR / "toys"

_LOCKBUD_BIN = OBOL_DIR / "target" / "debug" / "lockbud"


def _ensure_lockbud_bin():
    if not _LOCKBUD_BIN.exists():
        print(f"ERROR: lockbud binary not found at {_LOCKBUD_BIN}")
        print(f"  OBOL_DIR={OBOL_DIR}")
        print("  Build with: cargo build --bin lockbud")
        print("  Or set OBOL_DIR env var to the repo root.")
        sys.exit(1)


def _run_lockbud(cwd: Path, report_path: Path, kind: str, env: dict):
    """Run the lockbud binary and return the subprocess result."""
    result = subprocess.run(
        ["rustup", "run", TOOLCHAIN, str(_LOCKBUD_BIN), "-k", kind, "--report-file", str(report_path), "--", "--target", "x86_64-unknown-linux-gnu"],
        cwd=cwd, env=env, capture_output=True, text=True,
    )
    if result.returncode != 0:
        print(f"  WARNING: lockbud exited with code {result.returncode}")
        if result.stderr:
            # Print last few lines of stderr for diagnostics
            lines = result.stderr.strip().splitlines()
            for line in lines[-10:]:
                print(f"    {line}")
    return result

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
    _ensure_lockbud_bin()
    env = os.environ.copy()
    env["RUST_LOG"] = "warn"
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
    try:
        _run_lockbud(toy_dir, report_path, "deadlock", env)
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


def _setup_crate(tmp: str, name: str, cargo_toml: str, main_rs: str) -> Path:
    crate_dir = Path(tmp) / name
    crate_dir.mkdir()
    (crate_dir / "Cargo.toml").write_text(cargo_toml)
    src_dir = crate_dir / "src"
    src_dir.mkdir()
    (src_dir / "main.rs").write_text(main_rs)
    (crate_dir / "rust-toolchain.toml").write_text(
        (OBOL_DIR / "rust-toolchain.toml").read_text()
    )
    return crate_dir


def run_condvar_smoke_tests():
    _ensure_lockbud_bin()
    passed = 0
    failed = 0

    # ---- Test 1: std::sync::Condvar with extra lock (should report) ----
    with tempfile.TemporaryDirectory() as tmp:
        crate_dir = _setup_crate(
            tmp, "condvar_std_test",
            '[package]\nname = "condvar_std_test"\nversion = "0.1.0"\nedition = "2021"\n',
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
        env["RUST_LOG"] = "warn"
        env.pop("LD_LIBRARY_PATH", None)
        report_path = crate_dir / "report.json"
        report_path.unlink(missing_ok=True)
        _run_lockbud(crate_dir, report_path, "deadlock", env)

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
        crate_dir = _setup_crate(
            tmp, "condvar_std_ok",
            '[package]\nname = "condvar_std_ok"\nversion = "0.1.0"\nedition = "2021"\n',
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
        env["RUST_LOG"] = "warn"
        env.pop("LD_LIBRARY_PATH", None)
        report_path = crate_dir / "report.json"
        report_path.unlink(missing_ok=True)
        _run_lockbud(crate_dir, report_path, "deadlock", env)

        has_cd = False
        if report_path.exists():
            with open(report_path) as f:
                data = json.load(f)
            # Only check for CondvarDeadlock; Panic reports from unwrap() are expected.
            has_cd = any(r.get("bug_kind") == "CondvarDeadlock" for r in data)

        if not has_cd:
            print("[PASS] std::sync::Condvar correct-usage -> no CondvarDeadlock")
            passed += 1
        else:
            print("[FAIL] std::sync::Condvar correct-usage -> unexpected CondvarDeadlock")
            failed += 1

    # ---- Test 3: parking_lot::Condvar with extra lock (should report) ----
    with tempfile.TemporaryDirectory() as tmp:
        crate_dir = _setup_crate(
            tmp, "condvar_pl_test",
            '[package]\nname = "condvar_pl_test"\nversion = "0.1.0"\nedition = "2021"\n\n[dependencies]\nparking_lot = "0.12"\n',
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
        env["RUST_LOG"] = "warn"
        env.pop("LD_LIBRARY_PATH", None)
        report_path = crate_dir / "report.json"
        report_path.unlink(missing_ok=True)
        _run_lockbud(crate_dir, report_path, "deadlock", env)

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
        crate_dir = _setup_crate(
            tmp, "condvar_pl_ok",
            '[package]\nname = "condvar_pl_ok"\nversion = "0.1.0"\nedition = "2021"\n\n[dependencies]\nparking_lot = "0.12"\n',
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
        env["RUST_LOG"] = "warn"
        env.pop("LD_LIBRARY_PATH", None)
        report_path = crate_dir / "report.json"
        report_path.unlink(missing_ok=True)
        _run_lockbud(crate_dir, report_path, "deadlock", env)

        has_cd = False
        if report_path.exists():
            with open(report_path) as f:
                data = json.load(f)
            # Only check for CondvarDeadlock; other findings (Panic, UseAfterFree from inlined libs) are allowed.
            has_cd = any(r.get("bug_kind") == "CondvarDeadlock" for r in data)

        if not has_cd:
            print("[PASS] parking_lot::Condvar correct-usage -> no CondvarDeadlock")
            passed += 1
        else:
            print("[FAIL] parking_lot::Condvar correct-usage -> unexpected CondvarDeadlock")
            failed += 1

    print(f"\nCondvar smoke tests: {passed} passed, {failed} failed\n")
    return failed == 0


def run_additional_smoke_tests():
    _ensure_lockbud_bin()
    passed = 0
    failed = 0

    # ---- Atomicity Violation test ----
    with tempfile.TemporaryDirectory() as tmp:
        crate_dir = _setup_crate(
            tmp, "atomic_test",
            '[package]\nname = "atomic_test"\nversion = "0.1.0"\nedition = "2021"\n',
            '''
use std::sync::atomic::{AtomicUsize, Ordering};
fn check_and_set(atomic: &AtomicUsize) {
    let v = atomic.load(Ordering::Relaxed);
    if v == 0 {
        atomic.store(1, Ordering::Relaxed);
    }
}
fn main() {
    let a = AtomicUsize::new(0);
    check_and_set(&a);
}
'''
        )
        env = os.environ.copy()
        env["RUST_LOG"] = "warn"
        env.pop("LD_LIBRARY_PATH", None)
        report_path = crate_dir / "report.json"
        report_path.unlink(missing_ok=True)
        _run_lockbud(crate_dir, report_path, "atomicity_violation", env)

        has_av = False
        if report_path.exists():
            with open(report_path) as f:
                data = json.load(f)
            has_av = any(r.get("bug_kind") == "AtomicityViolation" for r in data)

        if has_av:
            print("[PASS] AtomicityViolation (load-store dep) -> reported")
            passed += 1
        else:
            print("[FAIL] AtomicityViolation (load-store dep) -> expected report")
            failed += 1

    # ---- InvalidFree test ----
    with tempfile.TemporaryDirectory() as tmp:
        crate_dir = _setup_crate(
            tmp, "invalid_free_test",
            '[package]\nname = "invalid_free_test"\nversion = "0.1.0"\nedition = "2021"\n',
            '''
use std::mem;
fn bad() {
    #[allow(deprecated)]
    let x: String = unsafe { mem::uninitialized() };
    mem::drop(x);
}
fn main() {
    bad();
}
'''
        )
        env = os.environ.copy()
        env["RUST_LOG"] = "warn"
        env.pop("LD_LIBRARY_PATH", None)
        report_path = crate_dir / "report.json"
        report_path.unlink(missing_ok=True)
        _run_lockbud(crate_dir, report_path, "memory", env)

        has_if = False
        if report_path.exists():
            with open(report_path) as f:
                data = json.load(f)
            has_if = any(r.get("bug_kind") == "InvalidFree" for r in data)

        if has_if:
            print("[PASS] InvalidFree (mem::uninitialized + mem::drop) -> reported")
            passed += 1
        else:
            print("[FAIL] InvalidFree (mem::uninitialized + mem::drop) -> expected report")
            failed += 1

    # ---- Panic test ----
    with tempfile.TemporaryDirectory() as tmp:
        crate_dir = _setup_crate(
            tmp, "panic_test",
            '[package]\nname = "panic_test"\nversion = "0.1.0"\nedition = "2021"\n',
            '''
fn main() {
    let _ = Some(1).unwrap();
}
'''
        )
        env = os.environ.copy()
        env["RUST_LOG"] = "warn"
        env.pop("LD_LIBRARY_PATH", None)
        report_path = crate_dir / "report.json"
        report_path.unlink(missing_ok=True)
        _run_lockbud(crate_dir, report_path, "panic", env)

        has_panic = False
        if report_path.exists():
            with open(report_path) as f:
                data = json.load(f)
            has_panic = any(r.get("bug_kind") == "Panic" for r in data)

        if has_panic:
            print("[PASS] Panic (Option::unwrap) -> reported")
            passed += 1
        else:
            print("[FAIL] Panic (Option::unwrap) -> expected report")
            failed += 1

    # ---- UseAfterFree test ----
    with tempfile.TemporaryDirectory() as tmp:
        crate_dir = _setup_crate(
            tmp, "uaf_test",
            '[package]\nname = "uaf_test"\nversion = "0.1.0"\nedition = "2021"\n',
            '''
fn consume(_: *mut String) {}

fn bad() {
    let raw;
    {
        let mut s = String::from("hello");
        raw = &raw mut s;
    }
    consume(raw);
}

fn main() {
    bad();
}
'''
        )
        env = os.environ.copy()
        env["RUST_LOG"] = "warn"
        env.pop("LD_LIBRARY_PATH", None)
        report_path = crate_dir / "report.json"
        report_path.unlink(missing_ok=True)
        _run_lockbud(crate_dir, report_path, "memory", env)

        has_uaf = False
        if report_path.exists():
            with open(report_path) as f:
                data = json.load(f)
            has_uaf = any(r.get("bug_kind") == "UseAfterFree" for r in data)

        if has_uaf:
            print("[PASS] UseAfterFree (raw ptr after scope drop) -> reported")
            passed += 1
        else:
            print("[FAIL] UseAfterFree (raw ptr after scope drop) -> expected report")
            failed += 1

    print(f"\nAdditional smoke tests: {passed} passed, {failed} failed\n")
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
    parser.add_argument(
        "--all",
        action="store_true",
        help="Run all smoke tests (condvar, atomic, memory, panic)",
    )
    args = parser.parse_args()

    ok = True
    print("=" * 70)
    print("Lockbud-ULLBC Deadlock Toy Benchmarks")
    print("=" * 70)
    ok = run_deadlock_benchmarks() and ok

    if args.condvar or args.all:
        print("=" * 70)
        print("Lockbud-ULLBC Condvar Smoke Tests")
        print("=" * 70)
        ok = run_condvar_smoke_tests() and ok

    if args.all:
        print("=" * 70)
        print("Lockbud-ULLBC Additional Smoke Tests")
        print("=" * 70)
        ok = run_additional_smoke_tests() and ok

    if ok:
        print("All tests PASSED ✓")
        sys.exit(0)
    else:
        print("Some tests FAILED ✗")
        sys.exit(1)


if __name__ == "__main__":
    main()
