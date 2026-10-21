use crate::rt::*;
use std::ffi::c_char;
use std::fmt::Write;

#[test]
fn truncates_like_snprintf() {
    let mut s = FixedStr::<5>::new();
    write!(s, "code {}", 1234).unwrap();
    assert_eq!(s.as_bytes(), b"code ");
    assert_eq!(s.wanted(), 9);
    let mut out = [b'x' as c_char; 8];
    // SAFETY: out has 8 bytes, and the test passes cap 4.
    unsafe { s.write_c_nul(out.as_mut_ptr(), 4) };
    assert_eq!(out.map(|c| c as u8), *b"cod\0xxxx");
}
