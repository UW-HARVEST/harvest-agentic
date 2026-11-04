---
name: rust_quality
description: Measures and improves the safety and idiomaticity of a Rust crate that was translated from C, without changing its behavior. qscan counts the operations that need `unsafe` (as DARPA counts them), the clippy and rustc lints, and the pseudo-safe functions of each function, and suggests recipes. regress runs the existing regression tests against a recorded baseline. qflow runs the fixed steps (baseline, local integration). Use it to refactor C-like Rust (raw pointers, unsafe fn everywhere, C memory calls, `as` casts) into safer Rust in small, tested steps.
license: none
metadata:
  version: "0.2"
---

# rust_quality

## The problem

A Rust crate translated from C usually keeps the C data model: raw pointers, `unsafe fn`
everywhere, pointer arithmetic, `memcpy`, `static mut`, integer flags and many `as` casts. It
passes its tests, but it is not safe Rust. This skill gives you tools to measure the problems per
function, refactorings with exact conditions, and a regression gate that tells you when a
refactoring changed the behavior. The rules for every change are in your task file.

| File | Content |
|---|---|
| `RECIPES.md` | Local refactorings: one function at a time. |
| `GLOBAL.md` | Global refactorings: a planned change of the data model across files. |
| `CURSORS.md` | The cursor model: buffers as views and pointer cursors as integer positions. |

## The tools

Run all commands from the root of your checkout (the crate root). `S` below means
`inputs/agent_skills/rust_quality/scripts`.

### qscan: measure

| Command | What it does |
|---|---|
| `python3 S/qscan.py scan --out DIR` | Measures the crate (about 2 to 60 seconds). Writes `DIR/summary.md`, `functions.tsv`, `ops.tsv`, `lints.tsv`, `summary.json`. Prints the head of the summary. |
| `python3 S/qscan.py file src/x.rs --scan DIR [--lines A-B] [--limit N]` | The worklist of a file: each function with work, its facts, the line numbers of its unsafe operations, its lints, and the suggested recipes. Highest score first, 25 functions unless `--limit` says otherwise. |
| `python3 S/qscan.py fn NAME --scan DIR` | Every unsafe operation and lint of one function, with line, column and message. |
| `python3 S/qscan.py compare BEFORE AFTER [--files a.rs,b.rs]` | Changes between two scans, and the PROBLEMS that a change must not create: a changed export, a new pseudo-safe function, a new blanket allow, boilerplate `# Safety` texts, compile errors. |
| `python3 S/qscan.py plan --scan DIR --out FILE` | Divides the work into units (whole functions) and waves for worker sub-agents. |
| `python3 S/qscan.py roots --scan DIR [--out FILE]` | The root causes of the unsafe operations, for global refactoring: types reached through raw pointers, buffer cursors and pointer arithmetic, unsafe callees, structs with raw pointer fields, the C ABI share. See `GLOBAL.md`. |
| `python3 S/qscan.py autofix --commit [--lints a,b]` | Applies the syntax-only clippy fixes, one lint at a time. Each lint is built, and committed alone. |

`scan` parses Rust with tree-sitter. The first scan of a run makes a Python venv with it in
`shared/.venv` (a few seconds; it needs network). All checkouts and worktrees use this venv.

What the numbers mean:

- **unsafe ops**: each operation that needs `unsafe` (raw pointer dereference, call of an unsafe
  function, use of a `static mut`, union field, ...). This is the DARPA safety measure. It does
  not change when you only move code between `unsafe fn` and `unsafe {}` blocks.
- **lints**: clippy lints of the groups correctness, suspicious, complexity, perf and style, plus
  the rustc warnings. Measured with the crate's `allow` attributes ignored. **extra lints** are
  C-style signals outside those groups: `cast_lossless`, `ptr_as_ptr`, `borrow_as_ptr`,
  `ptr_cast_constness`, `undocumented_unsafe_blocks`.
- **pseudo-safe**: a function with a safe signature that dereferences a raw pointer parameter in
  an `unsafe {}` block. The caller thinks it is safe, but it is not. Never create one. Fix the
  existing ones (recipe R12).
