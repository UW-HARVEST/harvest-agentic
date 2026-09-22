<!-- markdownlint-disable MD041 -->
{WORKFLOW_HINT}
You are testing a C-to-Rust translation for correctness. The C code is the
ground truth — the Rust code must produce byte-identical results.

- `c_src/` contains the original C source code
- `src/` contains the Rust translation
- The C code can be compiled as a shared library. Look at c_src/CMakeLists.txt
  to understand the build system. Build it with:
  ```
  cd c_src && mkdir -p build && cd build && \
  cmake .. -DCMAKE_POSITION_INDEPENDENT_CODE=ON {CMAKE_BUILD_FLAGS} && \
  cmake --build .
  ```
- Find the resulting .so files in the build output

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
   with the same inputs and compare their stdout byte-for-byte. Fix any differences.
9. Compare `nm -D` on the C .so and the Rust .so. Every symbol the C .so
   exports, the Rust .so must also export with the exact same name. This
   includes symbols created by preprocessor macros. If the C .so exports it,
   the Rust .so must export it — no exceptions. Add missing exports.

Add `libloading = "0.8"` to [dev-dependencies] in Cargo.toml.
Do NOT modify anything in c_src/.

If configurations are listed below, you MUST verify each one:
- Clean and rebuild C with the listed cmake flags
- Rebuild Rust with the matching Cargo features (`--no-default-features --features <list>`)
- Re-run integration tests and fix any mismatches before moving to the next configuration

{ALL_CONFIGURATIONS}

IMPORTANT: Use timeouts for all commands. No single build or test command should
run longer than 600 seconds. If a test takes too long, skip it and move on to
the next function. Use `timeout 600 cargo test ...` or similar. Do not get stuck
on any single step.

{MODEL_LIMITS}

{RUST_TOOLCHAIN_CONTEXT}

{WORKDIR_BOUNDARY}

{AGENT_TOOLS_SECTION}

## Static Analysis Tool Wishlist

As you work through verification and fixing, pay attention to moments where you think:
- "If I had a tool that could tell me X, I could skip this lengthy reasoning / exploration."
- "If I had a tool that could do Y, I would have much higher confidence in this fix."

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
