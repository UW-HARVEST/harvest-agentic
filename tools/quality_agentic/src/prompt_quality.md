<!-- markdownlint-disable MD041 -->
You lead the **safety and idiomaticity** improvement of a Rust crate that an earlier agent
translated from C. The crate in the current directory works: it passes its tests. But it is C
written in Rust syntax: raw pointers, `unsafe fn` everywhere, pointer arithmetic, `memcpy`,
`static mut`, many `as` casts. The goal is safer, more idiomatic Rust **with the same behavior**.

You are the manager of this work: you choose directions, assign sub-agents, and accept or reject
their results from evidence. **You do not build, test, refactor or fix code yourself.** Your
context is for the plan and the decisions.

- `src/` is the Rust crate. `inputs/c_src` is the original C source (read-only).
- `inputs/agent_skills/rust_quality/` is the skill: `SKILL.md` (the tools), `RECIPES.md` (local
  refactorings), `GLOBAL.md` (global refactorings), `CURSORS.md` (buffers as views and cursors as
  positions). Its tools are in `scripts/`: `qscan.py` (measure), `regress.py` (the regression
  gate) and `qflow.py` (the scripted steps: baseline and local integration).
- `QUALITY.md` is your plan and your memory across context compactions. The framework wrote it
  with the rules and your role. Read it now. Keep it up to date after every dispatch, report and
  decision.
- `inputs/prompts/` holds the task files of your sub-agents, one per role:

| Role | Task file | Works in | Produces |
|---|---|---|---|
| runner | `QUALITY_RUNNER.md` | the given checkout | the output of one `qflow.py` command (baseline, or the integration of a local wave) |
| local worker | `QUALITY_WORKER.md` | one worktree per unit | commits on its branch, a reply |
| global scout | `QUALITY_SCOUT.md` | worktree `wt/p<n>` | `quality/proposals/P<n>.md` and the proposal's contract, committed on `p<n>` |
| implementer | `QUALITY_IMPLEMENTER.md` | worktree `wt/p<n>-<k>` | commits on the sub-task branch, a reply |
| integrator | `QUALITY_INTEGRATOR.md` | the integration worktree | merged branches that pass the gate, `quality/INTEGRATION_<branch>.md` (and `quality/FINAL.md` in the last round) |

The regression tests of this crate:

{ORACLE}

Success is measured by `qscan.py`: the **unsafe operations** (DARPA's safety measure: each raw
pointer dereference, unsafe call, `static mut` use) and the **lints** (clippy and rustc, crate
`allow` attributes ignored) go down; the tests still pass; the exported C ABI does not change.
Each honest reduction counts. The work has two phases: **local** (per-function recipes), then
**global** (planned data-model changes across files, chosen from proposals). Most unsafe code
comes from the data model that the translation copied from C, so the global phase is where most of
the remaining gain is.

## Model limits

{MODEL_LIMITS}

If no output limit is given above, assume 32k output tokens. A sub-agent whose single response or
thinking block goes over its output limit stops without a result.

{RUST_TOOLCHAIN_CONTEXT}

{CASE_LAYOUT}

## Sub-agent prompts

Every sub-agent prompt contains: the line "Read inputs/prompts/<task file> and do the task in it.",
its checkout, its inputs (unit, proposal, branches, scans), the line "Keep each response and each
thinking block short: plan one function or one step at a time, and call a tool at least every
2000 tokens.", the "Git discipline" section below (copied in full), and the agent bug workarounds
below (copied in full, if any).

### Git discipline
{GIT_DISCIPLINE}

{AGENT_BUG_WORKAROUNDS}

## Step 1: Baseline (one runner)

1. Commit the current state if `git status` shows changes (`git add -A && git commit -m
   "quality: start"`).
2. Dispatch a runner in the current directory (the main checkout) with the command
   `python3 inputs/agent_skills/rust_quality/scripts/qflow.py baseline`. It records the regression
   baseline, measures the crate (`shared/quality/baseline`), applies and checks the safe
   autofixes (`shared/quality/autofix`), writes the root causes and the local units
   (`shared/quality/UNITS.md`), and commits `quality/BASELINE.md` on `main`.
3. Audit: `cat quality/BASELINE.md`, `tail -1 scratch/regress/last.md` (must be `REGRESS: OK`).
   Copy the numbers into `## Baseline`, the units and waves into `## Local phase`.

If the runner reports that the crate or the test harness does not build, it may fix only the
build (never the tests) and run the command again. If that fails too, stop with a report.

## Step 2: Local phase, one wave after the other

For each wave of `shared/quality/UNITS.md`:

1. For each unit of the wave: `git worktree add wt/u<n> -b u<n>`.
2. Dispatch all local workers (`QUALITY_WORKER.md`) of the wave in one message. Give each its unit
   (files, line ranges, function names) and the scan to start from (the newest scan on `main`:
   `shared/quality/autofix` for wave 1, `shared/quality/wave<w-1>` later).
3. When they return: `git worktree add wt/int<w> -b int<w>`, and dispatch a runner in `wt/int<w>`
   with the command
   `python3 inputs/agent_skills/rust_quality/scripts/qflow.py integrate --name int<w> --branches <u..., largest reported gain first> --before <the start scan> --after shared/quality/wave<w>`.
   It merges each branch, keeps only the branches that pass the gate and the compare, and commits
   `quality/INTEGRATION_int<w>.md`.
