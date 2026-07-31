<!-- markdownlint-disable MD041 -->
Translate the C code in c_src/ to Rust that produces **byte-identical output** for the same inputs.
Write Cargo.toml and src/ files in the current directory (NOT in c_src/).

## Step 0: Know yourself and plan for context limits

You are a model with a **finite context window** (typically 200k tokens or 1M tokens). When your context approaches its limit, the
runtime will automatically **compact** older turns into a lossy summary. After a
compaction:
- You retain a high-level idea of what you were doing
- You **lose** exact file contents, exact function signatures, and the fine-grained
  reasoning that translation depends on
- Re-reading the same files burns more context and can trigger another compaction

**Translation is uniquely sensitive to compaction.** Unlike bug-fixing, translation
requires line-level fidelity to the source. A summary like "I read hash.c (140
lines)" is useless — you need the actual code to translate it. Therefore: **a single
function/module's translation must be completed between two compactions**, or it
will degrade.

Before doing anything else, you MUST:

### 0.1 Self-assessment

Output the following in your first response:

```
Self-assessment:
- Model: <your model name as best you know it>
- Approximate usable context window: <e.g. 200k tokens>
- Approximate budget rule of thumb: ~4 chars per token; 200k tokens ≈ 800kB of text
- Approximate single-response output token limit: <e.g. 16k tokens>
  (If you do not know your output limit, use a conservative estimate of 16k.
   This limit caps how much Rust code a single sub-agent call can write.)
```

### 0.2 Cheap codebase reconnaissance (NO bulk reading)

Estimate the codebase size **without reading file contents**. Prefer cheap
shell-level operations:

```
ls -la c_src/                                  # what files exist
find c_src/ -name '*.c' -o -name '*.h' | xargs wc -l    # line counts
du -sh c_src/                                  # total size
```

From line counts and file sizes, decide which regime you are in:

- **Small (total source < 30% of your window)**: safe to read everything once and
  translate top-down. Skip the rest of Step 0 and proceed to Step 1.
- **Large (> 30% of window)**: full read is impossible. You MUST segment before
  reading anything substantial. Use targeted exploration (Step 0.3) to build a
  picture of the codebase from outlines, not full contents.

Write your decision into your first response so it is auditable.

### 0.3 Lightweight exploration tools — use these instead of reading whole files

You have a lot of cheap, surgical tools at your disposal. Prefer them over
opening entire files when you only need a fact, a name, or a signature:

- **`grep` / `rg`**: search for `struct foo`, `typedef`, `^[a-z_]+(`, `#define`,
  function definitions, etc. Scope it (`grep -rn 'struct sphincs_ctx' c_src/`).
- **`head` / `tail` / `sed -n 'A,Bp'`**: read just the first 50 lines of a header
  to see the public API; read just lines 120–180 of a `.c` file to see one
  function. You do NOT have to use Read on a whole file.
- **`ls`, `find`, `wc -l`, `du`**: file inventory, sizes, line counts.
- **`callgraph`** (see "Available Tools" below if present): for any C project
  with `compile_commands.json`, this gives you the whole-program call graph
  without reading a single source file. Use `list` for a flat overview, `from`
  to drill into one function's transitive callees, optionally with `--depth N`.
- **`symscan`** (see "Available Tools" below if present): for finding all
  definitions/uses of a symbol across the project.

Strategy: **build a high-level mental map first** (what files, what symbols, who
calls whom), then read full contents only for the module you are about to
translate. This keeps each translation subtask within a single uncompacted
window.

### 0.4 Write a persistent plan to `PLAN.md` BEFORE context fills up

If you decided you are in the Medium or Large regime, immediately create a file
`PLAN.md` in the current directory (the Rust project root, NOT inside `c_src/`).
This file is your **lifeline across compactions**: future-you, after a
compaction, will re-read this file to recover state. It is **not** a tool's TODO
list — it is a plain markdown file you maintain yourself, so it survives any
amount of context loss.

`PLAN.md` MUST contain (and you MUST keep up to date):

