<!-- markdownlint-disable MD041 -->
Translate the C code in inputs/c_src to Rust that produces **byte-identical output** for the same inputs.
Write Cargo.toml and src/ files in the current directory (NOT in inputs/c_src).

## Step 0: Know the files of this run, yourself, and the context limits

The framework wrote these files for this run. Read them where this prompt tells you to. Do not
copy their text into other files or into prompts.

- `PLAN.md`, in the current directory: your plan file. Its `## Invariants` section holds the rules
  of this run: your role, the model limits, the directories, the Cargo features, the translation
  rules, the completion gate and the git discipline. Its other sections are for you to fill in.
  **Read `PLAN.md` now, before anything else.**
- `inputs/prompts/`: the task files of your sub-agents, one for each role. A task file tells its
  sub-agent what to do, and it contains the rules that the sub-agent needs. The prompt that you
  write for a sub-agent names its task file and gives the facts of its task, not the rules.

| Role | Task file | When |
|---|---|---|
| scout | `SCOUT.md` | 0.2: explores the C code and proposes subtasks |
| translator | `TRANSLATOR.md` | Step 3: translates one subtask; also each fix of code |
| reviewer | `REVIEWER.md` | Step 4: checks the completion gate |

The Invariants name more task files when this run uses them.

Each agent has a finite context window. When the context comes near its limit, the runtime
compacts older turns into a short summary. A compaction loses detail: exact file contents, exact
signatures, and recent reasoning. Therefore:
- For you: keep `PLAN.md` up to date (see 0.3). It is the record that a compaction does not remove.
- For each sub-agent: give it a subtask that fits its limits (see "Translation subtasks" in
  `PLAN.md`), so that it finishes the subtask before a compaction.

After you read `PLAN.md`, you MUST:

### 0.1 Self-assessment

Output the following in your first response:

```
Self-assessment:
- Model: <your model name as best you know it>
- Approximate usable context window: <e.g. 1M tokens>
- Approximate budget rule of thumb: ~4 chars per token; 200k tokens ≈ 800kB of text
- Approximate single-response output token limit: <e.g. 32k tokens>
  (Take both limits from the model limits in the Invariants of `PLAN.md`. If they give none, use
  1M and 32k.)
- Sub-agent model, window, output limit: <the same as yours, unless the Invariants have a
  "Sub-agent model" section>
  (The sub-agent output limit caps how much Rust code a single sub-agent call can write.)
```

The context window and the output token limit are limits of a single agent and a single response.
They decide how you split the work. They do not limit the whole translation. The whole translation
is divided among many sub-agents, and the total work is within the budget of this run.

### 0.2 Send scouts to explore the C code

Scout sub-agents explore `inputs/c_src` for you and propose the subtasks. Do not explore the C code
yourself: your context is for the plan and the build.

1. Measure the code. Do not read it:

   ```
   find inputs/c_src -name '*.c' -o -name '*.h' | xargs wc -l
   ```

2. One scout can explore code of about 50% of its context window. Scouts use the sub-agent model
   from 0.1. If the code is larger, divide the files into groups of that size, one group for each scout.
3. Send all scouts in one message. Give each scout this prompt:

   ```
   Read inputs/prompts/SCOUT.md and do the task in it.
   Scout <n> of <N>. File group: <directories or files of inputs/c_src, or "all">.
   Context window: <sub-agent window from 0.1>. Output token limit: <sub-agent output limit from 0.1>.
   ```

4. Each scout writes `scout/REPORT_<n>.md` and replies with a codebase summary, proposed subtasks,
   and risks.

{SAFE_WRAPPER_DISPATCH}

### 0.3 Fill in `PLAN.md` BEFORE context fills up

`PLAN.md` is your **lifeline across compactions**: after a compaction, you read it again to recover
your state. It is a plain markdown file that you maintain yourself, not the TODO list of a tool.

When the scouts reply, immediately fill in its sections: "Self-assessment" (from 0.1), "Codebase
summary" and "Translation subtasks" (from the scout replies), and later "Notes for future-me".
Each section of `PLAN.md` says what goes in it. Merge the replies of all scouts, and number the
subtasks T1, T2, .... You can split, merge or reorder subtasks for their size and their
dependencies. All subtasks together must cover all of the C source.

