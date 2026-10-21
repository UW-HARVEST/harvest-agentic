use crate::rt::*;

#[test]
fn null_and_empty() {
    let null = std::ptr::null::<u8>();
    assert_eq!(unsafe { in_slice(null, 0) }, Ok(&[][..]));
    assert_eq!(unsafe { in_slice(null, 1) }, Err(NullArg));
    assert_eq!(unsafe { in_slice_opt(null, 0) }, None);
    let data = [1u8, 2];
    assert_eq!(unsafe { in_slice_opt(data.as_ptr(), 0) }, Some(&[][..]));
    assert_eq!(unsafe { in_slice(data.as_ptr(), 2) }, Ok(&data[..]));
}
