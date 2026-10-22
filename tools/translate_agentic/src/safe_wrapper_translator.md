# Safe API wrappers in your subtask

This run translates to a safe boundary design. The design document is
`scout/safe_wrapper/DESIGN.md`. Your task names the sections of it for your subtask. Read these
sections first. The terms of the design (containers, views, trusted methods, S1 to S11) are defined
in `inputs/prompts/SAFE_WRAPPER_DESIGNER.md`. Read only the part that you need.

The crate has three layers, as modules:

- `api`: the API wrappers, which are the C exports. Each one checks the arguments in the C order,
  converts them, calls one internal function, and writes back the results. Only `api` calls into
  `api`: `wrap` and `internal` never call an export.
- `wrap`: the wrapper layer. It holds the wrapper types and their methods, the conversion functions
  and the helpers.
- `internal`: the internal code, which is the translated algorithms. Its root file starts with
  `#![forbid(unsafe_code)]`.

Rules:

1. C is the only truth. Do what C does, also for input that you think is undefined behavior in C.
   Where C dereferences a NULL pointer, panic. Where C returns an error, return the same error.
2. In the internal code, keep the C data structures in their wrapped form, and keep the C structure
   and control flow. Change only how the code accesses the data. Each direct operation of C on
   wrapped data (field read or write, pointer arithmetic, cast, memcpy, memset) becomes a call of a
   wrapper method.
3. Write `unsafe` only in `api` and `wrap`. Give each `unsafe` block a `// SAFETY:` comment with
   its basis: the C interface contract, or an invariant of the wrapper layer.
4. Implement the methods of `wrap` and the functions of `api` of your task as `DESIGN.md` and their
   doc comments describe. `src/wrap/rt/` holds the framework items. Use them, and never change
   them.
5. Keep the signatures and the containers and views of the design. When the design misses a
   method or a signature does not work, add a method or function next to it. Never replace a
   container or a view.
6. In your report, list your design changes with their C lines: new functions and methods, new
   `unsafe` code, and new heap allocations.
