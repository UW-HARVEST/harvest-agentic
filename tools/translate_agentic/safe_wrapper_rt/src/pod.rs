//! Plain data, and byte slices viewed as slices of it.

/// Plain data: each bit pattern of its size is a valid value, and it has no padding. Integers,
/// floats, and arrays of them.
///
/// # Safety
/// Implement it only for types with these properties.
pub unsafe trait Pod: Copy + 'static {}

macro_rules! pod {
    ($($t:ty),*) => { $(
        // SAFETY: a primitive number type without padding or invalid values.
        unsafe impl Pod for $t {}
    )* };
}
pod!(
    u8, u16, u32, u64, u128, usize, i8, i16, i32, i64, i128, isize, f32, f64
);

// SAFETY: an array of plain data has no padding between its elements.
unsafe impl<T: Pod, const N: usize> Pod for [T; N] {}

/// Views bytes as values of `T`. Panics when the bytes are not empty and are not aligned for `T`,
/// or when their length is not a multiple of its size.
pub fn from_bytes<T: Pod>(b: &[u8]) -> &[T] {
    let n = check::<T>(b.as_ptr(), b.len());
    if n == 0 {
        return &[];
    }
    // SAFETY: check proved the alignment and the length. Each bit pattern is a valid T.
    unsafe { std::slice::from_raw_parts(b.as_ptr().cast(), n) }
}

/// The mutable form of [`from_bytes`].
pub fn from_bytes_mut<T: Pod>(b: &mut [u8]) -> &mut [T] {
    let n = check::<T>(b.as_ptr(), b.len());
    if n == 0 {
        return &mut [];
    }
    // SAFETY: as in from_bytes. Each T written through the slice is valid bytes.
    unsafe { std::slice::from_raw_parts_mut(b.as_mut_ptr().cast(), n) }
}

fn check<T>(p: *const u8, len: usize) -> usize {
    if len == 0 {
        // C loops 0 times over an empty input. An empty slice can have any address.
        return 0;
    }
    assert!(
        (p as usize).is_multiple_of(align_of::<T>()),
        "bytes not aligned for the type"
    );
    if size_of::<T>() == 0 {
        return 0;
    }
    assert!(
        len.is_multiple_of(size_of::<T>()),
        "byte length not a multiple of the type size"
    );
    len / size_of::<T>()
}
