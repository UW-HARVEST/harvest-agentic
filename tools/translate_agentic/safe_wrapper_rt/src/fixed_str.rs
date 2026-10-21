//! A fixed-length string on the stack.

use std::ffi::c_char;
use std::fmt;

/// A string of at most `N` bytes that does not allocate. `write!` into it truncates as C
/// `snprintf` does, and [`FixedStr::wanted`] gives the length without the truncation (the return
/// value of `snprintf`).
#[derive(Clone, Copy)]
pub struct FixedStr<const N: usize> {
    buf: [u8; N],
    len: usize,
    wanted: usize,
}

impl<const N: usize> FixedStr<N> {
    pub const fn new() -> Self {
        FixedStr {
            buf: [0; N],
            len: 0,
            wanted: 0,
        }
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.buf[..self.len]
    }

    pub fn wanted(&self) -> usize {
        self.wanted
    }

    /// Appends bytes, and drops the bytes that do not fit.
    pub fn push_bytes(&mut self, b: &[u8]) {
        let n = b.len().min(N - self.len);
        self.buf[self.len..self.len + n].copy_from_slice(&b[..n]);
        self.len += n;
        self.wanted += b.len();
    }

    /// Writes the string and a NUL into C memory, as `snprintf(dst, cap, ...)`: at most
    /// `cap - 1` bytes and the NUL. Writes nothing when `cap == 0`. The bytes after the NUL do not
    /// change.
    ///
    /// # Safety
    /// `dst` has `cap` writable bytes.
    pub unsafe fn write_c_nul(&self, dst: *mut c_char, cap: usize) {
        if cap == 0 {
            return;
        }
        let n = self.len.min(cap - 1);
        // SAFETY: n + 1 <= cap bytes, by the caller's contract.
        unsafe {
            std::ptr::copy_nonoverlapping(self.buf.as_ptr(), dst.cast::<u8>(), n);
            dst.add(n).write(0);
        }
    }
}

impl<const N: usize> Default for FixedStr<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> fmt::Write for FixedStr<N> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.push_bytes(s.as_bytes());
        Ok(())
    }
}
