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
- `run_tests.sh` — run the test binary with memory and time limits. Use it for
  every test run.
- `memory_scope.sh` — the memory cgroup launcher that the run scripts share.
  Do not run it directly.

## Build and run

Run these commands from the translated project directory:

```bash
# 1. Build the translated Rust cdylib:
cargo build --release
#    -> target/release/lib<crate>.so

# 2. Build the test binary:
./verify_env/build.sh

# 3. Run the tests, pointing RUST_LIB_PATH at the built cdylib:
RUST_LIB_PATH="$(pwd)/target/release/lib<crate>.so" ./verify_env/run_tests.sh

# GoogleTest flags go after the script name:
RUST_LIB_PATH="$(pwd)/target/release/lib<crate>.so" \
  ./verify_env/run_tests.sh --gtest_filter='Suite.Case*'
```

The C and Rust sides export the same public symbol names; that is why the C side
is linked statically and the Rust side is reached only through `dlopen`/`dlsym`.
A symbol the Rust `.so` fails to export shows up immediately as a failed lookup.

## Memory and time limits

A test with a defect can use all the memory of the machine. For example, a loop
that makes no progress can grow a buffer without end. `run_tests.sh` has
memory protection to stop this. It applies two memory limits and a time limit
to each run:

| Setting | Default | Meaning |
|---------|---------|---------|
| `TEST_AS_LIMIT_MB` | `2048` | Address-space limit for each test process (`ulimit -v`), in MiB |
| `TEST_HARD_LIMIT_MB` | `3072` | cgroup memory limit for the run and its child processes, in MiB, with swap disabled |
| `TEST_TIMEOUT_SECONDS` | `600` | Time limit for the run |

When a test asks for more memory than the address-space limit, the allocation
fails. C code gets `NULL`, C++ code gets `std::bad_alloc`, and Rust stops with
"memory allocation of N bytes failed". GoogleTest then reports the name of the
test, and the machine stays usable. Treat this result as a test failure and
find its cause.

The cgroup limit is a second barrier. It also bounds child processes. Keep it
above the address-space limit so that the first limit fails first. When the
cgroup kills a process, systemd stops the whole run, and the runner exits with
status 137 or 143. When the time limit expires, the runner exits with status
124. In these cases the output can stop in the middle of a test. Run a smaller
`--gtest_filter` to find the test. Core dumps are disabled.

Each call gets its own limits. Parallel calls add up: three parallel runs can
use three times `TEST_HARD_LIMIT_MB`.

The defaults are correct for ordinary tests. If a test really needs more
memory or time, read `run_tests.sh` and override a setting for one run:

```bash
TEST_AS_LIMIT_MB=4096 TEST_HARD_LIMIT_MB=5120 \
  RUST_LIB_PATH="$(pwd)/target/release/lib<crate>.so" \
  ./verify_env/run_tests.sh --gtest_filter='Suite.LargeInput'
```

Record the change and its reason in `HYPOTHESES.md`. Before you increase a
limit, make sure that the growth is not a defect in the test or in the Rust.
Do not run `build-test/verification_tests` directly, and do not remove the
limits to make a failure go away.

The cgroup limit requires Linux cgroup v2 with a working systemd user manager
(`systemd-run --user --scope` and memory-controller support). The runner
checks the applied `memory.max` and `memory.swap.max` inside the scope before
it starts the binary. If this check fails, the runner stops. It never runs the
binary without the cgroup limit. Inside a container or cgroup that already
limits memory, and only there, set `TEST_HARD_LIMIT_MB=0`. This prints a
warning and keeps the address-space limit and the time limit.

{FUZZTEST_README}
