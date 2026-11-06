// The crate's own levels must not hide the counted lints.
#![allow(clippy::all)]
#![deny(warnings)]

use core::ffi::c_int as Cint;
use std::ops::Add;

// Each C integer type once (11), the renaming `use` above and one use of the
// new name (2).
pub fn c_types(
    _a: core::ffi::c_char,
    _b: core::ffi::c_schar,
    _c: core::ffi::c_uchar,
    _d: core::ffi::c_short,
    _e: core::ffi::c_ushort,
    _f: core::ffi::c_int,
    _g: core::ffi::c_uint,
    _h: core::ffi::c_long,
    _i: core::ffi::c_ulong,
    _j: core::ffi::c_longlong,
    _k: std::os::raw::c_ulonglong,
) -> Cint {
    0
}

// Each pointer arithmetic method once on `*const u8` (10) and on `*mut u8`
// (10).
pub unsafe fn const_methods(p: *const u8) -> isize {
    unsafe {
        let q = p.add(1).sub(1).offset(1).wrapping_add(1).wrapping_sub(1);
        let q = q.wrapping_offset(1).byte_add(1).byte_sub(1).byte_offset(1);
        q.offset_from(p)
    }
}

pub unsafe fn mut_methods(p: *mut u8) -> isize {
    unsafe {
        let q = p.add(1).sub(1).offset(1).wrapping_add(1).wrapping_sub(1);
        let q = q.wrapping_offset(1).byte_add(1).byte_sub(1).byte_offset(1);
        q.offset_from(p)
    }
}

// Not pointer arithmetic: integer methods with the same names.
pub fn integers(a: u32, b: u32) -> u32 {
    a.add(b).wrapping_add(1).wrapping_sub(1)
}

// One position, expanded twice: one hit.
macro_rules! next {
    ($p:expr) => {
        $p.wrapping_add(1)
    };
}

pub fn twice(p: *const u8) -> *const u8 {
    next!(next!(p))
}
