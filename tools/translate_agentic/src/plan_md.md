# Translation Plan

The framework wrote this file for the main agent of the translation. Keep the `Invariants` section
as it is. Fill in the other sections and keep them up to date.

## Invariants (do not drift across compactions)

These rules are not negotiable. When in doubt, re-read this section.

### AFTER ANY COMPACTION: `cat PLAN.md` is your FIRST action before anything.

### Role of the main agent
- The main agent owns this plan, the merges into `main` and the build.
- Sub-agents read the C source and write the Rust code. The main agent sends every subtask to a
  sub-agent. It does not translate a subtask itself.
- A sub-agent prompt names the task file of the sub-agent in `inputs/prompts/` and gives the facts
  of its subtask. The task files contain the rules of this run. The main agent does not copy rules
  into prompts.
- The translation ends only when it passes the completion gate below.

{MODEL_LIMITS}

{RUST_TOOLCHAIN_CONTEXT}

{CASE_LAYOUT}

{CARGO_FEATURES}

### Cargo manifest target names
- `[lib] name` and `[[bin]] name` MUST use underscores only — NO hyphens.
  Hyphens in target names cause `cargo` to fail parsing the manifest entirely.
  RIGHT: `name = "sphincs_plus"`, WRONG: `name = "sphincs-plus"`.

{TRANSLATION_RULES}

{COMPLETION_GATE}

### Git discipline
{GIT_DISCIPLINE}

{TAB2RUST_INVARIANT}

{SAFE_WRAPPER_INVARIANT}

## Self-assessment
- Model: ...
- Window: ...
- Output token limit: ...
- Sub-agents: model ..., window ..., output token limit ...

## Codebase summary
- Files: <one line per .c/.h with line count + 1-line role>
- Project type: bin / lib / both
- Build configurability: <the options and their values; the features that `cargo_features.py names` prints>
- Public API surface: <list of public functions/types>

## Translation subtasks
Each subtask must satisfy TWO constraints:
1. **Context window**: the subtask's combined input (C source to read) + output
   (Rust code to write) + tool overhead must fit within ONE uncompacted context
   window. A safe rule: the total token count (C source + Rust output + tool
   calls) for a single subtask should not exceed **30%** of the sub-agent's usable
   context window. If it would, split the subtask further.
2. **Output token limit**: the Rust code a sub-agent writes in a single response
   must fit within the sub-agent's single-response output token limit (see
   Self-assessment above). Estimate the Rust output: about C lines × 1.2 lines, at
   about 10 tokens per line. Compare this estimate with the output token limit.
   When the estimate is larger, split at the plan level: assign different function
   groups or line ranges of the same file to different subtasks.

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
PLAN.md alone: its C files/functions, its dependencies, and the scout report
sections that its sub-agent needs.

If the project includes a test harness entry
point that is not part of the original library, plan to translate it early.

All subtasks together cover all of the C source. Write one line for each subtask:

- [ ] T1: <name> — files/functions: <list> — estimated output: ~Nk tokens — depends on: <other Tx>
- [ ] T2: ...
- [x] T3: ...   <!-- mark done after its branch is merged into main -->

## Notes for future-me (post-compaction)
- Decisions already made and why
- Cargo features chosen and what they gate
- Pitfalls noticed (e.g. macro renames, namespace prefixes)
- Where you stopped and what to do next
