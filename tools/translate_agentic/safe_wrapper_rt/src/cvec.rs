//! A growable array from the library allocator.

use super::alloc::{LibAlloc, Malloc, Oom, array_size, checked_block};
use std::ops::{Deref, DerefMut};
use std::ptr::NonNull;

/// The growth policy of the C code: `grow(capacity, needed)` returns the new capacity, at least
/// `needed`. Example: `|cap, need| (cap * 2).max(need)`.
pub type Grow = fn(usize, usize) -> usize;

/// A growable array from the library allocator. It grows only by the C growth policy: `malloc` for
/// the first block, then one `realloc` for each growth, so the allocations are the same as in C.
/// Zero-sized types are not supported.
pub struct CVec<T, A: LibAlloc = Malloc> {
    ptr: Option<NonNull<T>>,
    len: usize,
    cap: usize,
    grow: Grow,
    alloc: A,
}

impl<T> CVec<T, Malloc> {
    /// An empty vector with C malloc. It allocates nothing until the first growth.
    pub fn new(grow: Grow) -> Self {
        Self::new_in(Malloc, grow)
    }
}

impl<T, A: LibAlloc> CVec<T, A> {
    pub fn new_in(alloc: A, grow: Grow) -> Self {
        CVec {
            ptr: None,
            len: 0,
            cap: 0,
            grow,
            alloc,
        }
    }

    /// Takes over a C array of `len` values with room for `cap` values.
    ///
    /// # Safety
    /// `p` is null with `cap == 0`, or a live block of `alloc` with room for `cap` values of `T`,
    /// aligned for `T`, of which the first `len` are initialized. Nothing else frees it.
    pub unsafe fn from_raw_parts_in(
        p: *mut T,
        len: usize,
        cap: usize,
        alloc: A,
        grow: Grow,
    ) -> Self {
        assert!(len <= cap);
        CVec {
            ptr: NonNull::new(p),
            len,
            cap,
            grow,
            alloc,
        }
    }

    pub fn capacity(&self) -> usize {
        self.cap
    }

    /// Grows to exactly `cap` values with one `malloc` (first block) or `realloc`, when the
    /// capacity is smaller. Use this where the C code calculates its own new size.
    pub fn try_reserve_exact(&mut self, cap: usize) -> Result<(), Oom> {
        if cap <= self.cap {
            return Ok(());
        }
        let size = array_size::<T>(cap)?;
        let new = match self.ptr {
            None => self.alloc.malloc(size),
            // SAFETY: p is the live block of self.alloc with room for self.cap values, and
            // size > 0 because cap > 0 and T is not zero-sized. When realloc fails, the old block
            // stays valid and self does not change.
            Some(p) => unsafe {
                let old = size_of::<T>() * self.cap;
                self.alloc.realloc(p.as_ptr().cast(), old, size)
            },
        };
        if new.is_null() {
            return Err(Oom);
        }
        // The old block is gone. If the check below panics, self must not free it again: forget
        // the block and the values first (a leak is safe).
        let len = std::mem::take(&mut self.len);
        self.ptr = None;
        self.cap = 0;
        self.ptr = Some(checked_block::<T, A>(&self.alloc, new, size)?);
        self.len = len;
        self.cap = cap;
        Ok(())
    }

    /// Makes room for `extra` more values by the growth policy.
    fn try_grow_for(&mut self, extra: usize) -> Result<(), Oom> {
        let need = self.len.checked_add(extra).ok_or(Oom)?;
        if need <= self.cap {
            return Ok(());
        }
        let cap = (self.grow)(self.cap, need);
        assert!(
            cap >= need,
            "the growth policy returned a capacity below the need"
        );
        self.try_reserve_exact(cap)
    }

    pub fn try_push(&mut self, v: T) -> Result<(), Oom> {
        self.try_grow_for(1)?;
        // SAFETY: try_grow_for made room for the value at index len.
        unsafe { self.base().add(self.len).write(v) };
        self.len += 1;
        Ok(())
    }

    pub fn try_extend(&mut self, s: &[T]) -> Result<(), Oom>
    where
        T: Clone,
    {
        self.try_grow_for(s.len())?;
        for v in s {
            // SAFETY: try_grow_for made room for s.len() values after len.
            unsafe { self.base().add(self.len).write(v.clone()) };
            self.len += 1;
        }
        Ok(())
    }

    /// The address that goes to C, or null before the first growth.
    pub fn as_ptr(&self) -> *const T {
        self.ptr.map_or(std::ptr::null(), |p| p.as_ptr())
    }

    /// Gives the block to C, for example as a return value that the caller frees with the free
    /// of the library allocator. Null when the vector never allocated.
    pub fn into_raw(self) -> *mut T {
        let p = self.as_ptr().cast_mut();
        std::mem::forget(self);
        p
    }

    pub fn as_mut_ptr(&mut self) -> *mut T {
        self.ptr.map_or(std::ptr::null_mut(), |p| p.as_ptr())
    }

    /// Drops the values from index `len` on, as C does with `v->len = len`. Keeps the block.
    pub fn truncate(&mut self, len: usize) {
        while self.len > len {
            self.pop();
        }
    }

    pub fn pop(&mut self) -> Option<T> {
        self.len = self.len.checked_sub(1)?;
        // SAFETY: the value at the old last index is initialized, and len no longer covers it.
        Some(unsafe { self.base().add(self.len).read() })
    }

    /// The start of the values for slices.
    fn base(&self) -> *mut T {
        self.ptr
            .map_or(NonNull::dangling().as_ptr(), |p| p.as_ptr())
    }
}

impl<T, A: LibAlloc> Deref for CVec<T, A> {
    type Target = [T];

    fn deref(&self) -> &[T] {
        // SAFETY: base() is aligned and non-null, and the first len values are initialized.
        unsafe { std::slice::from_raw_parts(self.base(), self.len) }
    }
}

impl<T, A: LibAlloc> DerefMut for CVec<T, A> {
    fn deref_mut(&mut self) -> &mut [T] {
        // SAFETY: as in deref, and &mut self is unique.
        unsafe { std::slice::from_raw_parts_mut(self.base(), self.len) }
    }
}

impl<T, A: LibAlloc> Drop for CVec<T, A> {
    fn drop(&mut self) {
        // SAFETY: the first len values are initialized and are not used again. The block is null
        // or a live block of self.alloc.
        unsafe {
            std::ptr::drop_in_place(std::ptr::slice_from_raw_parts_mut(self.base(), self.len));
            if let Some(p) = self.ptr {
                self.alloc.free(p.as_ptr().cast());
            }
        }
    }
}
