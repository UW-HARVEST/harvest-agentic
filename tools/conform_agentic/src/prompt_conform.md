# C-to-Rust conformance refinement (third stage)

You are given a Rust project that was already translated from C and then
verified against the translator's **own, internally generated** tests. It
passes those internal tests, but may still fail an **external** test suite that
was written independently. Your single objective: **make every external test
pass**, by fixing the Rust — without weakening or gaming the tests.

{AGENT_BUG_WORKAROUNDS}

{WORKDIR_BOUNDARY}

{RUST_TOOLCHAIN_CONTEXT}

{MODEL_LIMITS}

## What is in your working directory

- `src/`, `Cargo.toml`, `Cargo.lock` — the Rust project you are refining. This
  is the ONLY code you may change.
- `c_src/` — the original C source. It is the **semantic ground truth**:
  when a test expects some value, `c_src/` is where you learn *why*. Never
  modify it.
- `{EXTERNAL_TEST_DIRS}` — the **external test suite**. This is your target.
  You may read it and run it, but you must NOT modify it (see rules below).
- `tests/` — the translator's own internally generated tests from the earlier
  verify stage (a differential harness comparing Rust against C). These are
  **context, not your target**: they already pass. They show you what was
  checked before — and, by omission, what was missed.
- `PLAN.md` / `plan_translate.md` — the original translation plan (round 1).
- `HYPOTHESES.md` / `hypotheses_verify.md` — the verify stage's bug-hypothesis
  log (round 2). Read it first: it often already names suspected defects
  (e.g. "decompress corrupts inputs over N bytes") that are exactly what the
  external tests will catch. Treat it as a set of leads.

## Step 0: after any compaction

If you were just compacted, your FIRST action is `cat CONFORM.md` to recover
state, then re-read this section. Do not re-explore from scratch.

## Step 1: maintain `CONFORM.md`

Create and continuously append to `CONFORM.md` in the project root — an
append-only log that survives compaction. Start it with an Invariants block
(copied verbatim below, it must never drift), then log every failing external
test, your hypothesis for the cause, the fix, and the outcome.

```
# Conformance Log

## Invariants (do not drift across compactions)

### AFTER ANY COMPACTION: `cat CONFORM.md` is my FIRST action.

### Ground truth
- The EXTERNAL test suite in {EXTERNAL_TEST_DIRS} defines success. The goal is
  zero external-test failures.
- `c_src/` is the semantic reference explaining WHY a test expects a value.
  If Rust and C disagree, the C behavior is correct — fix Rust.

### Boundaries
- Only `src/` and `Cargo.toml` may change. NEVER modify `c_src/`, the external
  test suite ({EXTERNAL_TEST_DIRS}), or `tests/`.
- The final grade re-runs the SAME external suite from a pristine copy, so any
  edit to the tests is discarded and counts as cheating. Editing tests can
  never help you.

### No overfitting
- Fix the underlying algorithm, never the specific test input. Do NOT
  special-case a test's exact bytes, hardcode an expected output, or branch on
  a recognized input. A fix that only works for the inputs you can see is a
  failed fix.

### Full public ABI
- Every non-`static` function the C shared library exports must also be
  exported by the Rust crate (verified with `nm -D`); a missing symbol makes
  the suite fail to link. Add any that are missing.

### Rust toolchain
- Build and run everything with the pinned toolchain (see the toolchain
  context above). If a different version is active, stop and report an
  environment problem rather than treating failures as translation bugs.

## Log
### C<N>: <failing test> — <one-line hypothesis>
- Status: open | fixed | refuted
- Evidence / expected vs actual:
- Fix (files/lines in src/):
- Outcome after re-running the test:
```

## Step 2: how to build and run the external tests

{CONFORM_TEST_INSTRUCTIONS}

### Per-test time budgets

If the suite ships a
`budgets.json` (look in the external test directory), each test is killed
after `max(C_baseline_seconds * factor, min_seconds)` — where `C_baseline`
is that test's measured runtime against the **original C library**. A test
that produces correct output but runs far slower than the C baseline is
killed and **counts as a failure**.

## Step 3: refinement workflow

1. Build and run the whole external suite once; record every failure in
   `CONFORM.md`.
2. Triage: group failures by likely root cause. Read the failing test source
   to learn the exact expected behavior, then read the corresponding `c_src/`
   to learn the correct semantics.
3. Fix the Rust in `src/`. Re-run the specific failing test, then the whole
   suite to catch regressions.
4. For independent bugs, delegate to sub-agents so they can be worked in
   parallel — each with a precise, self-contained task and the relevant
   `c_src/` reference.
5. Repeat until the external suite reports **zero failures**. Long-running
   tests are normal; do not mistake them for hangs.

## Step 4: completion gate — actually run the whole suite before you stop

Do NOT declare success from memory. Before you conclude and before writing the report, you MUST:

1. Do a **clean rebuild and run of the ENTIRE external suite in one pass**,
   using the exact same build/run commands the grader uses (Step 2) and the
   per-test budgets from `budgets.json`, not a larger timeout of your own.
2. Read the final result. The completion bar is **every test passing in that single clean run**.
3. Only if some test is genuinely unfixable -- you have made **at least three
   distinct, real fix attempts** at its root cause and it still fails -- may you stop with it unresolved.

State the final clean-run tally honestly in the report; do not round it up.

## Step 5: write `CONFORM_REPORT.md`

When you finish (whether or not you reached zero failures), write
`CONFORM_REPORT.md` in the project root analyzing the **gap between the
internal tests and the external tests**.

- Which external tests failed on arrival (before your fixes)? List them.
- For each, was the underlying bug something the internal tests in `tests/`
  (and the `hypotheses_verify.md` log) *could* have caught but didn't? Explain
  **why the internal differential harness failed to capture it**.
- Categorize the misses (missing API coverage, missing input diversity,
  missing multi-call/stateful sequences, missing edge cases, ABI/export gaps,
  ...) and estimate how much external failure each category accounts for.
- Concrete recommendations: what would the internal test generator have to do
  differently to have caught these bugs on its own?

Keep it evidence-based and specific to what you actually observed.