- **score**: the ranking weight in a worklist (unsafe ops + lints + a part of the extra lints).

### regress: the regression gate

| Command | What it does |
|---|---|
| `python3 S/regress.py setup` | Once, before any change (`qflow.py baseline` runs it). Builds the crate and the verify stage's test harness (`verify_env/`) into `shared/regress/`, and records the baseline results. Takes minutes. |
| `python3 S/regress.py check --quick` | Builds your checkout, runs the unit tests and a replay of the saved fuzz inputs, and compares with the baseline. Use it after each batch of changes. |
| `python3 S/regress.py check [--fuzz-filter REGEX] [--fuzz-seconds N]` | Also runs a short fuzz campaign (10 seconds by default) for each property, or for the matching ones. A quick smoke check, not the verify stage's fuzzing. |

The last line of `check` is `REGRESS: OK` or `REGRESS: FAIL: <reasons>`. A test that fails in the
baseline is not a regression. A regression is a test or saved input that passed in the baseline and
fails now, fewer passing tests than in the baseline, a change of the exported symbols, or a fuzz
failure that the baseline library does not have. The full report of the last check is in
`scratch/regress/last.md`.

The test binaries do not contain the Rust code: they load your library at run time. Thus `setup`
builds them once, and every worktree uses them.

### qflow: the scripted steps

| Command | What it does |
|---|---|
| `python3 S/qflow.py baseline` | In the main checkout, once: `regress.py setup`, a scan (`shared/quality/baseline`), `qscan.py autofix` with the gate (an autofix that fails the gate is reverted), a scan (`shared/quality/autofix`), the root causes, the units (`shared/quality/UNITS.md`), and the commit of `quality/BASELINE.md`. |
| `python3 S/qflow.py integrate --name intN --branches u1,u2 --before SCAN --after SCAN` | In the integration worktree of a local wave: merges each branch; keeps it only when the quick gate passes and `compare` finds no problem; runs the full gate (it reverts the newest merges until the gate passes); scans, and commits `quality/INTEGRATION_<name>.md`. |

The last line is `QFLOW: OK` or `QFLOW: FAIL: <reason>`.

## Long commands

`regress.py check --quick` takes about 2 to 4 minutes on a mid-size crate, a full check longer,
and `qflow.py` 10 minutes or more. A shell tool usually stops a command after 2 minutes. So:

- If your shell tool takes a timeout, give it 10 minutes (600000 ms) for `regress.py check`.
- Run a command that can take longer than that in the background, and wait for it in steps:

  ```
  nohup sh -c 'python3 S/<command> > scratch/run.log 2>&1; echo "EXIT $?" >> scratch/run.log' > /dev/null 2>&1 &
  timeout 500 sh -c 'until grep -q "^EXIT" scratch/run.log; do sleep 15; done'; tail -5 scratch/run.log
  ```

  Repeat the second line until the log has the `EXIT` line. (Without a 10-minute tool timeout, use
  `timeout 100` instead of `timeout 500`.)

## Two kinds of change

- **Local** (`RECIPES.md`): one function at a time, in the files of your unit. Most lints, honest
  signatures, references for context pointers, slices for simple buffers.
- **Global** (`GLOBAL.md`, `CURSORS.md`): a planned change of the data model across files:
  references threaded through call chains, buffers as views with integer cursors, owned buffers,
  an API isolation layer, a small trusted module. Global changes follow a written proposal with a
  contract.

## The method (local)

1. Measure (`qscan scan`). Read the worklist of your file (`qscan file`).
2. Take 2 or 3 functions. Read them (`sed -n 'A,Bp' FILE`). Apply recipes from `RECIPES.md`, only
   when all their conditions are true.
3. Build (`cargo build --release`). Check (`regress.py check --quick`).
4. `REGRESS: OK`: commit. `REGRESS: FAIL`: find the function that fails, fix it or undo it.
5. Next batch. At the end: scan again and compare.
