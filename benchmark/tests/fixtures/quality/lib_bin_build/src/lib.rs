//! A library, a binary and a build script. The patched rustc reports each
//! target `cargo check` compiles: `util` (compiled by the library and the
//! binary) counts twice; `helper` (binary only), `build.rs` and its module
//! `build_helper` (build script) once each.
pub mod util;
