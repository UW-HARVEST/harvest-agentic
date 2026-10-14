The external tests use the **cando2 library runner** (`{SUITE}/runner/` +
`{SUITE}/test_vectors/`). The grader builds your crate as a `cdylib` and, for
each vector, runs the compiled runner with `RUST_ARTIFACTS=1` so it
loads YOUR `.so`. Reproduce it:

```bash
RUSTUP_TOOLCHAIN={TOOLCHAIN} cargo build --release          # cdylib -> target/release/lib<crate>.so
cp -r {SUITE}/runner scratch/runner   # the suite is read-only: build a copy
RUSTUP_TOOLCHAIN={TOOLCHAIN} cargo build --release --manifest-path scratch/runner/Cargo.toml
# Per vector (bare filename; cando2 prepends test_vectors/):
RUST_ARTIFACTS=1 LD_LIBRARY_PATH=$(pwd)/target/release \
    ./scratch/runner/target/release/<runner-bin> -t {SUITE} -v <vector>.json --rust lib
```

A vector passes when the runner exits 0. Read `{SUITE}/runner/src/main.rs`
and the vector JSON to see what state is compared, and `inputs/c_src` for
the reference semantics.
