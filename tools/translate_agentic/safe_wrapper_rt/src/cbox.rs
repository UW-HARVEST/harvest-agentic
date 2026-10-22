//! One object from the library allocator, for example an opaque handle that C holds.

use super::alloc::{LibAlloc, Malloc, Oom, checked_block};
use std::mem::ManuallyDrop;
use std::ops::{Deref, DerefMut};
use std::ptr::NonNull;

/// One object from the library allocator (5.7 (a)). `into_raw` gives it to C as a handle, and
/// `from_raw_in` takes it back, for example in the free function of the library.
pub struct CBox<T, A: LibAlloc = Malloc> {
    ptr: NonNull<T>,
    alloc: A,
}

impl<T> CBox<T, Malloc> {
    /// [`CBox::try_new_in`] with C malloc.
    pub fn try_new(v: T) -> Result<Self, Oom> {
        Self::try_new_in(v, Malloc)
    }
}

impl<T, A: LibAlloc> CBox<T, A> {
    /// Allocates the object with one call of the allocator.
    pub fn try_new_in(v: T, alloc: A) -> Result<Self, Oom> {
        let size = size_of::<T>();
        let ptr = checked_block::<T, A>(&alloc, alloc.malloc(size), size)?;
        // SAFETY: the block has room for one T and is aligned for it.
        unsafe { ptr.as_ptr().write(v) };
        Ok(CBox { ptr, alloc })
    }

    /// Allocates the object with one `calloc`, as C does for a context that starts as zero bytes.
    ///
    /// # Safety
    /// All-zero bytes are a valid `T`.
    pub unsafe fn try_zeroed_in(alloc: A) -> Result<Self, Oom> {
        let size = size_of::<T>();
        let ptr = checked_block::<T, A>(&alloc, alloc.calloc(size), size)?;
        Ok(CBox { ptr, alloc })
    }

    /// Gives the object to C. The allocator value is dropped. `from_raw_in` needs it again.
    pub fn into_raw(self) -> *mut T {
        let me = ManuallyDrop::new(self);
        // SAFETY: me is not used again, so the allocator value is read out once.
        drop(unsafe { std::ptr::read(&me.alloc) });
        me.ptr.as_ptr()
    }

    /// Takes the object back from C.
    ///
    /// # Safety
    /// `p` came from `into_raw` with an equal allocator, and C does not use it after this call.
    pub unsafe fn from_raw_in(p: *mut T, alloc: A) -> Self {
        CBox {
            ptr: NonNull::new(p).expect("NULL handle"),
            alloc,
        }
    }
}

impl<T, A: LibAlloc> Deref for CBox<T, A> {
    type Target = T;

    fn deref(&self) -> &T {
        // SAFETY: the object is initialized and lives while self lives.
        unsafe { self.ptr.as_ref() }
    }
}

impl<T, A: LibAlloc> DerefMut for CBox<T, A> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: as in deref, and &mut self is unique.
        unsafe { self.ptr.as_mut() }
    }
}

impl<T, A: LibAlloc> Drop for CBox<T, A> {
    fn drop(&mut self) {
        // SAFETY: the object is initialized and is not used again. The block is a live block of
        // self.alloc.
        unsafe {
            std::ptr::drop_in_place(self.ptr.as_ptr());
            self.alloc.free(self.ptr.as_ptr().cast());
        }
    }
}
