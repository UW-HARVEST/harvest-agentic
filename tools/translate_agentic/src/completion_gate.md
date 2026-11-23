### Completion gate
The translation is complete only when all of these are true:
1. `cargo build --release` is green for the default features and for every
   configuration (see "Cargo features").
2. When the project has a library, the exported symbols of the Rust library
   match the C build. Compare the `nm -D` output of both.
3. There are no stubs (see "No stubs").
4. The smoke test passes: one quick check on one simple input. For an
   executable, run the Rust `driver` and the C executable once, and compare
   the output and the exit status. For a library, call one basic public
   function once in the Rust library and once in the C library (for example,
   from a small C program in `scratch/`), and compare the results. When the
   project has both, check the executable.
