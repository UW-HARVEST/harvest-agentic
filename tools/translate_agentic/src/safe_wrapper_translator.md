# Safe API wrappers in your subtask

This run translates to a safe boundary design. The design document is
`scout/safe_wrapper/DESIGN.md`. Your task names the sections of it for your subtask. Read these
sections first. The terms of the design (containers, views, trusted methods, S1 to S11) are defined
in `inputs/prompts/SAFE_WRAPPER_DESIGNER.md`. Read only the part that you need.

The crate has three layers, as modules:

- `api`: the API wrappers, which are the C exports. Each one checks the arguments in the C order,
  converts them, calls one internal function, and writes back the results.
- `wrap`: the wrapper layer. It holds the wrapper types and their methods, the conversion functions
  and the helpers.
- `internal`: the internal code, which is the translated algorithms. Its root file starts with
  `#![forbid(unsafe_code)]`.

Rules:

1. In the internal code, keep the C data structures in their wrapped form, and keep the C structure
   and control flow. Change only how the code accesses the data. Each direct operation of C on
   wrapped data (field read or write, pointer arithmetic, cast, memcpy, memset) becomes a call of a
   wrapper method.
2. Write `unsafe` only in `api` and in the trusted methods that `DESIGN.md` lists. Give each
   `unsafe` block a `// SAFETY:` comment with its basis: the C interface contract, or an invariant
   of the wrapper layer.
3. Implement the methods of `wrap` and the functions of `api` of your task as `DESIGN.md` and their
   doc comments describe. `src/wrap/rt/` holds the framework items. Use them, and never change
   them.
4. Do not change a signature of the skeleton. When a method is missing or a signature does not
   work, do not go around the design. Write `todo!("missing method: ...")` or
   `todo!("bad signature: ...")` at that place, and continue.
5. In your report, list these `todo!()` (with the C operation and the C lines), and the trusted
   methods that you implemented.