**Rules of engagement with `PLAN.md`:**

1. **Fill it in BEFORE your context fills up.** Its sections must be complete
   before the first compaction.
2. **Do not change the `## Invariants` section.** The framework wrote it. The
   other sections (Self-assessment, Codebase summary, subtasks, Notes) you
   fill in from the scout replies and your own analysis. Reason: anything outside
   PLAN.md drifts after compaction; only PLAN.md content reliably comes back.
3. **Update the checkboxes and "Notes for future-me" IMMEDIATELY after any
   work completes** — whether it was you or a sub-agent.
   Do NOT batch updates. A compaction can come at any time. After a
   compaction, work that PLAN.md does not record can be done again.
4. **After every compaction, re-read `PLAN.md` first thing.** Re-read the
   `## Invariants` section in particular and confirm none of your recent
   actions violated it. Then resume from the first unchecked subtask. Do not
   reconstruct state from memory; trust the file.
5. **Delegate aggressively to sub-agents. Your context window is the
   bottleneck of this whole run — protect it.** Your job as the main agent
   is to OWN the plan and OWN compilation (`cargo build`, error triage,
   feature-matrix verification). Everything else — reading C source
   files in detail, writing the corresponding Rust modules, debugging a
   single backend, translating a self-contained primitive — goes to
   a sub-agent so the C code and the new Rust code never have to live in
   YOUR context. Send every subtask to a sub-agent. Do not translate a
   subtask yourself.

{AGENT_BUG_WORKAROUNDS}

   Things you keep:
   - PLAN.md ownership (sub-agents do NOT edit PLAN.md)
   - Cargo.toml / feature-gate decisions, and the module list in `src/lib.rs` or `src/main.rs`
   - Merging the sub-agent branches into `main` and resolving the conflicts
   - Running `cargo build`, routing the resulting errors, and small build fixes
   - The cross-module type/ABI design

   When you delegate to a sub-agent:
   - Read its reply: the files that it changed, the items that it did not
     finish, and the pitfalls that it noticed. An item that is not finished
     is open work: send it to a sub-agent again.
   - Update PLAN.md checkboxes and "Notes for future-me" after the sub-agent
     returns, not before.
   - Size the subtask to fit within the sub-agent's output token limit.
     If a C file is too large for one sub-agent response (estimate: Rust
     output ≈ C lines × 1.2, converted to tokens at ~10 tok/line), split it:
     give the sub-agent a specific function range or module subset, not the
     whole file. A response that goes over the output token limit is cut off.
     This breaks only that response, not the session: the sub-agent must
     do that piece again. If a sub-agent returns with truncated output, the
     task was too large. Split it into smaller pieces for the next attempt.
     Do NOT retry the same task at the same size.
   - Pre-inject dependencies into the sub-agent prompt. Before launching
     a sub-agent, think about what types, constants, or function signatures
     it will need from other modules. Either include the relevant type definitions directly in the
     sub-agent's prompt, or give it the section of `scout/REPORT_<n>.md` that it needs, or give it
     specific `grep` commands. When each sub-agent reads the same 500-line infrastructure file, each
     one uses thousands of tokens on the same input.

   Rule of thumb: delegate all work whose details you do not need for your
   own decisions. Your decisions are the plan, the merges, the routing of
   build errors, and the cross-module design.

### 0.5 Token budget estimation per subtask

For each subtask in `PLAN.md`, do a back-of-envelope estimate before starting
it:

- Input: total bytes of C files the sub-agent will need to read for this subtask (use
  `wc -c` or the line counts you already gathered), divided by ~4 to get tokens.
- Output: rough estimate of Rust lines the sub-agent will write × ~10 tokens/line.
- Tool overhead: each grep/ls/build-error round-trip costs a few hundred to a
  few thousand tokens.

There are **two independent triggers** that require splitting a subtask further:

1. **Context window trigger**: estimated total (input + output + overhead)
   exceeds ~50% of the usable window of the sub-agent.
