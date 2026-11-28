# Anti-cheating rules

- The library and the program that `cargo build --release` makes must not contain compiled C
  code. They must not link compiled C code, other than external libraries: the libraries that
  the C code links and whose source is not in `inputs/c_src`.
- Do not use a crate that implements, wraps, re-exports or compiles the C code that you
  translate, or a part of it. A `build.rs` must not use files of `inputs/c_src`.
- Do not generate Rust from the C source with a tool that these instructions do not name, for
  example a C-to-Rust transpiler.
- Do not copy Rust code from outside the working directory, for example from another
  translation.
- Do not look elsewhere on the host for test suites, graders, results or other translations.
- Do not leave a placeholder in place of a translation. When you could not finish an item, say so
  at the end. Do not report an item as done when it is not.
