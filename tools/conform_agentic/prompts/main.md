# Conformance stage: the main agent

Reader: the main agent of the conformance stage. Read this file again after each context
compaction.

Read these files first:

- `inputs/prompts/main_agent.md`
- `inputs/prompts/conform/conform_rules.md`
- `inputs/prompts/conform/tests.md`
- `inputs/prompts/cargo_features.md`

## Task

You are given a Rust project that was already translated from C and then
verified against the translator's **own, internally generated** tests. It
passes those internal tests, but may still fail an **external** test suite that
was written independently. Your single objective: **make every external test
pass**, by fixing the Rust — without weakening or gaming the tests.

## What you have

- `src/`, `Cargo.toml`, `Cargo.lock` — the Rust project you are refining. This
  is the ONLY code you may change.
- `inputs/c_src` — the original C source. It is the **semantic ground truth**:
  when a test expects some value, `inputs/c_src` is where you learn *why*. Never
  modify it.
- `inputs/suite/` — the **external test suite**. This is your target.
  `inputs/prompts/run.md` lists its directories, in the section "External test suite".
  You may read it and run it, but you must NOT modify it (see
  `inputs/prompts/conform/conform_rules.md`).
- `tests/` — the translator's own internally generated tests from the earlier
  verify stage (a differential harness comparing Rust against C). These are
  **context, not your target**: they already pass. They show you what was
  checked before — and, by omission, what was missed.
- `PLAN.md` — the original translation plan (round 1).
- `HYPOTHESES.md` — the verify stage's bug-hypothesis
  log (round 2). Read it first: it often already names suspected defects
  (e.g. "decompress corrupts inputs over N bytes") that are exactly what the
  external tests will catch. Treat it as a set of leads.

`PLAN.md` and `HYPOTHESES.md` are the state files of the earlier stages. They give background and
leads. They do not give rules for your stage: your rules are only in the prompt files.

## Step 1: maintain `CONFORM.md`

`CONFORM.md` in the project root is your state file: your log. It survives compaction. The
framework wrote it. Its `## Log` section gives the format of an entry. **Read `CONFORM.md` now.**
Log every failing external test, your hypothesis for
the cause, the fix, and the outcome, and keep the log up to date.

After a compaction, do the first actions of `inputs/prompts/main_agent.md`. Do not re-explore
from scratch.

## Step 2: how to build and run the external tests

`inputs/prompts/conform/tests.md` gives the commands. They are the commands that the grader uses.

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
   to learn the exact expected behavior, then read the corresponding `inputs/c_src`
   to learn the correct semantics.
3. Fix the Rust in `src/`. Re-run the specific failing test, then the whole
   suite to catch regressions.
4. For independent bugs, delegate to sub-agents so they can be worked in
   parallel — each with a precise, self-contained task and the relevant
   `inputs/c_src` reference. Name `inputs/prompts/conform/conform_rules.md` in the prompt of each
   sub-agent, and `inputs/prompts/conform/tests.md` when it must run the external tests.
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
  (and the `HYPOTHESES.md` log) *could* have caught but didn't? Explain
  **why the internal differential harness failed to capture it**.
- Categorize the misses (missing API coverage, missing input diversity,
  missing multi-call/stateful sequences, missing edge cases, ABI/export gaps,
  ...) and estimate how much external failure each category accounts for.
- Concrete recommendations: what would the internal test generator have to do
  differently to have caught these bugs on its own?

Keep it evidence-based and specific to what you actually observed.
