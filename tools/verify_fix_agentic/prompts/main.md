# Verification stage: the main agent

Reader: the main agent of the verification stage. Read this file again after each context
compaction.

Read these files first:

- `inputs/prompts/main_agent.md`
- `inputs/prompts/verify/verify_rules.md`
- `inputs/prompts/cargo_features.md`

## Task

You are testing a C-to-Rust translation for correctness. The C code is the
ground truth — the Rust code must produce byte-identical results.

- `inputs/c_src` contains the original C source code
- `src/` contains the Rust translation

The public surface of the program has one or both of two kinds, and you verify
each kind that it has in the same way:

- **Library**: the exported functions. A call's inputs are its arguments; its
  results are the return value, the output buffers, and `errno`.
- **Executable**: the program that a C file with `main()` builds (the Rust
  executable is `driver`). A run's inputs are argv, stdin, input files, and
  the environment; its results are stdout, stderr, the exit status, and the
  files that it writes.

The concrete mechanism you use to compare C against Rust — how you build each
side and where you write the comparison — is described in the method prompt file
of this run (see **Step 3**). Everything before it (PLAN.md, the HYPOTHESES.md
discipline, the surface files, the rules) applies regardless of which comparison
mechanism you use.

Your state file is `HYPOTHESES.md`, in the current directory (see Step 1).

The rules of this stage are in the prompt files that this file lists above. Read them again
whenever you are unsure — do not work from memory.

## Sub-agent roles

| Role | Prompt file | When |
|---|---|---|
| surface | `inputs/prompts/verify/surface.md` | Step 2: writes one surface file, or does the cfg completeness check |
| fixer | `inputs/prompts/verify/fixer.md` | each fix of the Rust code: a divergence, or a missing entry of `SYMBOLS.md` |
| reviewer | `inputs/prompts/verify/reviewer.md` | the completion gate: the final cross-review |

## Step 0: Read PLAN.md FIRST

A previous translation agent left a file `PLAN.md` in this directory containing
its design notes, parameter tables, decisions, and pitfalls it noticed during
translation. **Before doing anything else**, read `PLAN.md`.

