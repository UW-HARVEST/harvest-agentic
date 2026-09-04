<!-- markdownlint-disable MD041 -->
You are testing a C-to-Rust translation for correctness. The C code is the
ground truth — the Rust code must produce byte-identical results.

- `c_src/` contains the original C source code
- `src/` contains the Rust translation

The concrete mechanism you use to compare C against Rust — how you build each
side and where you write the comparison — is described in **Step 3** below.
Everything before it (PLAN.md, the HYPOTHESES.md discipline, the surface
files, the invariants) applies regardless of which comparison mechanism you
use.

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

If `PLAN.md` does not exist, the project was small enough that the translator
chose not to write one; proceed without it.

## Step 1: Maintain `HYPOTHESES.md`

Verification work involves forming hypotheses about why the C and Rust outputs
differ, then confirming or refuting them. Your context window is finite and
will be **compacted** when it fills up; after a compaction your memory of which
hypotheses you already investigated is **lost**, leading to "rediscover the
same bug three times" loops that waste an entire run.

To prevent this, you MUST maintain a file `HYPOTHESES.md` in the current
directory as an append-only log of bug hypotheses. Create it at the very
start of your work (right after reading `PLAN.md`) with this template:

```markdown
# Verification Hypotheses Log

This is an append-only log of bug hypotheses I form while verifying the
Rust translation. After every compaction I will read `HYPOTHESES.md` first
thing to recover state.

## Invariants (do not drift across compactions)

These rules govern verification. They must survive every compaction unchanged.

{MODEL_LIMITS}

{RUST_TOOLCHAIN_CONTEXT}

{WORKDIR_BOUNDARY}

### AFTER ANY COMPACTION: read `PLAN.md` and `HYPOTHESES.md` is your first action before anything.

### Ground truth
- The C code is the authoritative reference. Rust outputs must match C
  byte-for-byte (binary stdout AND every public function output).
- If C and Rust diverge, fix Rust. NEVER modify C.

### Cargo features (FRAMEWORK CONTRACT — also enforced at the final build)
- The build harness invokes `cargo build --features <v1>,<v2>,...` using the
  bare lowercase VALUES of the CMake cache variables for each configuration.
- If `Cargo.toml`'s `[features]` section uses prefixed names (e.g.
  `opt_a_foo = []` with no `foo` alias), the harness will fail. That is a
  Cargo.toml bug, not a test-command bug — fix the Cargo.toml so it exposes
  the bare values (either as primary names or as aliases pointing to the
  internal prefixed gates).
- `[lib] name` and `[[bin]] name` MUST use underscores only — NO hyphens.
  Hyphens cause manifest parse failure. Fix Cargo.toml if you see them.

### Boundaries
- Do NOT modify anything in `c_src/`.

### Configuration coverage
- Every configuration listed under "Configurations to verify" in this task
  must be checked. For each one:
    1. Clean and rebuild C with the listed cmake flags.
    2. Rebuild Rust with the matching Cargo features
       (`cargo build --release --no-default-features --features <list>`).
    3. Re-run integration tests and fix any mismatches before moving on.

{ALL_CONFIGURATIONS}

### Surface coverage and completion
- `SYMBOLS.md`, `CONDITIONALS.md`, `ERRORS.md`, and `CONFIGS.md` are the test
  plan. Every `CONFIGS.md` row gets a differential test. Every `ERRORS.md`
  row gets a rejection test that asserts both sides return the same error
  code or sentinel value.
- Every region in `CONDITIONALS.md` is settled: conditional compilation in
  Rust, or a resolution to one branch with recorded evidence.
- An untested row or an inactive-branch judgment carries evidence: an
  experiment, a definition-site grep, or a premise in `CONDITIONALS.md`.
  Reading alone is not evidence.
- Verification is complete only when the `nm -D` symbol diff is empty, every
  row has a passing test under every listed configuration, every
  `CONDITIONALS.md` region is settled, and the final cross-review finds no
  gap. Green tests on inputs you picked yourself are not completion.

### Operational
- Wrap every `cargo build`, `cargo test`, `cmake`, or other long-running
  command in `timeout 600` (or shorter). No single command should run > 600s.
- If a single test takes too long, skip it and move on. Do not get stuck on
  one step.

## Hypothesis log

Format per entry:
### H<N>: <one-line hypothesis>
- Status: open | confirmed | refuted | fixed
- Evidence: <how I think I know>
- Coverage: <ERRORS.md #N / CONFIGS.md #M / CONDITIONALS.md #K / test name; omit when not tied to a row or a test>
- Files/lines suspected: <file path:line>
- Action taken: <Edit/test/none yet>
- Outcome: <what happened after the action>
```

**Rules of engagement with `HYPOTHESES.md`:**

1. **The `## Invariants` section is verbatim.** When you create
   `HYPOTHESES.md`, copy the entire `## Invariants` block from the template
   above byte-for-byte. Do not paraphrase, do not omit, do not reorder. The
   hypothesis log section you fill in as you work; Invariants is fixed text.
   Reason: anything outside HYPOTHESES.md drifts after compaction; only this
   file reliably comes back. Invariants must be byte-stable.
