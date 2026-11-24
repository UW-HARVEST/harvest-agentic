# Translation stage: the main agent

Reader: the main agent of the translation stage. Read this file again after each context
compaction.

Read these files first:

- `inputs/prompts/main_agent.md`
- `inputs/prompts/translate/translation_rules.md`
- `inputs/prompts/translate/completion_gate.md`
- `inputs/prompts/cargo_features.md`

## Task

Translate the C code in `inputs/c_src` to Rust that produces **byte-identical output** for the same inputs.
Write `Cargo.toml` and `src/` files in the current directory (NOT in `inputs/c_src`).

Your state file is `PLAN.md`, in the current directory. The framework wrote its sections. You
fill them in (see 0.3). **Read `PLAN.md` now.**

The rules of this stage are in the prompt files that this file lists above. They are the
authoritative source. If you are unsure about a rule, read its file again. Do not work from
memory, because memory drifts after a compaction.

## Your role in this stage

- You own the plan, the merges into `main` and the build (`cargo build`, error triage,
  feature-matrix verification).
- Sub-agents read the C source and write the Rust code. You send every subtask to a sub-agent.
  You do not translate a subtask yourself.
- The translation ends only when it passes the completion gate
  (`inputs/prompts/translate/completion_gate.md`).

Things you keep:
- `PLAN.md` ownership (sub-agents do NOT edit `PLAN.md`)
- Cargo.toml / feature-gate decisions, and the module list in `src/lib.rs` or `src/main.rs`
- Merging the sub-agent branches into `main` and resolving the conflicts
- Running `cargo build`, routing the resulting errors, and small build fixes
- The cross-module type/ABI design

**Delegate aggressively to sub-agents.** Everything else — reading C source files in detail,
writing the corresponding Rust modules, debugging a single backend, translating a self-contained
primitive — goes to a sub-agent so the C code and the new Rust code never have to live in YOUR
context.

Rule of thumb: delegate all work whose details you do not need for your own decisions. Your
decisions are the plan, the merges, the routing of build errors, and the cross-module design.

## Sub-agent roles

| Role | Prompt file | When |
|---|---|---|
| scout | `inputs/prompts/translate/scout.md` | 0.2: explores the C code and proposes subtasks |
| translator | `inputs/prompts/translate/translator.md` | Step 3: translates one subtask; also each fix of code |
| reviewer | `inputs/prompts/translate/reviewer.md` | Step 4: checks the completion gate |

The additional prompt files of this run (see `inputs/prompts/main_agent.md`) name more roles and
more prompt files when this run uses them.

## Step 0: Know yourself, the limits, and the code

### 0.1 Self-assessment

Output the following in your first response:

```
Self-assessment:
- Model: <your model name as best you know it>
- Approximate usable context window: <the context window in inputs/prompts/run.md>
- Approximate budget rule of thumb: ~4 chars per token; 200k tokens ≈ 800kB of text
- Approximate single-response output token limit: <the output limit in inputs/prompts/run.md>
- Sub-agent model, window, output limit: <from inputs/prompts/run.md>
  (The sub-agent output limit caps how much Rust code a single sub-agent call can write.)
```

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
   Read inputs/prompts/translate/scout.md and do the task in it.
   Scout <n> of <N>. File group: <directories or files of inputs/c_src, or "all">.
   Context window: <sub-agent window from 0.1>. Output token limit: <sub-agent output limit from 0.1>.
   <the lines that the additional prompt files of this run give for this scout, or none>
   ```

   The additional prompt files of this run can give more lines for the prompt of a scout. They
   can also give more sub-agents to send in the same message as the scouts. Add these lines and
   these sub-agents here.

4. Each scout writes `scout/REPORT_<n>.md` and replies with a codebase summary, proposed subtasks,
   and risks.

### 0.3 Fill in `PLAN.md` BEFORE context fills up

`PLAN.md` is your **lifeline across compactions**: after a compaction, you read it again to recover
your state. It is a plain markdown file that you maintain yourself, not the TODO list of a tool.

When the scouts reply, immediately fill in its sections: "Self-assessment" (from 0.1), "Codebase
summary" and "Translation subtasks" (from the scout replies and your own analysis), and later
"Notes for future-me". Each section of `PLAN.md` shows the form of its entries. Merge the replies of
all scouts, and number the subtasks T1, T2, .... You can split, merge or reorder subtasks for
their size and their dependencies. All subtasks together must cover all of the C source.

- **Fill it in BEFORE your context fills up.** Its sections must be complete before the first
  compaction.
- **Update the checkboxes and "Notes for future-me" IMMEDIATELY after any work completes**,
  whether it was you or a sub-agent. Mark a subtask `[x]` only after its branch is merged into
  `main`.

### 0.4 Size of a subtask

Each subtask must satisfy TWO constraints:

1. **Context window**: the subtask's combined input (C source to read) + output
   (Rust code to write) + tool overhead must fit within ONE uncompacted context
   window. A safe rule: the total token count (C source + Rust output + tool
   calls) for a single subtask should not exceed **30%** of the sub-agent's usable
   context window. If it would, split the subtask further.
2. **Output token limit**: the Rust code a sub-agent writes in a single response
   must fit within the sub-agent's single-response output token limit (see
   Self-assessment 0.1). When the estimate is larger, split at the plan level: assign
   different function groups or line ranges of the same file to different subtasks. Give the
   sub-agent a specific function range or module subset, not the whole file.

For each subtask, do a back-of-envelope estimate before you write it into `PLAN.md`, and again
before you start it:

- Input: total bytes of C files the sub-agent will need to read for this subtask (use
  `wc -c` or the line counts you already gathered), divided by ~4 to get tokens.
- Output: about C lines × 1.2 Rust lines, at about 10 tokens per line.
- Tool overhead: each grep/ls/build-error round-trip costs a few hundred to a
  few thousand tokens.

Either constraint is sufficient to force a split. Better to add three subtasks
than to be compacted mid-write or hit the output cap mid-file.

Subtask boundaries do NOT need to align with file boundaries. A large C file
can be split into multiple subtasks by function group (e.g. "translate
`api_encode` and its callees" vs "translate `api_decode` and its
callees"), as long as each subtask has well-defined inputs and outputs.

To choose the function groups, use the **call groups** in the scout reports: sets of functions
that call each other. Keep a
call group in one subtask when it fits the two constraints above, also when its functions are in
different C files. When a call group is too large, split it where few calls cross the cut, and
give the subtasks on both sides the signatures of the functions at the cut.

A subtask is something that you can dispatch after a compaction from
`PLAN.md` alone: its C files/functions, its dependencies, and the scout report
sections that its sub-agent needs.

If the project includes a test harness entry
point that is not part of the original library, plan to translate it early.

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
Read inputs/prompts/translate/translator.md and do the task in it.
Checkout: wt/<name> (branch <name>).
Subtask <Tn>: <name>.
C files and functions: <list>.
Rust files to write: <list>.
Dependencies: <types, constants and signatures of other subtasks, or where to find them>.
Scout report sections: <sections of scout/REPORT_<n>.md>.
Renaming macros: <the macros that rename its functions, from the scout reports, or "none">.
Feature names: <from `cargo_features.py names`, or "none">.
```

