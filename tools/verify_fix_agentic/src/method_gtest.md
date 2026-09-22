## Step 3: Verification workflow (GoogleTest)

You compare C against Rust from **inside a C++ GoogleTest binary**. A ready-to-use
test environment has been placed in `verify_env/`. The C reference is compiled
directly into the test binary; the translated Rust is loaded as a shared library
and called through its C-ABI exports. Every test input runs both sides and
compares the observable result with GoogleTest assertions.

`verify_env/` already handles the infrastructure so you can focus on the API
behavior:

- `CMakeLists.txt` — fetches a pinned GoogleTest, compiles `c_src/` into the test
  binary, links everything, and registers the tests. You normally do not edit it.
- `verification_tests.cc` — the test file you write and extend.
- `harvest_diff.h` — helpers for differential comparison: `FilledBuffer` (0xA5
  pre-fill to expose short/uninitialized writes), `FirstDifference`/`HexDump`/
  `Explain` (attach `<< harvest::Explain(c, rust)` to an `EXPECT_EQ` to get the
  first differing offset and a hex dump on failure), `WithErrno`, and a
  `Observation` struct to extend. Reuse or extend as the API needs.
- `rust_lib.h` — loads the translated Rust `.so` (path from `RUST_LIB_PATH`) with
  `dlopen(..., RTLD_LOCAL)` so its symbols never collide with the statically
  linked C reference of the same name, and resolves functions with `dlsym`.
- `build.sh` — configures and builds the test binary.
- `README.md` — exact build/run commands and pointers.

The C reference and the Rust translation both export the same public symbol
names (e.g. `LZ4_compress_default`). That is why the C side is linked statically
and the Rust side is reached only through `dlopen`/`dlsym` — never link the Rust
`.so` directly into the test binary.

Write fixed-input differential tests with GoogleTest:

```cpp
#include "gtest/gtest.h"

// A fixed-input differential test: one specific case.
TEST(CompressDifferential, EmptyInput) {
  EXPECT_EQ(RunC(/*level=*/9, {}), RunRust(/*level=*/9, {}));
}
```

Then do the actual verification:

1. Build the C reference and the Rust `.so`. `verify_env/README.md` has the exact
   commands; in short, `cargo build --release` for the Rust cdylib and
   `verify_env/build.sh` for the test binary.

   Before you trust any comparison, confirm the C reference is built correctly:
   the CMakeLists compiled `c_src/` into the test binary with compile
   definitions best-effort extracted from `c_src/CMakeLists.txt`, but that parse
   can miss flags (namespace macros like `XXH_NAMESPACE=...`, behavior switches,
   defs added via `add_definitions`/subdirectories). The C reference is your
   ground truth — if it is misbuilt, every "mismatch" is a false one and you
   will waste effort changing correct Rust. Cross-check the
   `target_compile_definitions(c_under_test ...)` line in `verify_env/CMakeLists.txt`
   against how `c_src` is actually built and fix it if it differs. Tell-tale
   symptoms of a misbuilt oracle: a symbol you expect is missing, or every test
   fails together in lock-step.
2. Write GoogleTest cases in `verify_env/verification_tests.cc` that call both the
   C reference and the Rust translation on the same inputs and assert the results
   match byte-for-byte. A fixture (`TEST_F`) is the natural place to hold a
   library context across an `init -> configure -> call -> observe -> destroy`
   sequence, with a fresh context per test.
3. Cover every row of the surface files from Step 2. Each `CONFIGS.md` row
   gets a differential test, and each `ERRORS.md` row gets a rejection test.
   The rejection test asserts that both sides return the same error code or
   sentinel value, not only that both sides fail. Also test the generic
   boundaries from Step 2: null pointers, zero and oversized lengths, values
   one step past a valid range, and enum arguments with no valid variant.
4. Start with the lowest-level functions and work upward. Look at the C headers to
   identify the public API and call hierarchy.
5. Run the test binary. Every time a test exposes a divergence, append a
   hypothesis to `HYPOTHESES.md`.
6. When a Rust function differs from C, fix the Rust code in `src/`, rebuild the
   `.so`, and re-run until the test passes. Update the matching hypothesis to
   `fixed` after the Edit.
7. Keep going until all public functions match.
8. Re-run the symbol comparison from `SYMBOLS.md` after your last change. The
   diff must be empty before you finish. When you resolve a missing symbol,
   follow the Step 2 rule: add the export if the implementation exists, or
   translate the missing C source. Never add a stub.

Normalize before comparing: pointer/allocator addresses, struct padding,
uninitialized bytes, timestamps, and other nondeterministic fields are not
meaningful differences. Initialize output buffers to a fixed pattern (e.g.
`0xA5`) before each call so you can detect one side writing a different range.

{FUZZTEST_SECTION}
