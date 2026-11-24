# Translation rules

Reader: each agent of the translation stage that plans, writes or checks Rust code.

## C ABI
- Public C exports use `#[unsafe(no_mangle)]` and `extern "C"` with exact C
  signatures (use `*const c_char`, `c_int`, etc. from `std::ffi`).
- The exported symbol name is the FINAL linker symbol after all preprocessor
  renames. If C has `#define foo NAMESPACE(foo)` producing `PREFIX_foo`, the
  Rust export is named `PREFIX_foo`, not `foo`.
- Export the ENTIRE public symbol surface: every non-`static` function the C
  shared library exports needs a matching Rust export — including functions
  no test or caller in the repo appears to use. The completion gate
  (`inputs/prompts/translate/completion_gate.md`) compares the exports with
  `nm -D` against the C build. Each export must be a real
  translation of its C function. An export whose body does not do what the C
  function does counts as missing, also when `nm -D` shows its name.

## Behavioral fidelity
- Do NOT fix bugs in the original C. Reproduce behavior exactly.
- Preserve the exact order of error checks and validation.
- Match C's stdin reading semantics (scanf reads across newlines; fgets does not).
- Match C's exact printf format including spacing and newlines.

## Crate constraints
- Do NOT use the `openssl` crate or any OpenSSL bindings. Use pure-Rust crates.
- Prefer safe Rust internally; do not relax the C ABI on exports.

## Translation fidelity
- You MUST faithfully translate ALL C source files to **pure Rust**. Do NOT use
  the `cc` crate (or any equivalent) in `build.rs` to compile or link the
  original C source code. The C source files in `inputs/c_src` will NOT exist in
  the final test environment — the only code available at test time is the
  Rust you write. Any attempt to wrap C via a compiled static archive or
  object file will fail at test time because the C files simply won't be
  there.
- Do NOT import or depend on any existing Rust crate that implements, wraps,
  re-exports, or compiles the same C library you are translating. If a
  function needs to call out to system libraries (e.g. POSIX APIs), use `libc`
  or equivalent thin FFI crates, not crates that compile the library you are
  meant to translate.
- Do NOT generate Rust from the C source with
  an external tool that these instructions do not name. This includes `c2rust`
  and other C-to-Rust transpilers.
- The agents of this run write all of the Rust code. Do NOT copy Rust code
  from outside your checkout, for example from another translation.
- A `build.rs` is allowed for legitimate build-time needs (code generation,
  feature detection, etc.), but it must NOT reference, compile, or link any
  file under `inputs/c_src`.

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