2. **Output token limit trigger**: estimated Rust output alone exceeds the
   single-response output token limit of the sub-agent (from Self-assessment 0.1).

Either trigger is sufficient to force a split. Better to add three subtasks
than to be compacted mid-write or hit the output cap mid-file.

## Step 1: Prepare each subtask

The scouts explored the project in Step 0. Their reports are in `scout/`. You prepare and dispatch
each subtask. You do not read the C source in detail: the sub-agent of a subtask reads its C code.

Decide these yourself, from the scout reports, and record them in `PLAN.md`:

1. The project type:
   - Has `main()` → needs `[[bin]]` with `name = "driver"`
   - Exports library functions → needs `[lib]` with `crate-type = ["cdylib"]`
   - Both → include both `[lib]` and `[[bin]]` sections
2. ALL backends/variants, if the project has build-time configurability (source file selection,
   options, conditional compilation). Check a detail with a scoped grep
   (`grep -n 'option(' inputs/c_src/CMakeLists.txt`).

Send each subtask to a translator. Its prompt has this form, and no rules:

```
Read inputs/prompts/TRANSLATOR.md and do the task in it.
Checkout: wt/<name> (branch <name>).
Subtask <Tn>: <name>.
C files and functions: <list>.
Rust files to write: <list>.
Dependencies: <types, constants and signatures of other subtasks, or where to find them>.
Scout report sections: <sections of scout/REPORT_<n>.md>.
Renaming macros: <the macros that rename its functions, from the scout reports, or "none">.
Feature names: <from `cargo_features.py names`, or "none">.
```

Add the lines that the Invariants give for special subtasks (for example data tables). Use the
same form, with the same task file, for a sub-agent that fixes code: give the files, the errors or
the findings, and what to change.

## Step 2: Plan the translation

If the project has build-time configurability (CMake cache variables selecting
different source files or parameters), you MUST preserve this using Cargo
features. Plan which source files map to which features, and which subtasks
in `PLAN.md` will own each feature gate, before you send the sub-agents.

The naming contract for those features is the Cargo features invariant of
`PLAN.md`. Write the `[features]` of Cargo.toml yourself, from the output of
`cargo_features.py names`, before you send the sub-agents that use them.

Break the work into phases: shared/core code first, then
each backend or variant, then wire up feature gates. These phases should
already be the subtasks in your `PLAN.md`. Do not re-plan here. Dispatch the
next unchecked subtasks.

## Step 3: Translate

Send every subtask of `PLAN.md` to a sub-agent. Send independent subtasks at the same time, in one
message. After each subtask completes:

1. Merge its branch into `main` (see "Git discipline").
2. Mark the subtask `[x]` in `PLAN.md`.
3. Append any relevant decision/pitfall to "Notes for future-me" in `PLAN.md`.
4. Then start more subtasks.

The rules of this run (your role, C ABI, behavioral fidelity, crate constraints,
read-only inputs, completion gate) are in the `## Invariants` section of `PLAN.md`. They are the
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
4. Prepare and dispatch that subtask (Step 1). Do not read its C files yourself.
5. Resume from there. Do not redo subtasks already marked `[x]`.

## Step 4: Build and check the completion gate

Run `cargo build --release` and fix any errors until it compiles.
If the project has Cargo features, also run the configuration builds of the
Cargo features invariant, and fix them until they compile.
Send a fix that is larger than a small build fix to a sub-agent.

When all subtasks are checked and every build is green, check the main points of the completion
gate (see the Invariants of `PLAN.md`) yourself: the `nm -D` comparison when the project has a library, and a quick
`grep` of `src/` for the stub forms. Then send a review sub-agent with this prompt:

```
Read inputs/prompts/REVIEWER.md and do the task in it.
```

Send the fixes for its findings to translators (Step 1). Do the small build fixes yourself. Merge
the fixes.

Mark the whole plan complete in `PLAN.md`, with a final note, only after the findings are fixed.

Your job ends when the translation passes the completion gate. A separate verification stage runs after
you and does the thorough execution testing. Beyond the one smoke test, do not write test suites or
differential tests here.

{TOOL_WISHLIST}
