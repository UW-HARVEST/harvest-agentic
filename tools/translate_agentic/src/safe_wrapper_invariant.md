### Safe API wrappers
- The boundary design is `scout/safe_wrapper/DESIGN.md`, with its code skeleton in
  `scout/safe_wrapper/skeleton/`. After the designer replies, you own the design. You fix the
  skeleton and the bad signatures, and you record each change in `DESIGN.md`.
- The crate has three modules: `api` (the C exports, with the C ABI rules above), `wrap` (the
  wrapper layer) and `internal` (`src/internal/mod.rs` starts with `#![forbid(unsafe_code)]`).
  `src/wrap/rt/` holds the framework items that the design uses. Never change it.
- Make the subtasks from the scout proposals and the design together:
  - T1 copies the skeleton into `src/`, and makes `cargo build` pass with the `todo!()` bodies. After T1, the other subtasks can run in one wave.
  - The subtask of a C file `X.c` writes `src/internal/X.rs` and `src/api/X.rs`.
  - The wrapper types of `wrap` are their own subtasks. A C module that the design wraps whole
    (8.2) goes into them.
  - Each subtask lists its sections of `DESIGN.md`: `— safe wrapper: <names>`. Its sub-agent prompt
    contains: `Read inputs/prompts/SAFE_WRAPPER_TRANSLATOR.md and the sections <names> of scout/safe_wrapper/DESIGN.md.`
- Sub-agents mark missing methods and bad signatures with `todo!()`, and report them. For missing
  methods and functions that the designer did not design, send the designer again with the line
  `Add to the design: <list>.` Only one designer runs at a time.
- The design marks non-local jumps, variadic functions and private heap buffers. Decide how to
  handle each one, and write it in "Notes for future-me".
- At the end, `grep -rn 'todo!' src/` finds nothing.
