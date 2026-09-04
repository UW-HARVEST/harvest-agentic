<!-- markdownlint-disable MD041 -->
You are testing a C-to-Rust translation for correctness. The C code is the
ground truth — the Rust code must produce byte-identical results.

The C implementation is ALWAYS correct. Never second-guess the C code's logic,
even if it looks unusual or inconsistent. Your Rust translation will be tested
against the C code and must match its behavior exactly for all inputs. If the
C code does something unexpected, replicate that behavior — do not "fix" it.

Working directory: {CASE_DIR}

- `translated_rust/c_src/` contains the original C source code
- `translated_rust/src/` contains the Rust translation
- The C code can be compiled as a shared library. Look at c_src/CMakeLists.txt
  to understand the build system. Build it with:
  ```
  cd translated_rust/c_src && mkdir -p build && cd build && \
  cmake .. -DCMAKE_POSITION_INDEPENDENT_CODE=ON {CMAKE_BUILD_FLAGS} && \
  cmake --build .
  ```
- Find the resulting .so files in the build output

Your task:
1. Read Cargo.toml [features] and c_src/CMakeLists.txt to understand all
   build-time configurations. Enumerate every valid feature combination.
2. Run `cargo check --no-default-features --features <combo>` for EVERY
   combination. Fix all compile errors before proceeding. Modules or code
   that only apply to certain backends must use `#[cfg(feature = "...")]`.
3. Build the C code as a shared library for the default configuration.
4. Write Rust integration tests (in translated_rust/tests/) that use
   `libloading` to load BOTH the C .so AND the Rust .so, and compare their
   outputs through the FFI boundary. Never call Rust functions directly —
   always load the Rust .so via libloading and call its exported symbols,
   exactly as an external caller would. This tests the `#[no_mangle]`
   export wrappers too.
5. Start with the lowest-level functions and work upward to higher-level ones.
   Look at the C headers to identify the public API and function call hierarchy.
6. For each function: create test inputs, call both C and Rust via their .so
   exports, assert outputs match byte-for-byte.
7. Run `cargo test` and fix any mismatches.
8. Compare `nm -D` on the C .so and the Rust .so. Every symbol the C .so
   exports, the Rust .so must also export with the exact same name. This
   includes symbols created by preprocessor macros. If the C .so exports it,
   the Rust .so must export it — no exceptions. Add missing exports.
9. Repeat steps 6-8 for EVERY feature combination from step 1. Switch features
   with `cargo test --no-default-features --features <combo>`. Each combination
   may exercise completely different code paths.
10. Do not declare success until every function matches under every feature
    combination. This code is shared across ALL configurations — you must
    build and test against C for every backend and parameter set, not just
    the default.

**Tip:** Write shell loops or scripts to automate repetitive work. For example,
to check all feature combinations: extract them from Cargo.toml, loop over them,
and run `cargo check` for each. Same for running tests across combinations.
Do not manually repeat commands for each configuration — automate it.

Add `libloading = "0.8"` to [dev-dependencies] in translated_rust/Cargo.toml.
Do NOT modify anything in c_src/.

IMPORTANT: If a file is too large to write in one tool call, build it up
piece by piece using multiple smaller writes (create then append).

IMPORTANT: Use timeouts for all commands. No single build or test command should
run longer than 600 seconds. If a test takes too long, skip it and move on to
the next function. Use `timeout 600 cargo test ...` or similar. Do not get stuck
on any single step.

{MODEL_LIMITS}

{RUST_TOOLCHAIN_CONTEXT}

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
