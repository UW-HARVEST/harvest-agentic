The external tests are **executable/stdout** vectors in
`{SUITE}/test_vectors/`. The grader builds the `driver` binary and, for each
vector, feeds `argv`/`stdin` and compares stdout to the expected
pattern. Reproduce it:

```bash
RUSTUP_TOOLCHAIN={TOOLCHAIN} cargo build --release          # -> target/release/driver
./target/release/driver <argv...>  < <stdin>          # compare stdout to the vector
```

Each `{SUITE}/test_vectors/*.json` gives `argv`, `stdin`, and the expected
`stdout`. Read `inputs/c_src` for the reference behavior.
