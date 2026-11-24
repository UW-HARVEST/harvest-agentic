<!-- markdownlint-disable MD041 -->
Translate the C code in inputs/c_src to Rust that produces **byte-identical output** for the same inputs.
Write Cargo.toml and src/ files in the current directory (NOT in inputs/c_src).

Before you start, read these files. They hold rules of this run that this prompt does not repeat:

- `inputs/prompts/common.md`: the directories of the run, the command environment and the Rust
  toolchain. It names two more files to read.
- `inputs/prompts/git_main.md`: the git discipline.
- `inputs/prompts/cargo_features.md`: the Cargo features of the C build options.
- `inputs/prompts/translate/completion_gate.md`: the completion gate.
- `inputs/prompts/translate/translation_rules.md`: read its section "No stubs".
- Each additional prompt file that `inputs/prompts/run.md` lists.

When you send a sub-agent, start its prompt with the line `Read inputs/prompts/subagent.md first.`

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

If the codebase is large, you must still translate all of it, with no stubs (see the completion
gate). Never refuse or abort a translation because the codebase seems too big. This
framework has translated C code bases of more than a million lines, and the budget of this run is
sufficient for this code base.

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
source files or parameters), you MUST preserve this with Cargo features (see
`inputs/prompts/cargo_features.md`):
- Use `#[cfg(feature = "...")]` to conditionally compile modules and set constants.
- Plan which source files map to which features before writing code.
- Do NOT hardcode a single configuration — every variant must be implemented.

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
  nothing in the repo appears to call. The completion gate compares the
  exports with `nm -D` against the C build. Each export must be a real
  translation of its C function. An export whose body does not do what the C
  function does counts as missing, also when `nm -D` shows its name.
- Do NOT fix bugs in the original C code — reproduce behavior exactly
- Preserve the exact order of error checks and validation
- Match C's stdin reading behavior exactly (scanf reads across newlines, fgets does not)
- Match C's exact printf format output including spacing and newlines
- Do NOT use the `openssl` crate or any OpenSSL bindings — use pure-Rust crates instead
- Use safe Rust internally where possible

## Step 4: Check the completion gate

Run `cargo build --release` and fix any errors until it compiles.
If the project has Cargo features, also run the configuration builds (see
`inputs/prompts/cargo_features.md`) and fix them until they compile.

Before you end, check each point of the completion gate
(`inputs/prompts/translate/completion_gate.md`) yourself. Search all of `src/` with `grep`
for each stub form of "No stubs". For each exported function, check that its body is a real translation of the C
function. Fix what you find, and check again.

Your job ends when the translation passes the completion gate. A separate verification stage runs
after you and does the thorough execution testing. Beyond the one smoke test, do not write test
suites or differential tests here.

Do NOT modify anything in inputs/c_src.
