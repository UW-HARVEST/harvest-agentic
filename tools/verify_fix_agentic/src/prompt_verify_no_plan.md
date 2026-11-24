<!-- markdownlint-disable MD041 -->
{WORKFLOW_HINT}
You are testing a C-to-Rust translation for correctness. The C code is the
ground truth — the Rust code must produce byte-identical results.

Before you start, read these files. They hold rules of this run that this prompt does not repeat:

- `inputs/prompts/common.md`: the directories of the run, the command environment and the Rust
  toolchain. It names two more files to read.
- `inputs/prompts/git_main.md`: the git discipline.
- `inputs/prompts/cargo_features.md`: the Cargo features of the C build options.
- Each additional prompt file that `inputs/prompts/run.md` lists.

When you send a sub-agent, start its prompt with the line `Read inputs/prompts/subagent.md first.`

- `inputs/c_src` contains the original C source code
- `src/` contains the Rust translation
- The C code can be compiled as a shared library. Look at inputs/c_src/CMakeLists.txt
  to understand the build system. Build it with:
  ```
  cmake -S inputs/c_src -B scratch/c-build \
    -DCMAKE_POSITION_INDEPENDENT_CODE=ON && \
  cmake --build scratch/c-build
  ```
- Find the resulting .so files in scratch/c-build

Your task:
1. Build the C code as a shared library
2. Write Rust integration tests (in tests/) that use `libloading`
   to load the C .so and compare C vs Rust function outputs
3. Start with the lowest-level functions and work upward to higher-level ones.
   Look at the C headers to identify the public API and function call hierarchy.
4. For each function: create fixed test inputs, call both C and Rust versions,
   assert outputs match byte-for-byte
5. Run `cargo test` and investigate any mismatches
6. When you find a Rust function that produces different output than C,
   fix the Rust code in src/ and re-run until the test passes
7. Keep going until all public functions match
8. If the project has a main binary, run both the C binary and the Rust binary
   with the same argv, stdin and input files, and compare their stdout, stderr,
   exit status and written files byte-for-byte. Fix any differences.
9. Compare `nm -D` on the C .so and the Rust .so. Every symbol the C .so
   exports, the Rust .so must also export with the exact same name. This
   includes symbols created by preprocessor macros. If the C .so exports it,
   the Rust .so must export it — no exceptions. Add missing exports.

Add `libloading = "0.8"` to [dev-dependencies] in Cargo.toml.
Do NOT modify anything in inputs/c_src.

Verify each configuration you can (see `inputs/prompts/cargo_features.md`):
- Clean and rebuild C with the option values as cmake `-D` flags
- Rebuild Rust with its Cargo features
- Re-run integration tests and fix any mismatches before moving to the next configuration

IMPORTANT: Use time limits for all commands, as "Long commands" in
`inputs/prompts/common.md` tells. If a test takes too long, skip it and move on to
the next function. Do not get stuck on any single step.
