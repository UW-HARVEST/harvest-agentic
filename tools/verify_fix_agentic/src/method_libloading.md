## Step 3: Verification workflow (libloading)

You compare C against Rust from **inside a Rust integration test**: build the C
code as a shared library, `dlopen` it with the `libloading` crate, and call the
C and Rust implementations side by side on the same inputs.

Build the C code as a shared library first. Look at `inputs/c_src/CMakeLists.txt` to
understand the build system, then:

```
cmake -S inputs/c_src -B scratch/c-build \
  -DCMAKE_POSITION_INDEPENDENT_CODE=ON && \
cmake --build scratch/c-build
```

Find the resulting `.so` files in `scratch/c-build`.

Then do the actual verification:

1. Add `libloading = "0.8"` to `[dev-dependencies]` in `Cargo.toml` (so your
   integration tests can dlopen the C shared library).
2. Write Rust integration tests (in `tests/`) that use `libloading` to load the
   C `.so` and compare C vs Rust function outputs.
3. Start with the lowest-level functions and work upward to higher-level ones.
   Look at the C headers to identify the public API and function call hierarchy.
4. For each function: create fixed test inputs, call both C and Rust versions,
   assert outputs match byte-for-byte.
5. Cover every row of the surface files from Step 2. Each `CONFIGS.md` row
   gets a differential test, and each `ERRORS.md` row gets a rejection test.
   The rejection test asserts that both sides give the same error code,
   sentinel value, or exit status and message, not only that both sides fail. Also test the generic
   boundaries from Step 2: null pointers, zero and oversized lengths, values
   one step past a valid range, and enum arguments with no valid variant.
6. Run `cargo test` and investigate any mismatches. Every time a test exposes a
   divergence, append a hypothesis to `HYPOTHESES.md`.
7. When you find a Rust function that produces different output than C, fix the
   Rust code in `src/` and re-run until the test passes. Update the matching
   hypothesis to `fixed` after the Edit.
8. For the executable, write tests that run the C executable (from the same
   CMake build) and the Rust `driver` on the same argv, stdin and input files,
   each in a fresh working directory at the same path, and compare stdout,
   stderr, exit status and written files byte-for-byte.
9. Keep going until every public entry point matches.
10. Re-run the comparison from `SYMBOLS.md` after your last change. The
    diff must be empty before you finish. When you resolve a missing entry,
    follow the Step 2 rule: add the export or the wiring if the implementation
    exists, or translate the missing C source. Never add a stub.
