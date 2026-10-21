use super::Counting;
use crate::rt::*;
use std::cell::Cell;

#[test]
fn allocates_once_also_for_zero_values() {
    let c = Counting::default();
    let a = CArray::try_from_fn_in(0, &c, |_| 0u32).unwrap();
    assert_eq!(a.len(), 0);
    drop(a);
    let a = CArray::try_from_fn_in(3, &c, |i| i as u32 * 10).unwrap();
    assert_eq!(&a[..], &[0, 10, 20]);
    drop(a);
    assert_eq!((c.mallocs.get(), c.frees.get()), (2, 2));
}

#[test]
fn failure_is_oom() {
    let c = Counting::failing_after(0);
    assert!(CArray::try_from_fn_in(4, &c, |_| 0u8).is_err());
    assert_eq!(c.frees.get(), 0);
}

#[test]
fn caller_writes_through_the_pointer() {
    let a = CArray::try_from_fn(2, |_| Cell::new(0u8)).unwrap();
    let p = a.as_ptr().cast::<u8>().cast_mut();
    // SAFETY: the test plays the C caller. Cell allows the write while a is borrowed.
    unsafe { p.add(1).write(7) };
    assert_eq!(a[1].get(), 7);
}