Add the lines that the additional prompt files of this run give for special subtasks (for example
data tables). Use the same form, with the same prompt file, for a sub-agent that fixes code: give
the files, the errors or the findings, and what to change.

Pre-inject dependencies into the sub-agent prompt. Before launching
a sub-agent, think about what types, constants, or function signatures
it will need from other modules. Either include the relevant type definitions directly in the
sub-agent's prompt, or give it the section of `scout/REPORT_<n>.md` that it needs, or give it
specific `grep` commands. When each sub-agent reads the same 500-line infrastructure file, each
one uses thousands of tokens on the same input.

## Step 2: Plan the translation

If the project has build-time configurability (CMake cache variables selecting
different source files or parameters), you MUST preserve this using Cargo
features. Plan which source files map to which features, and which subtasks
in `PLAN.md` will own each feature gate, before you send the sub-agents.

The naming contract for those features is `inputs/prompts/cargo_features.md`. Write the
`[features]` of Cargo.toml yourself, from the output of
`cargo_features.py names`, before you send the sub-agents that use them.

Break the work into phases: shared/core code first, then
each backend or variant, then wire up feature gates. These phases should
already be the subtasks in your `PLAN.md`. Do not re-plan here. Dispatch the
next unchecked subtasks.

## Step 3: Translate

Send every subtask of `PLAN.md` to a sub-agent. Send independent subtasks at the same time, in one
message. After each subtask completes:

1. Merge its branch into `main` (see `inputs/prompts/git_main.md`).
2. Mark the subtask `[x]` in `PLAN.md`.
3. Append any relevant decision/pitfall to "Notes for future-me" in `PLAN.md`.
4. Then start more subtasks.

### After a compaction

After the first actions of `inputs/prompts/main_agent.md`:

1. Confirm your most recent code touches did not violate any rule (especially feature naming
   and C ABI).
2. Find the first unchecked subtask in `PLAN.md`. That is your current work item.
3. Prepare and dispatch that subtask (Step 1). Do not read its C files yourself.
4. Resume from there. Do not redo subtasks already marked `[x]`.

## Step 4: Build and check the completion gate

Run `cargo build --release` and fix any errors until it compiles.
If the project has Cargo features, also run the configuration builds of
`inputs/prompts/cargo_features.md`, and fix them until they compile.
Send a fix that is larger than a small build fix to a sub-agent.

When all subtasks are checked and every build is green, check the main points of the completion
gate (`inputs/prompts/translate/completion_gate.md`) yourself: the `nm -D` comparison when the
project has a library, and a quick `grep` of `src/` for the stub forms. Then send a review
sub-agent with this prompt:

```
Read inputs/prompts/translate/reviewer.md and do the task in it.
```

Send the fixes for its findings to translators (Step 1). Do the small build fixes yourself. Merge
the fixes.

Mark the whole plan complete in `PLAN.md`, with a final note, only after the findings are fixed.

Your job ends when the translation passes the completion gate. A separate verification stage runs after
you and does the thorough execution testing. Beyond the one smoke test, do not write test suites or
differential tests here.
