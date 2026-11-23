<!-- markdownlint-disable MD041 -->
Translate the C code in inputs/c_src to Rust that produces **byte-identical output** for the same inputs.
Write Cargo.toml and src/ files in the current directory (NOT in inputs/c_src).

{WORKFLOW_HINT}
**CRITICAL CONSTRAINT: Pure Rust Translation Only**

You MUST faithfully translate ALL C source files to **pure Rust**. Do NOT use
the `cc` crate (or any equivalent) in `build.rs` to compile or link the original
C source code. The C source files in `inputs/c_src` will NOT exist in the final test
environment — the only code available at test time is the Rust you write. Any
`extern "C"` FFI declarations must resolve to Rust implementations you provide,
not to compiled C object files.

Do NOT import or depend on any existing Rust crate that implements, wraps,
re-exports, or compiles the same C library you are translating. If a function
needs to call out to system libraries (e.g. POSIX APIs), use `libc` or
equivalent thin FFI crates, not crates that compile the library you are meant
to translate.

Write all Rust code yourself. Do NOT generate Rust from the C source with an
external tool that these instructions do not name. This includes `c2rust` and
other C-to-Rust transpilers.

A `build.rs` is allowed for legitimate build-time needs (code generation,
feature detection, etc.), but it must NOT reference, compile, or link any
file under `inputs/c_src`.

If the codebase is large, you must still translate all of it. No stubs, no
placeholders, no shortcuts. Never refuse or abort a translation because the codebase seems too big.

## Step 1: Analyze BEFORE writing any code

Before writing a single line of Rust, you MUST:
1. Read `inputs/c_src/CMakeLists.txt` to understand the build system, source file selection,
   and any build-time configurability (cache variables, options, conditional compilation)
2. Read all header files to understand the public API, preprocessor macros, and
   namespace/symbol renaming patterns
3. Determine the project type:
   - Has `main()` → needs `[[bin]]` with `name = "driver"`
   - Exports library functions → needs `[lib]` with `crate-type = ["cdylib"]`
   - Both → include both `[lib]` and `[[bin]]` sections
4. Identify ALL backends/variants if the project has build-time configurability

## Step 2: Plan the translation

If the project has build-time configurability (CMake cache variables selecting different
source files or parameters), you MUST preserve this with Cargo features (see "Cargo features"
below):
- Use `#[cfg(feature = "...")]` to conditionally compile modules and set constants.
- Plan which source files map to which features before writing code.
- Do NOT hardcode a single configuration — every variant must be implemented.

{CARGO_FEATURES}

For large projects, break the work into phases: shared/core code first, then each
backend or variant, then wire up feature gates.

## Step 3: Translate

- All public C functions must use `#[unsafe(no_mangle)]` and `extern "C"` with exact
  C signatures (use `*const c_char`, `c_int`, etc. from `std::ffi`)
- Pay attention to C preprocessor macros that RENAME functions (e.g.,
  `#define foo NAMESPACE(foo)` makes the linker symbol `PREFIX_foo`, not `foo`).
  The Rust `#[no_mangle]` name must match the FINAL linker symbol, not the
  source-level name.
- Export the ENTIRE public symbol surface: every non-static function the C
  shared library exports needs a matching Rust export — including functions
  nothing in the repo appears to call (verified later with `nm -D` against
  the C build)
- Do NOT fix bugs in the original C code — reproduce behavior exactly
- Preserve the exact order of error checks and validation
- Match C's stdin reading behavior exactly (scanf reads across newlines, fgets does not)
- Match C's exact printf format output including spacing and newlines
- Do NOT use the `openssl` crate or any OpenSSL bindings — use pure-Rust crates instead
- Use safe Rust internally where possible

## Step 4: Verify

Run `cargo build --release` and fix any errors until it compiles.
If the project has Cargo features, also run the configuration builds (see "Cargo features"
above) and fix them until they compile.

Do NOT modify anything in inputs/c_src.

{MODEL_LIMITS}

{RUST_TOOLCHAIN_CONTEXT}

{AGENT_BUG_WORKAROUNDS}

{CASE_LAYOUT}

{TOOL_WISHLIST}

## Git discipline
{GIT_DISCIPLINE}
