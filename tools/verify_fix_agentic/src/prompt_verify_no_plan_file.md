<!-- markdownlint-disable MD041 -->
You are testing a C-to-Rust translation for correctness. The C code is the
ground truth — the Rust code must produce byte-identical results.

- `c_src/` contains the original C source code
- `src/` contains the Rust translation
- The C code can be compiled as a shared library. Look at c_src/CMakeLists.txt
  to understand the build system. Build it with:
  ```
  cd c_src && mkdir -p build && cd build && \
  cmake .. -DCMAKE_POSITION_INDEPENDENT_CODE=ON {CMAKE_BUILD_FLAGS} && \
  cmake --build .
  ```
- Find the resulting .so files in the build output

## Step 0: Track your hypotheses

Verification work involves forming hypotheses about why the C and Rust outputs
differ, then confirming or refuting them. Your context window is finite and
will be **compacted** when it fills up; after a compaction your memory of which
hypotheses you already investigated degrades, which can lead to "rediscover
the same bug three times" loops that waste an entire run.

Track your hypotheses as you work:

- **Every time you form a new hypothesis** (e.g. "I think `foo()` has an
  off-by-one in the padding length"), state it explicitly in your response as
  `H<N>: <one-line hypothesis>` with status `open`, the evidence, and the
  suspected files/lines. Do NOT wait until you have proof.
- **After running a test that bears on a hypothesis**, restate its status as
  `confirmed` or `refuted` with the evidence. Do NOT leave hypotheses stale.
- **After applying an Edit that you believe fixes a hypothesis**, restate it as
  `fixed` and note what you changed.
- **Before investigating a hypothesis, check whether you already investigated
  it.** If you find yourself about to re-derive a conclusion you have already
  stated, stop and continue from where the earlier investigation left off
  (e.g. if a hypothesis was `confirmed` but not yet `fixed`, your next action
  is to apply the fix, not re-confirm it).

## Non-negotiable rules

These rules govern verification. They are not negotiable.

{MODEL_LIMITS}

{RUST_TOOLCHAIN_CONTEXT}

{WORKDIR_BOUNDARY}

### Ground truth
- The C code is the authoritative reference. Rust outputs must match C
  byte-for-byte (binary stdout AND every public function output under
  libloading-based comparison).
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
- Add `libloading = "0.8"` to `[dev-dependencies]` in `Cargo.toml` (so your
  integration tests can dlopen the C shared library).

### Configuration coverage
- Every configuration listed under "Configurations to verify" in this task
  must be checked. For each one:
    1. Clean and rebuild C with the listed cmake flags.
    2. Rebuild Rust with the matching Cargo features
       (`cargo build --release --no-default-features --features <list>`).
    3. Re-run integration tests and fix any mismatches before moving on.

{ALL_CONFIGURATIONS}

### Operational
- Wrap every `cargo build`, `cargo test`, `cmake`, or other long-running
  command in `timeout 600` (or shorter). No single command should run > 600s.
- If a single test takes too long, skip it and move on. Do not get stuck on
  one step.

## Delegate fixing work aggressively

**Your context window is the bottleneck — protect it.** Your job as the main
agent is to OWN the hypothesis tracking and OWN execution: building C and
Rust, running tests, running `nm`, comparing C-vs-Rust outputs, deciding which
functions diverge. Almost everything else — reading large C source files to
understand an algorithm, locating the matching Rust code, applying the actual
fix — should go to a sub-agent so neither the C nor the buggy Rust ever has to
live in YOUR context. Default to delegating; only do a fix in-process when it
is a one-line change you can apply from what you already see.

{AGENT_BUG_WORKAROUNDS}

Things you keep:
- Hypothesis tracking (sub-agents report findings back; you decide each
  hypothesis's status)
- Building C / Rust, running cargo test, running nm, output comparison
- Hypothesis status updates after each test run
- Per-configuration coverage tracking

Rule of thumb: if investigating or fixing a hypothesis would require
reading more than ~200 lines of C or Rust into your own context,
delegate the fix to a sub-agent and let it report back what it changed.

## Verification workflow

Now do the actual verification:

1. Build the C code as a shared library
2. Write Rust integration tests (in tests/) that use `libloading`
   to load the C .so and compare C vs Rust function outputs
3. Start with the lowest-level functions and work upward to higher-level ones.
   Look at the C headers to identify the public API and function call hierarchy.
4. For each function: create fixed test inputs, call both C and Rust versions,
   assert outputs match byte-for-byte
5. Run `cargo test` and investigate any mismatches. Every time a test
   exposes a divergence, state a new hypothesis (see Step 0).
6. When you find a Rust function that produces different output than C,
   fix the Rust code in src/ and re-run until the test passes. Restate the
   matching hypothesis as `fixed` after the Edit.
7. Keep going until all public functions match
8. If the project has a main binary, run both the C binary and the Rust binary
   with the same inputs and compare their stdout byte-for-byte. Fix any differences.
9. Compare `nm -D` on the C .so and the Rust .so. Every symbol the C .so
   exports, the Rust .so must also export with the exact same name. This
   includes symbols created by preprocessor macros. If the C .so exports it,
   the Rust .so must export it — no exceptions. Add missing exports.

All operational rules (libloading dev-dep, c_src boundary, per-configuration
re-verification, the 600-second timeout cap) live in the "Non-negotiable
rules" section above. Re-read them whenever you are unsure.


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

## Git discipline
{GIT_DISCIPLINE}
