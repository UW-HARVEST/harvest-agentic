use crate::rt::*;

#[test]
fn size_overflow_is_oom() {
    assert!(CArray::try_from_fn(usize::MAX, |_| 0u64).is_err());
}

#[test]
#[should_panic(expected = "more alignment")]
fn over_aligned_type_panics() {
    #[repr(align(32))]
    struct Big(#[allow(dead_code)] u8);
    let _ = CBox::try_new(Big(0));
}
