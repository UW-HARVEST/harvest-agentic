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

#[test]
fn zeroed_context() {
    #[derive(Debug, PartialEq)]
    struct Ctx {
        n: u32,
        p: *const u8,
    }
    // SAFETY: zero bytes are a valid Ctx (0 and a null pointer).
    let b = unsafe { CBox::<Ctx>::try_zeroed_in(Malloc) }.unwrap();
    assert_eq!(
        *b,
        Ctx {
            n: 0,
            p: std::ptr::null()
        }
    );
}
