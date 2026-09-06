# verify_env — differential verification environment

A ready-to-use GoogleTest environment for comparing the original C reference
against the translated Rust. The C reference (`../c_src/`) is compiled into the
test binary; the translated Rust cdylib is loaded as a black box via `dlopen`.

## Files

- `CMakeLists.txt` — fetches a pinned GoogleTest, compiles `../c_src/src/*.c`
  into the test binary, links, and registers tests. Normally unedited; the one
  spot you may need to touch is the `target_compile_definitions(c_under_test ...)`
  line if the C reference needs build flags this scaffold did not pick up.
- `verification_tests.cc` — the test file you write. Declare the C functions in
  the `extern "C"` block; resolve the Rust ones via `harvest::RustLib`.
- `harvest_diff.h` — differential-comparison helpers (buffer fill pattern,
  normalized observation struct).
- `rust_lib.h` — loads the translated `.so` (path from `RUST_LIB_PATH`) with
  `RTLD_LOCAL` and resolves symbols.
- `build.sh` — build in unit-test mode.

## Build and run

```bash
# 1. Build the translated Rust cdylib (from the translated_rust/ directory):
cargo build --release
#    -> target/release/lib<crate>.so

# 2. Build the test binary:
./build.sh

# 3. Run, pointing RUST_LIB_PATH at the built cdylib:
RUST_LIB_PATH=$(pwd)/../target/release/lib<crate>.so ./build-test/verification_tests
```

The C and Rust sides export the same public symbol names; that is why the C side
is linked statically and the Rust side is reached only through `dlopen`/`dlsym`.
A symbol the Rust `.so` fails to export shows up immediately as a failed lookup.

{FUZZTEST_README}
