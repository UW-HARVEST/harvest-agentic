//! A reference-counted byte buffer from the library allocator.

use super::alloc::{LibAlloc, Malloc, Oom, checked_block};
use std::cell::Cell;
use std::ptr::NonNull;

/// The start of the block: the reference count, then the bytes.
struct Header<A> {
    refs: Cell<usize>,
    len: usize,
    alloc: A,
}

/// A reference-counted `[Cell<u8>]` from the library allocator, for a library object that a
/// callback can change while the library reads it (reentrant handles). `Clone` only adds a
/// reference. The count is in the same block as the bytes, so one buffer is one allocation of
/// `len` bytes plus a small header.
pub struct CBuf<A: LibAlloc = Malloc> {
    head: NonNull<Header<A>>,
}

impl CBuf<Malloc> {
    /// [`CBuf::try_zeroed_in`] with C malloc.
    pub fn try_zeroed(len: usize) -> Result<Self, Oom> {
        Self::try_zeroed_in(len, Malloc)
    }
}

impl<A: LibAlloc> CBuf<A> {
    /// Allocates `len` bytes set to zero.
    pub fn try_zeroed_in(len: usize, alloc: A) -> Result<Self, Oom> {
        let size = size_of::<Header<A>>().checked_add(len).ok_or(Oom)?;
        let head = checked_block::<Header<A>, A>(&alloc, alloc.malloc(size), size)?;
        // SAFETY: the block has room for the header and len bytes, and is aligned for the header.
        unsafe {
            head.as_ptr().write(Header {
                refs: Cell::new(1),
                len,
                alloc,
            });
            Self::data(head).write_bytes(0, len);
        }
        Ok(CBuf { head })
    }

    fn data(head: NonNull<Header<A>>) -> *mut u8 {
        head.as_ptr()
            .cast::<u8>()
            .wrapping_add(size_of::<Header<A>>())
    }

    fn header(&self) -> &Header<A> {
        // SAFETY: the header lives while a reference exists.
        unsafe { self.head.as_ref() }
    }

    pub fn len(&self) -> usize {
        self.header().len
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The bytes `[off, off + len)`. Panics when they are not inside the buffer.
    pub fn cells(&self, off: usize, len: usize) -> &[Cell<u8>] {
        assert!(
            off <= self.len() && len <= self.len() - off,
            "range outside the buffer"
        );
        // SAFETY: the range is inside the initialized bytes, and Cell<u8> has the layout of u8.
        unsafe { std::slice::from_raw_parts(Self::data(self.head).add(off).cast(), len) }
    }

    /// The address of the bytes that goes to C or to a callback. It comes from the whole block.
    pub fn as_ptr(&self) -> *mut u8 {
        Self::data(self.head)
    }
}

impl<A: LibAlloc> Clone for CBuf<A> {
    fn clone(&self) -> Self {
        let refs = &self.header().refs;
        refs.set(refs.get().checked_add(1).expect("reference count overflow"));
        CBuf { head: self.head }
    }
}

impl<A: LibAlloc> Drop for CBuf<A> {
    fn drop(&mut self) {
        let refs = self.header().refs.get() - 1;
        self.header().refs.set(refs);
        if refs == 0 {
            // SAFETY: this was the last reference. The header is read out once, and the block is a
            // live block of the allocator in it.
            unsafe {
                let Header { alloc, .. } = self.head.as_ptr().read();
                alloc.free(self.head.as_ptr().cast());
            }
        }
    }
}
