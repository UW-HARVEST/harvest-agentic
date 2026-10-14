The external tests are a **GoogleTest suite** in `{SUITE}/gtest_suite/`. The
grader builds your crate as a `cdylib`, then builds the suite against
that `.so` and runs **each test in its own process** (a crash or
timeout in one test never hides the others). Reproduce it exactly:

```bash
# 1. Build your crate as a cdylib (Cargo.toml must set crate-type = ["cdylib"]).
RUSTUP_TOOLCHAIN={TOOLCHAIN} cargo build --release
#    The .so lands in target/release/lib<crate>.so

# 2. Configure + build the suite against YOUR freshly built library.
cmake -S {SUITE}/gtest_suite -B scratch/gtest_build -DCMAKE_BUILD_TYPE=Release \
      -DTEST_LIB_PATH=$(pwd)/target/release/lib<crate>.so
cmake --build scratch/gtest_build -j

# 3. Enumerate tests (parameterized cases expand here).
LD_LIBRARY_PATH=$(pwd)/target/release \
    ./scratch/gtest_build/harvest_gtest --gtest_list_tests

# 4. Run one test, in its own process, exactly like the grader.
LD_LIBRARY_PATH=$(pwd)/target/release \
    ./scratch/gtest_build/harvest_gtest --gtest_filter='SuiteName.TestName'
```

Read the failing test's source under `{SUITE}/gtest_suite/` to learn the
exact behavior it asserts, then trace back into `inputs/c_src` for the
reference semantics. Some tests are heavy (tens of seconds) — that is
not a hang. Every public symbol the C library exports must also be
exported by your crate, or the suite fails to link.
