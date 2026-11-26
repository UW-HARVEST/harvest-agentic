# Translation Plan

This file is the working state of the main agent of the translation stage. It holds state, not
rules. The main agent fills in the sections and keeps them up to date.

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

## External dependencies
One line for each library that the C code uses and whose source is not in `inputs/c_src`, other
than glibc ("none" when there is none):

- <library> — functions used: <list> — decision: <Rust crate `name`, or link the C library> — reason: <why>

## Translation subtasks
All subtasks together cover all of the C source. One line for each subtask:

- [ ] T1: <name> — files/functions: <list> — estimated output: ~Nk tokens — depends on: <other Tx>
- [ ] T2: ...
- [x] T3: ...   <!-- mark done after its branch is merged into main -->

## Notes for future-me (post-compaction)
- Decisions already made and why
- Cargo features chosen and what they gate
- Pitfalls noticed (e.g. macro renames, namespace prefixes)
- Where you stopped and what to do next
