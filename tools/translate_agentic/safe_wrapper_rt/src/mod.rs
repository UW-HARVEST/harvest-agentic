//! Framework items for the safe boundary of a C-to-Rust translation (`--safe-wrapper`).
//!
//! The Safe API Wrapper Designer copies the parts of this directory that its design uses into the
//! translated crate as `src/wrap/rt/` ("rt": the runtime support that the translated code calls).
//! The items are the helpers that the boundary design guideline names: the library allocator, the
//! containers on it, `(pointer, length)` conversion, a fixed-length string, exported mutable
//! globals, errno and plain data. Each `unsafe` block here has its basis in a `// SAFETY:` comment.
//!
//! | Item | Use |
//! |---|---|
//! | [`LibAlloc`], [`Malloc`], [`Oom`] | The library allocator: C malloc or the user's allocation hooks. Failure is `Err(Oom)`, never an abort |
//! | [`in_slice`], [`in_slice_opt`], [`NullArg`] | A caller's `(pointer, length)` input as a slice |
//! | [`CBox`] | One object from the library allocator, for example an opaque handle |
//! | [`CArray`] | Fixed-length array from the library allocator, at a stable address |
//! | [`CVec`] | Growable array from the library allocator, with the growth policy of the C code |
//! | [`CBuf`] | Reference-counted `[Cell<u8>]` from the library allocator |
//! | [`FixedStr`] | Fixed-length string on the stack, written like `snprintf` |
//! | [`CShared`], [`SharedValue`] | Exported mutable global (integer or pointer) that C and Rust both read and write |
//! | [`errno`], [`set_errno`] | The C errno |
//! | [`Pod`], [`from_bytes`], [`from_bytes_mut`] | Plain data, and byte slices viewed as slices of it |
//!
//! `carray.rs`, `cbox.rs`, `cbuf.rs` and `cvec.rs` need `alloc.rs`. The other files stand alone.

// A translation does not use every item.
#![allow(dead_code, unused_imports)]

mod alloc;
mod carray;
mod cbox;
mod cbuf;
mod cvec;
mod fixed_str;
mod pod;
mod shared;
mod slice;

pub use alloc::{LibAlloc, MALLOC_ALIGN, Malloc, Oom};
pub use carray::CArray;
pub use cbox::CBox;
pub use cbuf::CBuf;
pub use cvec::CVec;
pub use fixed_str::FixedStr;
pub use pod::{Pod, from_bytes, from_bytes_mut};
pub use shared::{CShared, SharedValue, errno, set_errno};
pub use slice::{NullArg, in_slice, in_slice_opt};
