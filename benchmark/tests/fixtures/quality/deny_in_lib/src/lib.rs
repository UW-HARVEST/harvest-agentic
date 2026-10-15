//! The library denies a clippy lint it triggers, so the clippy run stops
//! here; `cargo check` still compiles the binary and its own module.
#![deny(clippy::needless_return)]

pub fn id(x: u32) -> u32 {
    return x;
}
