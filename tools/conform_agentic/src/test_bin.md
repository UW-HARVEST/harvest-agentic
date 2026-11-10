The external tests are **executable/stdout** vectors. Each
`{SUITE}/test_vectors/<config>/` holds the vectors of one build configuration:
`<name>.json`, or `<name>/cando_vector.json` with an optional `setup` script
that prepares the working directory. A configuration other than `default` lists
its build options in `configuration.json`; the grader builds it with
`cargo build --release --no-default-features --features <list>`, where a boolean
option `NAME` is the feature `NAME` and an option `NAME` with value `v` is the
feature `NAME_v`. The grader builds the `driver` binary in each configuration
and, for each vector, runs it in a fresh directory with the vector's `argv`,
`stdin` and `env`, and compares stdout to the expected pattern. Reproduce it:

```bash
RUSTUP_TOOLCHAIN={TOOLCHAIN} cargo build --release          # -> target/release/driver
./target/release/driver <argv...>  < <stdin>          # compare stdout to the vector
```

Read `inputs/c_src` for the reference behavior.