If `PLAN.md` exists, treat it as authoritative background. Do NOT re-derive
project structure, module layout, Cargo features, parameter values, or
design rationale from scratch — that information is already there. Pay
particular attention to the "Notes for future-me" section the translation
agent may have flagged specific concerns (e.g. "macro renames", "padding
edge cases") that point directly at likely bug sites.

`PLAN.md` is the state file of the translation stage, which is complete. It gives
background. It does not give rules for your stage: your rules are only in the
prompt files.

If `PLAN.md` does not exist, the project was small enough that the translator
chose not to write one; proceed without it.

## Step 1: Maintain `HYPOTHESES.md`

Verification work involves forming hypotheses about why the C and Rust outputs
differ, then confirming or refuting them. Your context window is finite and
will be **compacted** when it fills up; after a compaction your memory of which
hypotheses you already investigated is **lost**, leading to "rediscover the
same bug three times" loops that waste an entire run.

To prevent this, you MUST maintain the file `HYPOTHESES.md` in the current
directory as an append-only log of bug hypotheses. The framework wrote it. Its
`## Hypothesis log` section gives the format of an entry. **Read `HYPOTHESES.md`
now** (right after `PLAN.md`). When an earlier verification left entries in the
log, continue that log. A rules section in an earlier `HYPOTHESES.md` is out of date. Your rules
are only in the prompt files.

**Rules of engagement with `HYPOTHESES.md`:**

1. **Every time you form a new hypothesis** (e.g. "I think `foo()` has an
   off-by-one in the padding length"), append a new `## H<N>` entry
   immediately, with status `open`. Do NOT wait until you have proof.
2. **After running a test that bears on a hypothesis**, update its `Status`
   to `confirmed` or `refuted` and write the evidence in `Outcome`. Do NOT
   leave entries stale.
3. **After applying an Edit that you believe fixes a hypothesis**, mark it
   `fixed` and note what you changed.
4. **Before forming a new hypothesis, check if it is already in the file.**
   If so, do not re-investigate it — read its current Status and proceed.
5. The file is for **future-you across your own compactions**, not for
   sub-agents. Do not delegate; you maintain it yourself.
6. **Delegate fixing work aggressively. Your context window is the
   bottleneck — protect it.** Your job as the main agent is to OWN
   HYPOTHESES.md and OWN execution: building C and Rust, running tests,
   running `nm`, comparing C-vs-Rust outputs, deciding which functions
   diverge. Almost everything else — reading large C source files to
   understand an algorithm, locating the matching Rust code, applying
   the actual fix — should go to a sub-agent so neither the C nor the
   buggy Rust ever has to live in YOUR context. Default to delegating;
   only do a fix in-process when it is a one-line change you can apply
   from what you already see.

   Things you keep:
   - HYPOTHESES.md ownership (sub-agents do NOT edit HYPOTHESES.md)
   - Building C / Rust, running cargo test, running nm, output comparison
   - Hypothesis status updates after each test run
   - Per-configuration coverage tracking

   Rule of thumb: if investigating or fixing a hypothesis would require
   reading more than ~200 lines of C or Rust into your own context,
   delegate the fix to a sub-agent and let it report back what it changed.

   Send a fix to a fixer with this prompt:

   ```
   Read inputs/prompts/verify/fixer.md and do the task in it.
   Checkout: <wt/<name> (branch <name>), or the current directory>.
   Problem: <the hypothesis H<N>, the test or the inputs that show it, the C result and the Rust result>.
   Suspected files and functions: <list, or "not known">.
   More prompt files: <the method prompt files of this run, when the fixer must build or run tests>.
   ```
7. **When a hypothesis is tied to a table row or a test, name both in its
   Coverage field.** Example: `Coverage: ERRORS.md #7; Test Init.RejectNull`.
   This is what the final cross-review (see the completion gate) checks, so an
   entry without coverage data cannot be audited later.

### After a compaction

After the first actions of `inputs/prompts/main_agent.md`:

1. Find the first hypothesis with status `open` or `confirmed` (but not yet
   `fixed`). That is your current work item.
2. Resume from its `Action taken` field. Do not redo work already logged.
3. If the very first hypothesis you form already exists with status
   `confirmed` or `fixed`, you are in a thrashing loop — stop, re-read the
   existing entry, and continue from where it left off (e.g. if status is
   `confirmed` but not yet `fixed`, your next action is to apply the fix, not
   re-confirm).

## Step 2: Map the verification surfaces

Before you write any test, produce four files in the working directory.
Together they are the test plan: tests cover their rows, and the completion
gate checks them. Build the C reference and the Rust artifacts first: the
library `.so` and the executable, for each kind the program has. Run the two
executables on the same few inputs: such a smoke test finds gross divergences
before any test exists. Generate
each file with a separate sub-agent. Each sub-agent reads `inputs/c_src` and the
built artifacts, and writes exactly one file. Its prompt has this form:

```
Read inputs/prompts/verify/surface.md and do the task in it.
Your part: <SYMBOLS.md | CONDITIONALS.md | ERRORS.md | CONFIGS.md | the cfg completeness check>.
Built artifacts: <the paths of the C and the Rust library and executable>.
```

| Part | What it maps | Wave |
|---|---|---|
| `SYMBOLS.md` | the entry points: the exported symbols, and the options and commands of the executable | 1 |
| `CONDITIONALS.md` | the conditional compilation: the macros and the conditional regions | 1 |
| `ERRORS.md` | the rejections: each way in which an entry point rejects an input | 2 |
| `CONFIGS.md` | the configurations: the options, modes and input shapes of valid inputs | 2 |
| the cfg completeness check | each region of `CONDITIONALS.md`, checked and corrected in `src/` | 2 |

Dispatch in two waves. Dispatch `SYMBOLS.md` and `CONDITIONALS.md` in
parallel. When `CONDITIONALS.md` is complete, dispatch `ERRORS.md`,
`CONFIGS.md`, and the cfg completeness check in parallel.

The sub-agent of `SYMBOLS.md` reports the diff. It does
not change any code. When the diff is not empty, dispatch a separate
sub-agent (a fixer) to resolve it (add the export or the wiring, or translate the
missing C source). Do not stub a missing entry to empty the diff. A stub
that claims behavior is worse than a missing entry.

Step 3 tells you how to test. Every `CONFIGS.md` row gets a differential test
on valid inputs, and every `ERRORS.md` row gets a rejection test. Also test
the generic boundaries of `inputs/prompts/verify/verify_rules.md`.

A row you cannot test stays in the table, unchecked, with a note. The note
carries evidence: an experiment that did not trigger the row, or a premise
from `CONDITIONALS.md` that shows the branch is inactive. A judgment made
from reading alone is not evidence.

## Step 3: Test and fix

The method prompt file of this run tells how you build both sides, where you write the tests, and
how you run them. `inputs/prompts/run.md` lists it with the additional prompt files for the main
agent. Do what it tells. Name the method prompt files of this run in the prompt of each sub-agent
that builds or runs tests.

## Completion gate

Work through this checklist before you declare verification complete. Passing
tests on the inputs you happened to pick are not completion. Re-read this
list after your last fix.

- [ ] `SYMBOLS.md` has no open missing-entry rows. For the library, the
      `nm -D` symbol diff between the C `.so` and the Rust `.so` is empty.
- [ ] A reviewer compared `HYPOTHESES.md`, the test files, and the
      four surface files. It checked that every row has a test, that every
      test maps back to a row or a logged hypothesis, that every
      `confirmed` or `fixed` hypothesis has a passing test, and that every
      unchecked-row note carries evidence. You resolved what it reported
      before finishing.

Send the reviewer with this prompt:

```
Read inputs/prompts/verify/reviewer.md and do the task in it.
More prompt files: <the method prompt files of this run>.
```
