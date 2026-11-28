Read these files first:

- `inputs/prompts/output_spec.md`
- `inputs/prompts/environment.md`
- `inputs/prompts/anti_cheating.md`

Where these files and the prompt below differ, follow these files.

<!-- markdownlint-disable MD041 -->
Translate the C code in inputs/c_src/ to Rust that produces **byte-identical output** for the same inputs.
Write Cargo.toml and src/ files in the current directory (NOT in inputs/c_src/).

You MUST translate ALL C source files — no stubs, no placeholders, no empty
functions. Every .c file MUST have a complete Rust equivalent. The binary MUST
produce the same stdout as the C binary for the same inputs.

This project has **build-time configurability** via CMake cache variables.
Look at inputs/c_src/CMakeLists.txt — it uses variables to select which source files
to compile and which parameter headers to include at build time.

You MUST preserve this configurability using **Cargo features**. Each CMake cache
variable value becomes a Cargo feature.
Use `#[cfg(feature = "...")]` to conditionally compile modules and set constants.
All combinations of features must compile.

This project produces a shared library, a binary executable, or both.
Your Cargo.toml must have `[lib]` with `crate-type = ["cdylib"]` for the library and
`[[bin]]` with `name = "driver"` and `path = "src/main.rs"` for the executable.

**This is a large project.** Do NOT try to translate everything yourself in one go.
Instead:
1. Analyze the C project structure and create a plan (TODO list) breaking the
   translation into subtasks (e.g., core/shared code, each backend, entry points)
2. The binary driver (main.rs) MUST be one of the subtasks — do not leave it for last.
   Translate it fully, not as a stub.
3. Work through the subtasks one at a time, with a clear, focused scope for each:
   - Which specific C source files to translate
   - Which Rust file(s) to write
   - Build and verify each subtask compiles with the relevant features
   - Do NOT modify files outside the current subtask's scope
4. After each subtask completes, verify the work compiles before moving on
5. Once all subtasks are done, wire up the feature gates and verify the full build

After all subtasks complete, wire up the feature gates and do a final build check.
If a combination fails, only fix the glue code (lib.rs, mod declarations) — do NOT
modify the backend implementation files.

Requirements:
- All public C functions must use #[unsafe(no_mangle)] and extern "C"
- Pay attention to C preprocessor macros that RENAME functions (e.g.,
  `#define foo NAMESPACE(foo)` makes the linker symbol `PREFIX_foo`, not `foo`).
  The Rust #[no_mangle] name must match the FINAL linker symbol, not the
  source-level name. Check header files for namespace macros.
- Preserve the exact C function signatures (use *const c_char, c_int, etc. from std::ffi)
- Do NOT fix bugs in the original C code — reproduce behavior exactly
- Use safe Rust internally where possible

Do NOT modify anything in inputs/c_src/.