```markdown
# Translation Plan

## Self-assessment
- Model: ...
- Window: ...
- Output token limit: ... (unknown → use 16k as conservative default)
- Regime: small | medium | large

## Invariants (do not drift across compactions)

These rules are not negotiable and must survive every compaction unchanged.
When in doubt, re-read this section.

{MODEL_LIMITS}

{RUST_TOOLCHAIN_CONTEXT}

{WORKDIR_BOUNDARY}

### AFTER ANY COMPACTION: `cat PLAN.md` is your FIRST action before anything.

### Cargo features
- Feature names exposed to the build harness are the **bare lowercase VALUE**
  of each CMake cache variable, NOT the variable name nor a prefix-decorated
  form. The harness invokes
  `cargo build --features <value1>,<value2>,...` with those bare values.
- Suppose the CMake cache has `OPT_A=foo`, `OPT_B=bar`, `OPT_C=2k` (a value
  starting with a digit).
  RIGHT — bare values directly:
      [features]
      foo = []
      bar = []
      "2k" = []
  ALSO ACCEPTABLE — prefixed gate + bare alias (useful when Cargo dislikes
  a bare name; the alias keeps the harness contract intact):
      [features]
      opt_c_2k = []
      "2k" = ["opt_c_2k"]
  WRONG — prefixed without an alias to the bare value:
      opt_a_foo = []           # NO — harness passes `--features foo,bar,2k`
      opt_b_bar = []           # NO — and gets "package does not contain
      opt_c_2k  = []           # NO   these features"
- ALL feature combinations must compile (`cargo build --release --features <combo>`).

### Cargo manifest target names
- `[lib] name` and `[[bin]] name` MUST use underscores only — NO hyphens.
  Hyphens in target names cause `cargo` to fail parsing the manifest entirely.
  RIGHT: `name = "sphincs_plus"`, WRONG: `name = "sphincs-plus"`.

### C ABI
- Public C exports use `#[unsafe(no_mangle)]` and `extern "C"` with exact C
  signatures (use `*const c_char`, `c_int`, etc. from `std::ffi`).
- The exported symbol name is the FINAL linker symbol after all preprocessor
  renames. If C has `#define foo NAMESPACE(foo)` producing `PREFIX_foo`, the
  Rust export is named `PREFIX_foo`, not `foo`.
- Export the ENTIRE public symbol surface: every non-`static` function the C
  shared library exports needs a matching Rust export — including functions
  no test or caller in the repo appears to use. Completeness is verified with
  `nm -D` against the C build; a missing symbol is a test failure.

### Behavioral fidelity
- Do NOT fix bugs in the original C. Reproduce behavior exactly.
- Preserve the exact order of error checks and validation.
- Match C's stdin reading semantics (scanf reads across newlines; fgets does not).
- Match C's exact printf format including spacing and newlines.

### Crate constraints
- Do NOT use the `openssl` crate or any OpenSSL bindings. Use pure-Rust crates.
- Prefer safe Rust internally; do not relax the C ABI on exports.

### Boundaries
- Do NOT modify anything in `c_src/`.

### Translation fidelity
- You MUST faithfully translate ALL C source files to **pure Rust**. Do NOT use
  the `cc` crate (or any equivalent) in `build.rs` to compile or link the
  original C source code. The C source files in `c_src/` will NOT exist in
  the final test environment — the only code available at test time is the
  Rust you write. Any attempt to wrap C via a compiled static archive or
  object file will fail at test time because the C files simply won't be
  there.
- Do NOT import or depend on any existing Rust crate that implements, wraps,
  re-exports, or compiles the same C library you are translating. Every line
  of Rust code must be written by you. If a function needs to call out to
  system libraries (e.g. POSIX APIs), use `libc` or equivalent thin FFI crates,
  not crates that compile the library you are meant to translate.
- A `build.rs` is allowed for legitimate build-time needs (code generation,
  feature detection, etc.), but it must NOT reference, compile, or link any
  file under `c_src/`.
- No shortcuts: every function, every struct, every constant, every macro in
  the C source must become a corresponding Rust implementation. Stub
  functions that return 0 or a hardcoded value are NOT acceptable unless the
  function's return value is defined by the API contract as a compile-time
  constant.

{AGENT_TOOLS_SECTION}

## Codebase summary
- Files: <one line per .c/.h with line count + 1-line role>
- Project type: bin / lib / both
- Build configurability: <Cargo features needed, if any>
- Public API surface: <list of public functions/types>

## Translation subtasks
Each subtask must satisfy TWO constraints:
1. **Context window**: the subtask's combined input (C source to read) + output
   (Rust code to write) + tool overhead must fit within ONE uncompacted context
   window. A safe rule: the total token count (C source + Rust output + tool
   calls) for a single subtask should not exceed **30%** of your usable context
   window. If it would, split the subtask further.
2. **Output token limit**: the Rust code a sub-agent writes in a single response
   must fit within the single-response output token limit (see Self-assessment
   above). **Any C file or section exceeding ~1000 lines is very likely to
   exceed the output limit.** There are two strategies to handle this:
   - **Preferred**: split at the plan level — assign different function groups
     or line ranges of the same file to different subtasks/sub-agents.
   - **Fallback**: instruct the sub-agent explicitly to write the Rust file in
     multiple smaller Write calls, rather than attempting one giant write. Even if the
     context window can hold the entire file at once, the output token limit
     still applies to each individual response.

