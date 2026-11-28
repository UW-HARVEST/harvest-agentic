# The shape of the result

The framework builds the result and runs tests against it. For this, the result has this shape:

- The result is one Cargo package in the working directory: `Cargo.toml` and `src/`.
- Library: when the C build makes a shared library, the package builds a `cdylib`. Tests load this
  library and call its symbols directly. It must export each symbol that the C shared library
  exports, not only the functions of the public headers. Each exported function has the final
  linker name after all preprocessor renames, the C calling convention, and the exact C
  signature. A public type has the C memory layout.
- Program: when the C build makes an executable, the package has a `[[bin]]` target with the name
  `driver`. Tests run `driver` in place of the C executable.
- Tests build the package with `cargo build --release` and the Rust toolchain that
  `RUSTUP_TOOLCHAIN` names. The `default` features select the CMake default of each build
  option.
- Build options become Cargo features. When `inputs/c_src/configuration.json` exists, its
  `configurable_variables` are the build options, and only those. A boolean option `NAME` is the
  feature `NAME`. A string or integer option `NAME` with the value `v` is the feature `NAME_v`.
  Keep the case of the name. Each value of each option must build with
  `cargo build --release --no-default-features --features <list>`. `<list>` has one feature for
  each option that the configuration sets, and no other feature.
- When the package exists before you start, it still has this shape at the end.
