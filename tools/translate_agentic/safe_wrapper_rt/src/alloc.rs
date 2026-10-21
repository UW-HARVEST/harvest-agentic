//! The library allocator: C malloc, or the allocation hooks that the user of the C library
//! registers.

use std::ffi::c_void;
use std::ptr::NonNull;

/// The alignment that C malloc gives on the supported targets (`alignof(max_align_t)`).
pub const MALLOC_ALIGN: usize = 16;

/// An allocation failed. The C code returns its out-of-memory error at this point.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Oom;

/// The allocator of the C library. The containers of this module allocate only through it, so the
/// number and the order of the allocations are the same as in C.
///
/// Hooks with only malloc and free (no realloc) keep the default `realloc`: malloc, copy, free.
///
/// # Safety
/// `malloc` returns null or a block of at least `n` bytes. `free` and `realloc` accept each block
/// that `malloc` or `realloc` of the same value returned. `realloc` returns null and keeps the old
/// block when it fails. The containers never call `realloc` with `n == 0`, and they check the
/// alignment of each block.
pub unsafe trait LibAlloc {
    fn malloc(&self, n: usize) -> *mut u8;

    /// # Safety
    /// `p` is null or a live block of this allocator. After the call, `p` is not used.
    unsafe fn free(&self, p: *mut u8);

    /// # Safety
    /// `p` is a live block of this allocator with `old` bytes, and `n > 0`. On success, `p` is not
    /// used again.
    unsafe fn realloc(&self, p: *mut u8, old: usize, n: usize) -> *mut u8 {
        let new = self.malloc(n);
        if !new.is_null() {
            // SAFETY: p has old bytes, new has n bytes, and they are different blocks.
            unsafe {
                std::ptr::copy_nonoverlapping(p, new, old.min(n));
                self.free(p);
            }
        }
        new
    }
}

unsafe extern "C" {
    #[link_name = "malloc"]
    fn c_malloc(n: usize) -> *mut c_void;
    #[link_name = "free"]
    fn c_free(p: *mut c_void);
    #[link_name = "realloc"]
    fn c_realloc(p: *mut c_void, n: usize) -> *mut c_void;
}

/// The C malloc, free and realloc.
#[derive(Clone, Copy, Debug, Default)]
pub struct Malloc;

// SAFETY: the C library functions have the contract of the trait.
unsafe impl LibAlloc for Malloc {
    fn malloc(&self, n: usize) -> *mut u8 {
        // SAFETY: malloc accepts any size.
        unsafe { c_malloc(n).cast() }
    }

    unsafe fn free(&self, p: *mut u8) {
        // SAFETY: the caller gives null or a live block of malloc.
        unsafe { c_free(p.cast()) }
    }

    unsafe fn realloc(&self, p: *mut u8, _old: usize, n: usize) -> *mut u8 {
        // SAFETY: the caller gives a live block of malloc, and n > 0.
        unsafe { c_realloc(p.cast(), n).cast() }
    }
}

/// Checks a block of `size` bytes that the allocator returned for `T`: null is `Err(Oom)`. A block
/// that is not aligned for `T` is a bug of the allocator hooks: it is freed, and the call panics.
/// A block of 0 bytes holds no value, so its alignment does not matter.
pub(crate) fn checked_block<T, A: LibAlloc>(
    alloc: &A,
    p: *mut u8,
    size: usize,
) -> Result<NonNull<T>, Oom> {
    const { assert!(size_of::<T>() > 0, "zero-sized types are not supported") };
    assert!(
        align_of::<T>() <= MALLOC_ALIGN,
        "the type needs more alignment than malloc gives"
    );
    let block = NonNull::new(p).ok_or(Oom)?;
    if size > 0 && !(p as usize).is_multiple_of(align_of::<T>()) {
        // SAFETY: p is a live block of alloc, and nothing else holds it.
        unsafe { alloc.free(p) };
        panic!("the library allocator returned a block that is not aligned for the type");
    }
    Ok(block.cast())
}

/// The byte size of `n` values of `T`, or `Err(Oom)` when it overflows.
pub(crate) fn array_size<T>(n: usize) -> Result<usize, Oom> {
    n.checked_mul(size_of::<T>()).ok_or(Oom)
}