Subtask boundaries do NOT need to align with file boundaries. A large C file
can be split into multiple subtasks by function group (e.g. "translate
LZ4_compress_default and its callees" vs "translate LZ4_compress_HC and its
callees"), as long as each subtask has well-defined inputs and outputs.

Use the **call graph** to decide boundaries: group functions that call each
other into the same subtask; split at natural call-graph boundaries where
cross-module dependencies are minimal. Functions in different C files that
call each other can still belong to the same subtask if that reduces
cross-subtask coordination.

A subtask is something future-you (after compaction) can pick up by
reading just PLAN.md plus the listed C files/functions.

- [ ] T1: <name> — files/functions: <list> — estimated output: ~Nk tokens — depends on: <other Tx>
- [ ] T2: ...
- [x] T3: ...   <!-- mark done as you go -->

If the project includes a test harness entry
point that is not part of the original library, plan to translate it early.

## Notes for future-me (post-compaction)
- Decisions already made and why
- Cargo features chosen and what they gate
- Pitfalls noticed (e.g. macro renames, namespace prefixes)
- Where you stopped and what to do next
```

**Rules of engagement with `PLAN.md`:**

1. **Write it BEFORE your context fills up.** The whole point is that it must
   exist before the first compaction. If you wait, it will be too late.
2. **The `## Invariants` section is verbatim.** When you create PLAN.md, copy
   the entire `## Invariants` block from the template above byte-for-byte. Do
   not paraphrase, do not omit a rule, do not reorder. The other sections
   (Self-assessment, Codebase summary, subtasks, Notes) you fill in based on
   your analysis — but Invariants is fixed text. Reason: anything outside
   PLAN.md drifts after compaction; only PLAN.md content reliably comes back.
   Invariants must be byte-stable across the whole run.
3. **Update the checkboxes and "Notes for future-me" IMMEDIATELY after any
   work completes** — whether it was you or a sub-agent.
   Do NOT batch updates. Compaction can hit at any moment,
   so every second of unrecorded progress is may lead to work being redone after a compaction.
4. **After every compaction, re-read `PLAN.md` first thing.** Re-read the
   `## Invariants` section in particular and confirm none of your recent
   actions violated it. Then resume from the first unchecked subtask. Do not
   reconstruct state from memory; trust the file.
5. **Delegate aggressively to sub-agents. Your context window is the
   bottleneck of this whole run — protect it.** Your job as the main agent
   is to OWN the plan and OWN compilation (`cargo build`, error triage,
   feature-matrix verification). Almost everything else — reading C source
   files in detail, writing the corresponding Rust modules, debugging a
   single backend, translating a self-contained primitive — should go to
   a sub-agent so the C code and the new Rust code never have to live in
   YOUR context. Default to delegating; only do a subtask in-process when
   it genuinely depends on shared state you already hold.

{AGENT_BUG_WORKAROUNDS}

   Things you keep:
   - PLAN.md ownership (sub-agents do NOT edit PLAN.md)
   - Cargo.toml / feature-gate decisions
   - Running `cargo build` and routing the resulting errors
   - The cross-module type/ABI design

   When you delegate to a sub-agent:
   - Each sub-agent must report back what files it created/modified and any
     pitfalls it noticed.
   - Update PLAN.md checkboxes and "Notes for future-me" after the sub-agent
     returns, not before.
   - Size the subtask to fit within the sub-agent's output token limit.
     If a C file is too large for one sub-agent response (estimate: Rust
     output ≈ C lines × 1.2, converted to tokens at ~10 tok/line), split it:
     give the sub-agent a specific function range or module subset, not the
     whole file. A sub-agent that hits the output cap mid-write produces an
     incomplete file and wastes the entire run. If a sub-agent returns with
     truncated output, treat it as a signal that the task was too large —
     split it into smaller pieces on the next attempt, do NOT retry the same
     task at the same size.
   - Pre-inject dependencies into the sub-agent prompt. Before launching
     a sub-agent, think about what types, constants, or function signatures
     it will need from other modules. Either include the relevant type definitions directly in the
     sub-agent's prompt, or instruct it to search with specific `grep` commands
     rather than reading entire files. Every sub-agent that independently reads
     a 500-line infrastructure file wastes thousands of tokens on redundant I/O.

   Rule of thumb: if a subtask would require reading more than ~200 lines
   of C into your own context, delegate it.

### 0.5 Token budget estimation per subtask

For each subtask in `PLAN.md`, do a back-of-envelope estimate before starting
it:

- Input: total bytes of C files you will need to read for this subtask (use
  `wc -c` or the line counts you already gathered), divided by ~4 to get tokens.
- Output: rough estimate of Rust lines you'll write × ~10 tokens/line.
- Tool overhead: each grep/ls/build-error round-trip costs a few hundred to a
  few thousand tokens.

There are **two independent triggers** that require splitting a subtask further:

1. **Context window trigger**: estimated total (input + output + overhead)
   exceeds ~50% of your remaining usable window.
2. **Output token limit trigger**: estimated Rust output alone exceeds your
   single-response output token limit (from Self-assessment 0.1).

Either trigger is sufficient to force a split. Better to add three subtasks
than to be compacted mid-write or hit the output cap mid-file.

## Step 1: Analyze BEFORE writing any code

You have already done your reconnaissance in Step 0. Now, **for the subtask you
are about to start** (or for the whole project if you decided you are in the
Small regime):

1. Read only the C files this subtask actually needs. For headers, prefer
   reading just the public-API portion (e.g. `head -100 c_src/foo.h`) unless
   you need the macros below.
2. Read `c_src/CMakeLists.txt` to understand source file selection and
   build-time configurability (cache variables, options, conditional
   compilation). If your subtask only touches a small slice, scoped grep
   (`grep -n 'option(' c_src/CMakeLists.txt`) is often enough.
3. Pay attention to preprocessor macros that RENAME functions across the
   project (e.g. `#define foo NAMESPACE(foo)`). These affect the linker symbol
   you will emit in Rust.
4. Determine the project type (record this in `PLAN.md` if not already there):
   - Has `main()` → needs `[[bin]]` with `name = "driver"`
   - Exports library functions → needs `[lib]` with `crate-type = ["cdylib"]`
   - Both → include both `[lib]` and `[[bin]]` sections
5. Identify ALL backends/variants if the project has build-time configurability
   (this is project-wide; do it once, in Step 0).

## Step 2: Plan the translation

If the project has build-time configurability (CMake cache variables selecting
different source files or parameters), you MUST preserve this using Cargo
features. Plan which source files map to which features, and which subtasks
in `PLAN.md` will own each feature gate, before writing code.

The exact naming contract for those features lives in the `## Invariants`
section of your `PLAN.md` template above (and, after Step 0.4, in `PLAN.md`
itself). Do not restate the rule from memory — re-read it from PLAN.md
whenever you touch `[features]` in Cargo.toml.

For large projects, break the work into phases: shared/core code first, then
each backend or variant, then wire up feature gates. These phases should
already be the subtasks in your `PLAN.md` — do not re-plan here, just execute
the next unchecked subtask.

## Step 3: Translate

Translate according to `PLAN.md`, preferably multiple sub-agents for parallelizable tasks. After
each subtask completes:

1. Mark the subtask `[x]` in `PLAN.md`.
2. Append any relevant decision/pitfall to "Notes for future-me" in `PLAN.md`.
3. Then start more subtasks.

The translation rules (C ABI, behavioral fidelity, crate constraints, c_src/
boundary) are in the `## Invariants` section of `PLAN.md`. They are the
authoritative source — if you are unsure about a rule, `cat PLAN.md` and
re-read Invariants. Do not work from memory of this prompt, because this
prompt drifts after compaction; PLAN.md does not.

### Recovery protocol (if you suspect you were just compacted)

Symptoms: you cannot recall what you just did, or your last assistant turn looks
like a summary rather than concrete work. In that case:

1. `cat PLAN.md` first thing.
2. Re-read the `## Invariants` section. Confirm your most recent code touches
   did not violate any invariant (especially feature naming and C ABI).
3. Find the first unchecked subtask. That is your current work item.
4. Read only the C files that subtask requires (per `PLAN.md`).
5. Resume from there. Do not redo subtasks already marked `[x]`.

## Step 4: Compile check

Run `cargo build --release` and fix any errors until it compiles.
If the project has Cargo features, verify ALL feature combinations compile:
run `cargo build --release --features <combo>` for each one. The exact feature
names to test are the bare lowercase CMake cache values (see Invariants).

Once the build is green and all subtasks are checked, mark the whole plan
complete in `PLAN.md` with a final note.

Your job ends when every feature combo's `cargo build` is green.
A separate verification agent runs after you and owns ALL execution-based
correctness checking. Doing that work here wastes turns. Trust the next agent. Stop at green compile.

## Static Analysis Tool Wishlist

As you work through this translation, pay attention to moments where you think:
- "If I had a tool that could tell me X, I could skip this lengthy file reading / reasoning."
- "If I had a tool that could do Y, I would have much higher confidence in this translation step."

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
