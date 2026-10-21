//! A fixed-length array from the library allocator.

use super::alloc::{LibAlloc, Malloc, Oom, array_size, checked_block};
use std::ops::{Deref, DerefMut};
use std::ptr::NonNull;

/// A fixed-length array from the library allocator, at a stable address until it is dropped. Use
/// `CArray<Cell<T>>` when the caller writes through a pointer that the library gave out.
pub struct CArray<T, A: LibAlloc = Malloc> {
    ptr: NonNull<T>,
    len: usize,
    alloc: A,
}

impl<T> CArray<T, Malloc> {
    /// [`CArray::try_from_fn_in`] with C malloc.
    pub fn try_from_fn(n: usize, f: impl FnMut(usize) -> T) -> Result<Self, Oom> {
        Self::try_from_fn_in(n, Malloc, f)
    }
}

impl<T, A: LibAlloc> CArray<T, A> {
    /// Allocates `n` values with one call of the allocator, also when `n == 0` (as C `malloc(0)`
    /// does), and sets value `i` to `f(i)`.
    pub fn try_from_fn_in(n: usize, alloc: A, mut f: impl FnMut(usize) -> T) -> Result<Self, Oom> {
        let size = array_size::<T>(n)?;
        let ptr = checked_block::<T, A>(&alloc, alloc.malloc(size), size)?;
        // Until all values are written, a panic of f leaks the block. A leak is safe.
        for i in 0..n {
            // SAFETY: the block holds n values of T, and i < n.
            unsafe { ptr.as_ptr().add(i).write(f(i)) };
        }
        Ok(CArray { ptr, len: n, alloc })
    }

    /// The address that goes to C. It comes from the whole block, not from a reference.
    pub fn as_ptr(&self) -> *const T {
        self.ptr.as_ptr()
    }

    pub fn as_mut_ptr(&mut self) -> *mut T {
        self.ptr.as_ptr()
    }

    pub fn as_slice(&self) -> &[T] {
        self
    }

    pub fn as_mut_slice(&mut self) -> &mut [T] {
        self
    }

    /// The start of the values for slices: a block of 0 bytes can be unaligned.
    fn values(&self) -> *mut T {
        if self.len == 0 {
            NonNull::dangling().as_ptr()
        } else {
            self.ptr.as_ptr()
        }
    }
}

impl<T, A: LibAlloc> Deref for CArray<T, A> {
    type Target = [T];

    fn deref(&self) -> &[T] {
        // SAFETY: values() is aligned, and the block holds len initialized values.
        unsafe { std::slice::from_raw_parts(self.values(), self.len) }
    }
}

impl<T, A: LibAlloc> DerefMut for CArray<T, A> {
    fn deref_mut(&mut self) -> &mut [T] {
        // SAFETY: as in deref, and &mut self is unique.
        unsafe { std::slice::from_raw_parts_mut(self.values(), self.len) }
    }
}

impl<T, A: LibAlloc> Drop for CArray<T, A> {
    fn drop(&mut self) {
        // SAFETY: the values are initialized and are not used again. The block is a live block of
        // self.alloc.
        unsafe {
            std::ptr::drop_in_place(std::ptr::slice_from_raw_parts_mut(self.values(), self.len));
            self.alloc.free(self.ptr.as_ptr().cast());
        }
    }
}
