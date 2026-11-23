<!-- markdownlint-disable MD041 -->
Translate the C code in inputs/c_src to Rust that produces **byte-identical output** for the same inputs.
Write Cargo.toml and src/ files in the current directory (NOT in inputs/c_src).

This is a LIBRARY. Requirements:
- Cargo.toml must have crate-type = ["cdylib"] under [lib]
- All public C functions must use #[unsafe(no_mangle)] and extern "C"
- Pay attention to C preprocessor macros that RENAME functions (e.g.,
  `#define foo NAMESPACE(foo)` makes the linker symbol `PREFIX_foo`, not `foo`).
  The Rust #[no_mangle] name must match the FINAL linker symbol, not the
  source-level name. Check header files for namespace macros.
- Export the ENTIRE public symbol surface: every non-static function the C
  shared library exports needs a matching Rust export, including functions
  nothing in the repo appears to call (checked with nm -D against the C build)
- Preserve the exact C function signatures (use *const c_char, c_int, etc. from std::ffi)
- Do NOT fix bugs in the original C code — if the C has incorrect behavior, reproduce it exactly
- Preserve the exact order of error checks and validation
- Use safe Rust internally where possible

Run 'cargo build --release' and fix any errors until it compiles.
Do NOT modify anything in inputs/c_src.

{MODEL_LIMITS}

{RUST_TOOLCHAIN_CONTEXT}

{TOOL_WISHLIST}

## Git discipline
{GIT_DISCIPLINE}

{CASE_LAYOUT}
