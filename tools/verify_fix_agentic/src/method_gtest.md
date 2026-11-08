## Step 3: Verification workflow (GoogleTest)

You compare C against Rust from **inside a C++ GoogleTest binary**. A ready-to-use
test environment has been placed in `verify_env/`. Every test input runs both
sides and compares the observable result with GoogleTest assertions:

- **Library**: the C reference is compiled into the test binary; the
  translated Rust `.so` is loaded and called through its C-ABI exports.
- **Executable**: the test binary runs the C executable and the Rust
  executable as child processes on the same argv, stdin, and input files.

`verify_env/` already handles the infrastructure so you can focus on behavior:

- `CMakeLists.txt` — fetches a pinned GoogleTest, compiles `inputs/c_src` into the test
  binary, links everything, and registers the tests. You normally do not edit it.
- `verification_tests.cc` — the test file you write and extend.
- `harvest_diff.h` — helpers for differential comparison. For library calls:
  `FilledBuffer` (0xA5 pre-fill to expose short/uninitialized writes),
  `FirstDifference`/`HexDump`/`Explain` (attach `<< harvest::Explain(c, rust)`
  to an `EXPECT_EQ` to get the first difference on failure), `WithErrno`, and
  an `Observation` struct to extend. For executable runs: `Run` (argv, stdin,
  input files, output files to read back) and `RunBoth`, which runs both
  executables on one `Run` in a fresh working directory with a fixed
  environment and returns both results for `EXPECT_EQ` and `Explain`.
- `rust_lib.h` — loads the translated Rust `.so` (path from `RUST_LIB_PATH`) with
  `dlopen(..., RTLD_LOCAL)` so its symbols never collide with the statically
  linked C reference of the same name, and resolves functions with `dlsym`.
- `build.sh` — configures and builds the test binary.
- `run_tests.sh` — runs the test binary with memory and time limits.
  GoogleTest flags such as `--gtest_filter` go after the script name.
- `README.md` — exact build/run commands and pointers.

Always run the tests through `verify_env/run_tests.sh`, also in sub-agents. Do
not run `build-test/verification_tests` directly. The script applies memory and
time limits, so a test with a defect cannot stop the machine. If a test fails
because of these limits, examine the test first. A loop that makes no progress
can use all memory. If the test really needs more, read `verify_env/README.md`
and `run_tests.sh` to understand the limits, and change a setting for that run.

Add this rule to the `### Operational` invariants in `HYPOTHESES.md`, and copy
it into every sub-agent prompt that runs tests:

```
- Run the GoogleTest binary only through `verify_env/run_tests.sh`, never
  `build-test/verification_tests` directly.
```

The C reference and the Rust library both export the same public symbol
names. That is why the C side is linked statically
and the Rust side is reached only through `dlopen`/`dlsym` — never link the Rust
`.so` directly into the test binary.

Write fixed-input differential tests with GoogleTest:

```cpp
#include "gtest/gtest.h"

// A fixed-input differential test of a library call.
// kSomeMode is a placeholder for a mode value from the C source.
TEST(ApiDifferential, EmptyInput) {
  EXPECT_EQ(RunC(kSomeMode, {}), RunRust(kSomeMode, {}));
}

// A fixed-input differential test of an executable run.
TEST(CliDifferential, UnknownOption) {
  harvest::Run run;
  run.args = {"--no-such-option"};
  auto [c, rust] = harvest::RunBoth(run);
  EXPECT_EQ(rust, c) << harvest::Explain(c, rust);
}
```

Then do the actual verification:

1. Build the C reference and the Rust artifacts. `verify_env/README.md` has the
   exact commands; in short, `cargo build --release` for the Rust `.so` and
   executable, the C project's own CMake build for the C executable, and
   `verify_env/build.sh` for the test binary. Run the tests with
   `verify_env/run_tests.sh`.

   Before you trust any comparison, confirm the C reference is built correctly:
   the CMakeLists compiled `inputs/c_src` into the test binary with compile
   definitions best-effort extracted from `inputs/c_src/CMakeLists.txt`, but that parse
   can miss flags (namespace macros like `XXH_NAMESPACE=...`, behavior switches,
   defs added via `add_definitions`/subdirectories). The C reference is your
   ground truth — if it is misbuilt, every "mismatch" is a false one and you
   will waste effort changing correct Rust. Cross-check the
   `target_compile_definitions(c_under_test ...)` line in `verify_env/CMakeLists.txt`
   against how `inputs/c_src` is actually built and fix it if it differs. Tell-tale
   symptoms of a misbuilt oracle: a symbol you expect is missing, or every test
   fails together in lock-step.
2. Write GoogleTest cases in `verify_env/verification_tests.cc` that run the
   C reference and the Rust translation on the same inputs and assert the results
   match byte-for-byte. A fixture (`TEST_F`) is the natural place to hold a
   library context across an `init -> configure -> call -> observe -> destroy`
   sequence, with a fresh context per test.
3. Cover every row of the surface files from Step 2. Each `CONFIGS.md` row
   gets a differential test, and each `ERRORS.md` row gets a rejection test.
   The rejection test asserts that both sides give the same error code,
   sentinel value, or exit status and message, not only that both sides
   fail. Also test the generic boundaries from Step 2. Library code that only
   the executable registers (built-in commands or functions) is covered
   through the executable.
4. Start with the lowest-level entry points and work upward. Look at the C
   headers and the argument parser to identify the public surface and the
   call hierarchy.
5. Run the tests with `verify_env/run_tests.sh`. Every time a test exposes a
   divergence, append a hypothesis to `HYPOTHESES.md`.
6. When the Rust differs from C, fix the Rust code in `src/`, rebuild, and
   re-run until the test passes. Update the matching hypothesis to `fixed`
   after the Edit.
7. Keep going until every public entry point matches.
8. Re-run the comparison from `SYMBOLS.md` after your last change. The
   diff must be empty before you finish. When you resolve a missing entry,
   follow the Step 2 rule: add the export or the wiring if the implementation
   exists, or translate the missing C source. Never add a stub.

Normalize before comparing: pointer/allocator addresses, struct padding,
uninitialized bytes, timestamps, and other nondeterministic fields are not
meaningful differences. Initialize output buffers to a fixed pattern (e.g.
`0xA5`) before each call so you can detect one side writing a different range.
Keep normalization rules narrow and explicit: a broad pattern can hide a real
difference.

{FUZZTEST_SECTION}
