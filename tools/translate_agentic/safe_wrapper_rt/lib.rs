//! Test harness of the framework items. `src/` is what the Safe API Wrapper Designer copies into a
//! translated crate (as `src/wrap/rt/`), so the tests live outside it, in `unit/`.

#[path = "src/mod.rs"]
pub mod rt;

#[cfg(test)]
mod unit;
