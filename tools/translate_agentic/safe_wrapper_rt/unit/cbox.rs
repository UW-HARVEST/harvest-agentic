use super::Counting;
use crate::rt::*;

#[test]
fn handle_round_trip() {
    let c = Counting::default();
    let h = CBox::try_new_in(String::from("ctx"), &c)
        .unwrap()
        .into_raw();
    // SAFETY: the test plays C, which gives the handle back to the free function.
    let b = unsafe { CBox::from_raw_in(h, &c) };
    assert_eq!(*b, "ctx");
    drop(b);
    assert_eq!((c.mallocs.get(), c.frees.get()), (1, 1));
}