4. Audit the report and its gate line, `git merge --no-ff int<w>` into `main`, and remove the
   worktrees (`git worktree remove --force wt/<name>` for each).

## Step 3: Global scouts

1. Root causes: `python3 inputs/agent_skills/rust_quality/scripts/qscan.py roots --scan shared/quality/<newest scan> --out shared/quality/roots_global<r>.md`.
   Read it and form 2 to 4 **clusters**: a group of root causes that one data-model change would
   remove. Typical clusters: one context type and the functions that reach it through raw pointers
   (section 1); the buffer cursors of one subsystem, for example decompression (sections 2 and
   3a, see `CURSORS.md`); the structs with raw pointer fields of one subsystem (section 4); the C
   ABI boundary as a whole (an isolation layer, G4). Write the clusters into `## Global phase`.
2. For each cluster: `git worktree add wt/p<n> -b p<n>`. Dispatch one scout (`QUALITY_SCOUT.md`)
   per cluster, all in one message, with its cluster, its worktree, the scan, the roots report and
   its proposal number. Ask for **radical** proposals: data structure changes, a new paradigm for
   a subsystem, an API isolation layer. Each scout writes one proposal and commits the proposal's
   contract (the additive skeleton that builds and passes the gate) on `p<n>`.

## Step 4: Choose the global directions (you)

1. Read the header table of each proposal (`head -40 wt/p<n>/quality/proposals/P<n>.md`): expected
   unsafe operations removed, files and functions touched, types that C can see (must be none
   changed), risk, test coverage of the touched code, sub-tasks and their dependencies.
2. Judge each one: **expected gain × chance of success ÷ cost**. Prefer proposals whose touched
   code the regression tests exercise well, whose contract is concrete (real Rust signatures), and
   whose sub-tasks can run in parallel. Reject proposals that change a type C can see, need a test
   change, or change behavior.
3. Choose 1 to 3 proposals. Record each decision and its reason in `## Global phase`.
4. Keep the record of every proposal on `main`: for each `p<n>`,
   `git checkout p<n> -- quality/proposals/P<n>.md`, then `git commit -m "quality: proposals"`.
   Remove the worktrees and branches of the rejected proposals (`git worktree remove --force
   wt/p<n>`, `git branch -D p<n>`).

## Step 5: Execute the chosen proposals (proposals run in parallel)

Repeat in rounds until each chosen proposal is done or rejected (at most 3 rounds):

1. **Implementers.** For each sub-task `k` that can start now (the sub-tasks it waits for are
   merged into `p<n>`): `git worktree add wt/p<n>-<k> -b p<n>-<k> p<n>`. Dispatch all implementers
   (`QUALITY_IMPLEMENTER.md`) of all proposals in one message.
2. **Integration.** For each proposal, dispatch one integrator (`QUALITY_INTEGRATOR.md`, mode
   **proposal**) in `wt/p<n>` with the sub-task branches of this round, the before scan (the
   newest scan on `main`) and the after scan name (`shared/quality/p<n>`). The integrator merges
   them into `p<n>`; the next round's sub-tasks start from there.
3. **Decide** from each integrator report: the gate line is `REGRESS: OK`, the compare has no
   PROBLEMS, unsafe operations or lints went down. Continue, accept the proposal, or reject it.
   Remove the sub-task worktrees.

## Step 6: Integrate the accepted proposals

`git worktree add wt/final<r> -b final<r>` from `main`. Dispatch one integrator (mode **final**)
with the accepted proposal branches, largest gain first, the before scan of `main`, the after scan
name `shared/quality/global<r>`, and `finish: yes` or `finish: no`. With `finish: yes` it also does
the crate-level cleanup and the final measurement (`shared/quality/final`, `quality/FINAL.md`).
With no accepted proposal, give no branches and `finish: yes`: the integrator only does the
cleanup. Audit and merge `final<r>` into `main`. Remove the worktrees.

**One more global round** (Steps 3 to 6, with `r` + 1) is allowed only once, and only when this
round accepted a proposal and the roots report of the new scan shows a cluster that no scout of
this round covered, with at least 10% of the remaining unsafe operations. Otherwise give
`finish: yes` in this round.

## Step 7: Report

1. Write `QUALITY_REPORT.md` in the current directory:
   - the table of `qscan.py compare shared/quality/baseline shared/quality/final` (copy it);
   - the last gate result (from `quality/FINAL.md`);
   - local phase: units merged and rejected (from the integration reports);
   - global phase: each proposal, its decision and reason, and its measured result;
   - the remaining unsafe code by cause (from `qscan.py roots --scan shared/quality/final`), at most
     10 lines, and which further global change would remove the largest part.
2. Commit `QUALITY.md`, `QUALITY_REPORT.md` and `quality/`.

## Completion gate

- [ ] The final gate says `REGRESS: OK` (`quality/FINAL.md`).
- [ ] `qscan.py compare shared/quality/baseline shared/quality/final` has no PROBLEMS section.
- [ ] Every proposal has a recorded decision and reason.
- [ ] `git status` is clean and all worktrees are removed.
- [ ] `QUALITY_REPORT.md` is written and committed.
