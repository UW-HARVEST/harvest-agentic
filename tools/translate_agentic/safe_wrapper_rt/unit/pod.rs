use crate::rt::*;

#[test]
fn view_and_write() {
    let mut words = [0u32; 2];
    let bytes: &mut [u8] = {
        let p = words.as_mut_ptr().cast::<u8>();
        // SAFETY: the test makes a byte view of its own array.
        unsafe { std::slice::from_raw_parts_mut(p, 8) }
    };
    from_bytes_mut::<u32>(bytes)[1] = 7;
    assert_eq!(from_bytes::<u32>(bytes), &[0, 7]);
    assert_eq!(words[1], 7);
}

#[test]
fn empty_bytes_at_any_address() {
    assert!(from_bytes::<u32>(&[0u8; 3][1..1]).is_empty());
}

#[test]
#[should_panic(expected = "multiple")]
fn wrong_length_panics() {
    let words = [0u32; 2];
    // SAFETY: the test makes a byte view of its own array.
    let bytes: &[u8] = unsafe { std::slice::from_raw_parts(words.as_ptr().cast(), 8) };
    from_bytes::<u32>(&bytes[..3]);
}
