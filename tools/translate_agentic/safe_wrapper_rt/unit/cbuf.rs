use super::Counting;
use crate::rt::*;

#[test]
fn shared_bytes_and_one_free() {
    let c = Counting::default();
    let a = CBuf::try_zeroed_in(8, &c).unwrap();
    let b = a.clone();
    a.cells(2, 1)[0].set(5);
    assert_eq!(b.cells(0, 8)[2].get(), 5);
    // SAFETY: the test plays a C callback that writes through the address.
    unsafe { b.as_ptr().add(7).write(9) };
    assert_eq!(a.cells(7, 1)[0].get(), 9);
    drop(a);
    assert_eq!(c.frees.get(), 0);
    drop(b);
    assert_eq!((c.mallocs.get(), c.frees.get()), (1, 1));
}

#[test]
#[should_panic(expected = "outside")]
fn range_outside_panics() {
    CBuf::try_zeroed(4).unwrap().cells(3, 2);
}
