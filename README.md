# lockbud
Statically detect memory, concurrency bugs and possible panic locations for Rust.

## Introduction

This project, "lockbud", is the artifact of the research paper ["Understanding and Detecting Real-World Safety Issues in Rust"](https://songlh.github.io/paper/rust-tse.pdf), published in TSE'24. It builds upon our previous work ["Understanding Memory and Thread Safety Practices and Issues in Real-World Rust Programs"](https://songlh.github.io/paper/rust-study.pdf), published in PLDI'20.

The project includes detectors for the following types of issues:

- Concurrency Bugs
  - Blocking Bugs (use `-k deadlock`)
    - Double-Lock
    - Conflicting-Lock-Order
    - Condvar Misuse (not appeared in paper)
  - Non-blocking Bugs
    - Atomicity-Violation (use `-k atomicity_violation`)
- Memory Bugs (use `-k memory`)
    - Use-After-Free
    - Invalid-Free (not appeared in paper)
- Panic Locations (use `-k panic`)

The `Data` directory contains two Excel files:

- BugStudy.xlsx: Records the results of the research study.
- BugReport.xlsx: Records the experimental results.

## Announcements

The codebase was implemented quickly, and I plan to refactor it in the future. The todo list is in #58.

For now, when checking your project, please ignore the bug reports originating from the standard library and common dependencies (especially the memory detectors), as they are mostly false positives. To focus on your own project instead of dependencies, you can pass the `-l your_project_name` flag to lockbud.

The deadlock detectors (double-lock and conflicting-lock-order) perform better than the other detectors, as the project was initially designed for deadlock detection. The bug patterns are based on the observations from our research, and I've tried to minimize false positives and false negatives caused by the program analysis over-approximation.

To reduce the detectors' overhead, I did not introduce SMTs or other expensive analyses. As a result, the panic location detector may report nearly all the panic locations, making it less useful. I hope that a new, unified static analysis framework for Rust will emerge soon to address these limitations.

## Install
Currently supports rustc nightly-2026-02-07.
```
$ git clone https://github.com/BurtonQin/lockbud.git
$ cd lockbud
$ rustup +nightly-2026-02-07 component add rust-src rustc-dev llvm-tools-preview
$ cargo +nightly-2026-02-07 install --path .
```

Note that you must use the same rustc nightly version as lockbud to detect your project!
You can either override your rustc version or specify rust-toolchains in your project.

## Example
## Test toys
```
$ ./detect.sh toys/inter
```
It will print 15 doublelock bugs in json format, like the following one:

```
      {
        "DoubleLock": {
          "bug_kind": "DoubleLock",
          "possibility": "Possibly",
          "diagnosis": {
            "first_lock_type": "ParkingLotWrite(i32)",
            "first_lock_span": "src/main.rs:77:16: 77:32 (#0)",
            "second_lock_type": "ParkingLotRead(i32)",
            "second_lock_span": "src/main.rs:84:18: 84:33 (#0)",
            "callchains": [
              [
                [
                  "src/main.rs:79:20: 79:52 (#0)"
                ]
              ]
            ]
          },
          "explanation": "The first lock is not released when acquiring the second lock"
        }
      }
```

The output shows that there is possibly a doublelock bug. The DeadlockDiagnosis reads that the first lock is a parking_lot WriteLock acquired on src/main.rs:77 and the second lock is a parking_lot ReadLock aquired on src/main.rs:84. The first lock reaches the second lock through callsites src/main.rs:79. The explanation demonstrates the reason for doubelock.

```
$ ./detect.sh toys/conflict-inter
```
It will print one conflictlock bug

```
      {
        "ConflictLock": {
          "bug_kind": "ConflictLock",
          "possibility": "Possibly",
          "diagnosis": [
            {
              "first_lock_type": "StdRwLockRead(i32)",
              "first_lock_span": "src/main.rs:29:16: 29:40 (#0)",
              "second_lock_type": "StdMutex(i32)",
              "second_lock_span": "src/main.rs:36:10: 36:34 (#0)",
              "callchains": [
                [
                  [
                    "src/main.rs:31:20: 31:38 (#0)"
                  ]
                ]
              ]
            },
            {
              "first_lock_type": "StdMutex(i32)",
              "first_lock_span": "src/main.rs:18:16: 18:40 (#0)",
              "second_lock_type": "StdRwLockWrite(i32)",
              "second_lock_span": "src/main.rs:25:10: 25:35 (#0)",
              "callchains": [
                [
                  [
                    "src/main.rs:20:20: 20:35 (#0)"
                  ]
                ]
              ]
            }
          ],
          "explanation": "Locks mutually wait for each other to form a cycle"
        }
      }
```

The output shows that there is possibly a conflictlock bug. The DeadlockDiagnosis is similar to doublelock bugs except that there are at least two diagnosis records. All the diagnosis records form a cycle, e.g. A list of records [(first_lock, second_lock), (second_lock', first_lock')] means that it is possible that first_lock is aquired and waits for second_lock in one thread, while second_lock' is aquired and waits for first_lock' in another thread, which incurs a conflictlock bug.

`detect.sh` is mainly for development of the detector and brings more flexibility.
You can modify `detect.sh` to use release vesion of lockbud to detect large and complex projects.
And you can use `LOCKBUD_FLAGS` to select detectors and projects to be checked. See the commented `export LOCKBUD_FLAGS=...` in `detect.sh`

## Using `cargo lockbud`
For ease of use, you can also run cargo lockbud
```
$ cd toys/inter; cargo clean; cargo lockbud -k deadlock
```
Note that you need to run
```
cargo clean
```
before re-running lockbud.

You can also specify blacklist or whitelist of crate names.

The `-b` implies the list is a blacklist.

The `-l` is followed by a list of crate names seperated by commas.
```
$ cd YourProject; cargo clean; cargo lockbud -k deadlock -b -l cc,tokio_util,indicatif
```

You have ommitted the checking of dependencies cc, tokio_util, indicatif .

You can also ensure to use the right version of the compiler by running:

```
$ cargo +nightly-2024-12-01 lockbud
```

### Using by docker

Current available docker image is `burtonqin/lockbud`[^1]

```shell
docker run --rm -it -v ./toys/inter/:/volume burtonqin/lockbud -k deadlock
```

lockbud will execute `cargo clean && cargo lockbud -k deadlock` in `/volume` directory.

> **Note**  
> It will compile your project in docker, so you need to manualy remove the target directory before you are ready for working.
> The lockbud version in docker may not be the latest. Please help test it.

### Using in CI

```yaml
name: Lockbud

on: workflow_dispatch

jobs:
  test:
    name: lockbud
    runs-on: ubuntu-latest
    container:
      image: burtonqin/lockbud
    steps:
      - name: Checkout repository
        uses: actions/checkout@v3

      - name: Generate code coverage
        run: |
          cargo lockbud -k deadlock
```

> **Note**  
> Currently lockbud output in stdout

## How it works
In Rust, a lock operation returns a lockguard. The lock will be unlocked when the lockguard is dropped.
So we can track the lifetime of lockguards to detect lock-related bugs.
For each crate (the crate to be detected and its dependencies)
1. Collect the caller-callee info to generate a callgraph.
2. Collect LockGuard info, including
   - The lockguard type and span;
   - Where it is created and where it is dropped.
3. Apply a GenKill algorithm on the callgraph to find pairs of lockguards (a, b) s.t.
   - a not dropped when b is created.
4. A pair (a, b) can doublelock if
   - the lockguard types of a & b can deadlock;
   - and a & b may point to the same lock (obtained from points-to analysis).
5. For (a, b), (c, d) in the remaining pairs
   - if b and c can deadlock then add an edge from (a, b) to (c, d) into a graph.
6. The cycle in the graph implies a conflictlock.

I will write some doc to explain the implementation details of other checkers.

## Caveats

Deadlock Detectors

1. Currently only supports `std::sync::{Mutex, RwLock}`, `parking_lot::{Mutex, RwLock}`, `spin::{Mutex, RwLock}`
2. The callgraph is crate-specific (the callers and callees are in the same crate) and cannot track indirect call.
3. The points-to analysis is imprecise and makes heuristic assumptions for function calls and assignments.
   - A common FP comes from `cc`, where points-to analysis incorrectly assumes that two unrelated lockguards are from the same lock. Thus blacklist `cc` in `detector.sh`.
  
Memory and Panic Location Detectors

See Announcements.

## Results
Found dozens of bugs in many repositories: openethereum, grin, winit, sonic, lighthouse, etc.
Some of the repositories are dependencies of other large projects.
We try to strike a balance between FP and FN to make the detector usable.

Some typical Bugs detected and fixed (one PR may fix multiple bugs) in the follwing list. For a complete bug report list, please refer to `Data/BugReport.xlsx`. 

1. https://github.com/openethereum/openethereum/pull/289
2. https://github.com/openethereum/parity-ethereum/pull/11764
3. https://github.com/sigp/lighthouse/pull/1241
4. https://github.com/solana-labs/solana/pull/10466
5. https://github.com/solana-labs/solana/pull/10469
6. https://github.com/wasmerio/wasmer/pull/1466
7. https://github.com/paritytech/substrate/pull/6277
8. https://github.com/mimblewimble/grin/pull/3337
9. https://github.com/mimblewimble/grin/pull/3340
10. https://github.com/sigp/lighthouse/pull/1241
11. https://github.com/rust-windowing/winit/pull/1579
12. https://github.com/solana-labs/solana/pull/26046
13. https://github.com/solana-labs/solana/pull/26047
14. https://github.com/solana-labs/solana/pull/26053
15. https://github.com/qdrant/qdrant/issues/724
16. https://github.com/apache/incubator-teaclave-sgx-sdk/pull/269

## Disclaimer

This open-source project is provided "as is" without warranty of any kind, either expressed or implied, including, but not limited to, the implied warranties of merchantability and fitness for a particular purpose. In no event shall the authors or copyright holders be liable for any claim, damages, or other liability, whether in an action of contract, tort, or otherwise, arising from, out of, or in connection with the software or the use or other dealings in the software.

The purpose of this project is to provide a tool for detecting and identifying potential vulnerabilities in software applications. However, the accuracy and effectiveness of the tool is not guaranteed. Users of this tool should exercise caution and judgment when interpreting the results, and should not rely solely on the tool's findings to make decisions about the security of their systems.

The authors and contributors of this project do not endorse or encourage the use of this tool for any unlawful or unethical purposes, such as hacking, breaking into systems, or exploiting vulnerabilities without authorization. Users of this tool are solely responsible for their actions and the consequences thereof.

By using this open-source project, you acknowledge and agree to the terms of this disclaimer. If you do not agree with the terms of this disclaimer, you should not use this project.

## License
The lockbud Project is licensed under BSD-3.
