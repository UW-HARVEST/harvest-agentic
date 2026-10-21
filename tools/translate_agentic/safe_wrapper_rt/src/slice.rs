//! A caller's `(pointer, length)` input as a slice.

/// A NULL pointer with a length above 0.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NullArg;

/// The caller's `(pointer, length)` input. When `n == 0`, the pointer is not examined. When
/// `n > 0` and the pointer is NULL, returns `Err`.
///
/// # Safety
/// When `n > 0`, `p` points to `n` initialized, aligned values of `T`, and nothing writes them
/// during the call.
pub unsafe fn in_slice<'call, T>(p: *const T, n: usize) -> Result<&'call [T], NullArg> {
    if n == 0 {
        return Ok(&[]);
    }
    if p.is_null() {
        return Err(NullArg);
    }
    // SAFETY: the caller's contract above.
    Ok(unsafe { std::slice::from_raw_parts(p, n) })
}

/// Use this when C makes a difference between "NULL" and "not NULL, length 0".
///
/// # Safety
/// The same as [`in_slice`].
pub unsafe fn in_slice_opt<'call, T>(p: *const T, n: usize) -> Option<&'call [T]> {
    if p.is_null() {
        return None;
    }
    if n == 0 {
        return Some(&[]);
    }
    // SAFETY: the caller's contract above.
    Some(unsafe { std::slice::from_raw_parts(p, n) })
}
