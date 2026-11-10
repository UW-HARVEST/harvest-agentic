The external tests use the **cando2 library runner** (`{SUITE}/runner/` +
`{SUITE}/test_vectors/`). Each `{SUITE}/test_vectors/<config>/` holds the
vectors of one build configuration: `<name>.json`, or `<name>/cando_vector.json`
with an optional `setup` script that prepares the working directory. A
configuration other than `default` lists its build options in
`configuration.json`; the grader builds it with
`cargo build --release --no-default-features --features <list>`, where a boolean
option `NAME` is the feature `NAME` and an option `NAME` with value `v` is the
feature `NAME_v`. When `{SUITE}/CMakePresets.json` exists, its `test` preset is
the `default` configuration, and the harness puts its features in your `default`
features before grading. Build it with them, as below.
The grader builds your crate as a `cdylib` in each
configuration and, for each vector, runs the compiled runner, which loads YOUR
`.so`. Reproduce it for the `default` configuration:

```bash
RUSTUP_TOOLCHAIN={TOOLCHAIN} cargo build --release [--no-default-features --features <the test preset's features>]
# cdylib -> target/release/lib<crate>.so
cp -r {SUITE}/runner scratch/runner   # the suite is read-only: build a copy
RUSTUP_TOOLCHAIN={TOOLCHAIN} cargo build --release --manifest-path scratch/runner/Cargo.toml
# The runner loads <root>/translated_rust/target/release/lib<name>.so, with the
# <name> of `library:` in runner/src/main.rs (another configuration: target-<config>/).
# The name of <root> must end in `_lib`.
mkdir -p scratch/root_lib/translated_rust/target/release
cp target/release/lib<crate>.so scratch/root_lib/translated_rust/target/release/lib<name>.so
# Per vector:
./scratch/runner/target/release/<runner-bin> -t $(pwd)/scratch/root_lib \
    -v {SUITE}/test_vectors/default/<vector>.json --build-config default --rust lib
```

A vector passes when the runner exits 0. Read `{SUITE}/runner/src/main.rs`
and the vector JSON to see what state is compared, and `inputs/c_src` for
the reference semantics.
