# Translation rules

`inputs/prompts/output_spec.md` tells the shape that the result must have.
`inputs/prompts/anti_cheating.md` tells what is not permitted. This file adds
to them.

## C ABI
- Public C exports use `#[unsafe(no_mangle)]` and `extern "C"` with exact C
  signatures (use `*const c_char`, `c_int`, etc. from `std::ffi`).
- The final linker name: if C has `#define foo NAMESPACE(foo)` producing
  `PREFIX_foo`, the Rust export is named `PREFIX_foo`, not `foo`.
- The exports include every non-`static` function that the C shared library
  exports, also functions no test or caller in the repo appears to use. The
  completion gate
  (`inputs/prompts/translate/completion_gate.md`) compares the exports with
  `nm -D` against the C build. Each export must be a real
  translation of its C function. An export whose body does not do what the C
  function does counts as missing, also when `nm -D` shows its name.
- Prefer safe Rust internally; do not relax the C ABI on exports.

## Behavioral fidelity
- Do NOT fix bugs in the original C. Reproduce behavior exactly.
- Preserve the exact order of error checks and validation.
- Match C's stdin reading semantics (scanf reads across newlines; fgets does not).
- Match C's exact printf format including spacing and newlines.

## Translation fidelity
- This is a translation task: the result must be Rust. You MUST faithfully
  translate ALL C source files, also the third-party code that the project
  has in its source tree.
- The rule against compiled C code applies to the files of `inputs/c_src`, to
  C code that an agent writes, and to C code that a crate brings (for example
  a `-sys` crate that builds a C library).
- A `build.rs` is allowed for other build-time needs (code generation,
  feature detection, link directives).
- The agents of this run write all of the Rust code.

## External dependencies
The external dependencies of the C code are its external libraries (see
`inputs/prompts/anti_cheating.md`). Before you add a crate to `Cargo.toml`,
make sure that Cargo downloads and builds it.

### glibc
glibc is libc, libm, and the pthread, dl and rt libraries in it. The Rust
code uses glibc as the C code does.
- Call a glibc symbol through the `libc` crate. Write an `extern` declaration
  yourself only for a symbol that the `libc` crate does not have.
- When the implementation of glibc decides a result (for example the output
  of `printf`), get the result from glibc. Do NOT write your own version. For
  other operations, the Rust standard library is acceptable.

### Other dependencies
- Use a pure-Rust crate (no C code, no binding to a C library) that takes the
  place of the dependency with no layer that imitates its interface.
- When there is no such crate, link the C library of the system: write
  `#[link(name = "...")]` and the `extern` declarations from its header.
- Do NOT write your own version of a dependency or of a part of it. Where the
  specification of a dependency leaves a result open, a crate can give a
  different result than the C library, as long as the observable behavior of
  the translated code stays the same as C. If it does not, do NOT use the
  crate: link the C library.
- The main agent decides for each dependency. A sub-agent follows the
  decision in its task message, and reports a dependency that the message
  does not name.

## No stubs
Every function, struct, constant and macro of the C source has a Rust
translation that does what the C code does. Common forms of a stub:
- a body that returns a constant, zero or a null pointer and ignores its parameters,
- an empty body,
- `todo!()`, `unimplemented!()`, `panic!("not implemented")` or similar,
- a function that only logs, or only returns an error code, where the C function does real work,
- a generated file of placeholder exports.

A function is not a stub when the API contract defines its return value as
a compile-time constant.
