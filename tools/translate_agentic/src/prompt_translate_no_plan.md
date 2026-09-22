<!-- markdownlint-disable MD041 -->
Translate the C code in c_src/ to Rust that produces **byte-identical output** for the same inputs.
Write Cargo.toml and src/ files in the current directory (NOT in c_src/).

{WORKFLOW_HINT}
**CRITICAL CONSTRAINT: Pure Rust Translation Only**

You MUST faithfully translate ALL C source files to **pure Rust**. Do NOT use
the `cc` crate (or any equivalent) in `build.rs` to compile or link the original
C source code. The C source files in `c_src/` will NOT exist in the final test
environment — the only code available at test time is the Rust you write. Any
`extern "C"` FFI declarations must resolve to Rust implementations you provide,
not to compiled C object files.

Do NOT import or depend on any existing Rust crate that implements, wraps,
re-exports, or compiles the same C library you are translating. Every line
of Rust code must be written by you. If a function needs to call out to
system libraries (e.g. POSIX APIs), use `libc` or equivalent thin FFI crates,
not crates that compile the library you are meant to translate.

A `build.rs` is allowed for legitimate build-time needs (code generation,
feature detection, etc.), but it must NOT reference, compile, or link any
file under `c_src/`.

If the codebase is large, you must still translate all of it. No stubs, no
placeholders, no shortcuts. Never refuse or abort a translation because the codebase seems too big.

## Step 1: Analyze BEFORE writing any code

Before writing a single line of Rust, you MUST:
1. Read `c_src/CMakeLists.txt` to understand the build system, source file selection,
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
source files or parameters):
- You MUST preserve this using **Cargo features**. Each CMake cache variable value
  becomes a Cargo feature using the **exact same name in lowercase**.
- Use `#[cfg(feature = "...")]` to conditionally compile modules and set constants.
- ALL combinations of features must compile.
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
If the project has Cargo features, verify ALL feature combinations compile:
run `cargo build --release --features <feature>` for each one.

Do NOT modify anything in c_src/.

{MODEL_LIMITS}

{RUST_TOOLCHAIN_CONTEXT}

{WORKDIR_BOUNDARY}

{AGENT_TOOLS_SECTION}

## Static Analysis Tool Wishlist

As you work through this translation, pay attention to moments where you think:
- "If I had a tool that could tell me X, I could skip this lengthy file reading / reasoning."
- "If I had a tool that could do Y, I would have much higher confidence in this translation step."

Whenever such a thought arises, **immediately** append one JSON object (on a single line) to
the file `{WISHLIST_PATH}`. Do not wait until the end — record the wish as soon as it occurs,
while the context is fresh. Multiple entries are encouraged; record every distinct need.

Each entry must be a single-line JSON object with exactly these fields:

```
{"category": "...", "description": "...", "language": "...", "soundness": "...", "completeness": "...", "value": 0}
```

Field definitions:
- `category`: `"info_query"` (read-only analysis that answers a question) or `"code_edit"` (a transformation/rewrite tool)
- `description`: plain English description of what the tool does — **no implementation details**, just what it gives you and why it would help
- `language`: `"C"`, `"Rust"`, `"C_and_Rust"`, or another language name
- `soundness`: `"required"` (must never give wrong answers), `"preferred"`, or `"not_needed"` (approximate/heuristic output is fine)
- `completeness`: `"required"` (must cover all cases), `"preferred"`, or `"not_needed"` (partial results are useful enough)
- `value`: integer 0–10 estimating how much this tool would have helped you in this specific task

## Git discipline
{GIT_DISCIPLINE}