2. **Every time you form a new hypothesis** (e.g. "I think `foo()` has an
   off-by-one in the padding length"), append a new `## H<N>` entry
   immediately, with status `open`. Do NOT wait until you have proof.
3. **After running a test that bears on a hypothesis**, update its `Status`
   to `confirmed` or `refuted` and write the evidence in `Outcome`. Do NOT
   leave entries stale.
4. **After applying an Edit that you believe fixes a hypothesis**, mark it
   `fixed` and note what you changed.
5. **Before forming a new hypothesis, check if it is already in the file.**
   If so, do not re-investigate it — read its current Status and proceed.
6. **After every compaction, `cat HYPOTHESES.md` first thing.** Re-read the
   `## Invariants` section, then the hypothesis log. If the very first
   hypothesis you form already exists with status `confirmed` or `fixed`,
   you are in a thrashing loop — stop, re-read the existing entry, and
   continue from where it left off (e.g. if status is `confirmed` but not
   yet `fixed`, your next action is to apply the fix, not re-confirm).
7. The file is for **future-you across your own compactions**, not for
   sub-agents. Do not delegate; you maintain it yourself.
8. **Delegate fixing work aggressively. Your context window is the
   bottleneck — protect it.** Your job as the main agent is to OWN
   HYPOTHESES.md and OWN execution: building C and Rust, running tests,
   running `nm`, comparing C-vs-Rust outputs, deciding which functions
   diverge. Almost everything else — reading large C source files to
   understand an algorithm, locating the matching Rust code, applying
   the actual fix — should go to a sub-agent so neither the C nor the
   buggy Rust ever has to live in YOUR context. Default to delegating;
   only do a fix in-process when it is a one-line change you can apply
   from what you already see.

{AGENT_BUG_WORKAROUNDS}

   Things you keep:
   - HYPOTHESES.md ownership (sub-agents do NOT edit HYPOTHESES.md)
   - Building C / Rust, running cargo test, running nm, output comparison
   - Hypothesis status updates after each test run
   - Per-configuration coverage tracking

   Rule of thumb: if investigating or fixing a hypothesis would require
   reading more than ~200 lines of C or Rust into your own context,
   delegate the fix to a sub-agent and let it report back what it changed.
9. **When a hypothesis is tied to a table row or a test, name both in its
   Coverage field.** Example: `Coverage: ERRORS.md #7; Test Init.RejectNull`.
   This is what the final cross-review (see the completion gate) checks, so an
   entry without coverage data cannot be audited later.

### Recovery protocol (if you suspect you were just compacted)

Symptoms: you cannot recall what hypothesis you were testing, or your last
turn looks like a summary rather than concrete work. In that case:

1. `cat PLAN.md HYPOTHESES.md` first thing.
2. Find the first hypothesis with status `open` or `confirmed` (but not yet
   `fixed`). That is your current work item.
3. Resume from its `Action taken` field. Do not redo work already logged.

## Step 2: Map the verification surfaces

Before you write any test, produce four files in the working directory.
Together they are the test plan: tests cover their rows, and the completion
gate checks them. Build the C reference and the Rust `.so` first. Generate
each file with a separate sub-agent. Each sub-agent reads `c_src/` and the
built artifacts, writes exactly one file, and gets the working-directory
boundary rules from the Invariants section.

Dispatch in two waves. Dispatch `SYMBOLS.md` and `CONDITIONALS.md` in
parallel. When `CONDITIONALS.md` is complete, dispatch `ERRORS.md`,
`CONFIGS.md`, and the cfg completeness check in parallel. The check may fix
`src/`. The two generators only read `c_src/`. The three do not write to the
same files.

1. `SYMBOLS.md`: the symbol surface. Run `nm -D` on both `.so` files. List
   every public symbol of the C `.so`, and mark every symbol the Rust `.so`
   does not export. For each missing symbol, record which case holds: the
   implementation exists in Rust but is not exported, or the C source was
   never translated. This sub-agent reports the diff. It does not change any
   code. When the diff is not empty, dispatch a separate sub-agent to resolve
   it (add the export, or translate the missing C source). Do not stub a
   missing symbol to empty the diff. A stub that claims behavior is worse
   than a missing symbol.
2. `CONDITIONALS.md`: the conditional-compilation surface. Collect every
   macro that `#if`, `#else` and other variants reference in `c_src/`, deduplicated. Skip
   macros used only in system headers. Classify each macro:
   - feature: the user sets it at build time, with `-D` or a CMake option.
   - sys: the environment fixes it. A direct flag comes from the toolchain
     or the operating system. An indirect flag comes from a measurement,
     such as pointer width or byte order.
   - constant: an in-source unconditional `#define` fixes it.

   | macro | class (feature / sys / constant) | evidence |
   |-------|----------------------------------|----------|

   List every conditional region with its nesting and its activation premise,
   written as an expression over the macros:

   | # | location | activation premise |
   |---|----------|---------------------|

   Every class and every premise names its evidence in the row: a
   definition-site grep hit, or a compile probe that prints the macro value
   under the project build flags. Example: a source has `#ifndef FOO_ALIGN`
   and then `#define FOO_ALIGN 1`. The macro is defined with value 1 by
   default, so an `#if FOO_ALIGN` region is active unless the user overrides
   the macro at build time. Do not decide that a branch is inactive from
   reading alone.
3. `ERRORS.md`: the rejection surface of the C code. Grep the C source for
   every distinct way a function rejects an input: error-return macros,
   `return NULL`, negative sentinels, error enums, `assert`, range checks,
   null checks, and size limits. Write one row per distinct rejection:

   | # | function | trigger (the exact invalid input) | expected C result |
   |---|----------|------------------------------------|-------------------|

   Derive each row from what the C code checks. Do not invent rows and do not
   copy them from documentation alone. Three error branches in one function
   are three rows. When a rejection sits behind a conditional, write its
   premise from `CONDITIONALS.md` in the trigger column. Example row for a
   function that returns NULL on a null argument:
   `| 7 | cfg_parse | s == NULL | NULL |`.
4. `CONFIGS.md`: the configuration surface. It is the mirror of `ERRORS.md`
   for valid inputs. Enumerate the axes the C code branches on. Use the
   feature-class macros from `CONDITIONALS.md` as configuration axes. Cover
   every option, mode, or flag the public API can set. Cover every input
   shape the code special-cases: size classes, empty, one, many, boundary
   values, element types. Cover every public entry point, including the
   lowest-level ones and not only the convenience wrappers. Write one row per
   combination the code treats differently:

   | # | entry point(s) | configuration (options and input shape) | [ ] |
   |---|----------------|------------------------------------------|-----|
5. The cfg completeness check runs as its own sub-agent after
   `CONDITIONALS.md` is complete. For each region, it confirms one of two
   cases in `src/`: a matching conditional exists, or the region is resolved
   to one branch with the resolution recorded in `CONDITIONALS.md`. A region
   that is reachable under some legal build configuration must not be
   dropped. When the check finds a dropped or mistranslated region, it fixes
   `src/` and rebuilds. It is the only sub-agent writing to `src/` at that
   moment.

Step 3 tells you how to test. Every `CONFIGS.md` row gets a differential test
on valid inputs, and every `ERRORS.md` row gets a rejection test. Also test
the generic boundaries even when no row lists them: null pointers, zero and
oversized lengths, values one step past a valid range, and enum arguments
with no valid variant. The C code takes any int where an API declares an
enum, and answers such calls through its default or error branch. Rust must
answer the same way.

A row you cannot test stays in the table, unchecked, with a note. The note
carries evidence: an experiment that did not trigger the row, or a premise
from `CONDITIONALS.md` that shows the branch is inactive. A judgment made
from reading alone is not evidence.

{VERIFICATION_METHOD}

All operational rules (the c_src boundary, per-configuration re-verification,
the 600-second timeout cap) live in the `## Invariants` section of your
`HYPOTHESES.md` template above. Re-read them from `HYPOTHESES.md` whenever you
are unsure — do not work from memory of this prompt.

## Completion gate

Work through this checklist before you declare verification complete. Passing
tests on the inputs you happened to pick are not completion. Re-read this
list after your last fix.

- [ ] The `nm -D` symbol diff between the C `.so` and the Rust `.so` is empty.
      `SYMBOLS.md` has no open missing-symbol rows.
- [ ] A review sub-agent compared `HYPOTHESES.md`, the test files, and the
      four surface files. It checked that every row has a test, that every
      test maps back to a row or a logged hypothesis, that every
      `confirmed` or `fixed` hypothesis has a passing test, and that every
      unchecked-row note carries evidence. You resolved what it reported
      before finishing.

{AGENT_TOOLS_SECTION}

## Static Analysis Tool Wishlist

As you work through verification and fixing, pay attention to moments where you think:
- "If I had a tool that could tell me X, I could skip this lengthy reasoning / exploration."
- "If I had a tool that could do Y, I would have much higher confidence in this fix."

Whenever such a thought arises, **immediately** append one JSON object (on a single line) to
the file `{WISHLIST_PATH}`. Do not wait until the end — record the wish as soon as it occurs,
while the context is fresh. Multiple entries are encouraged; record every distinct need.

Each entry must be a single-line JSON object with exactly these fields:

```
{"category": "...", "description": "...", "language": "...", "soundness": "...", "completeness": "...", "value": 0}
```

Field definitions:
- `category`: `"info_query"` (read-only analysis that answers a question) or `"code_edit"` (a transformation/rewrite tool)
- `description`: plain English description of what the tool does — **no implementation details**, just what it gives you and why it would help
- `language`: `"C"`, `"Rust"`, `"C_and_Rust"`, or another language name
- `soundness`: `"required"` (must never give wrong answers), `"preferred"`, or `"not_needed"` (approximate/heuristic output is fine)
- `completeness`: `"required"` (must cover all cases), `"preferred"`, or `"not_needed"` (partial results are useful enough)
- `value`: integer 0–10 estimating how much this tool would have helped you in this specific task
