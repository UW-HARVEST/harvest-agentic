//! The Harvest Intermediate Representation ([HarvestIR]), types it depends on (e.g.
//! [Representation]), and utilities for working with them.

pub mod diagnostics;
pub mod fs;
mod id;
pub mod ir;
pub mod llm;
pub mod tools;
pub mod utils;

pub mod test_util;

pub use id::Id;
pub use ir::{HarvestIR, Representation};

pub mod cargo_utils;
pub mod cmake_presets;
pub mod config;
pub mod stage_manifest;
