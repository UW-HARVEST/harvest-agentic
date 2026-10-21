use crate::rt::*;
use std::ffi::c_int;

static LEVEL: CShared<c_int> = CShared::new(3);
static NAME: CShared<*const std::ffi::c_char> = CShared::new(std::ptr::null());
static COUNTER: CShared<usize> = CShared::new(0);

#[test]
fn pointer_global() {
    NAME.set(c"x".as_ptr());
    assert!(!NAME.get().is_null());
}

#[test]
fn threads_do_not_race() {
    let t = std::thread::spawn(|| (0..50).for_each(|i| COUNTER.set(i)));
    for _ in 0..50 {
        let _ = COUNTER.get();
    }
    t.join().unwrap();
}

#[test]
fn global_read_and_write() {
    assert_eq!(LEVEL.get(), 3);
    // SAFETY: the test plays a C caller that writes the global.
    unsafe { LEVEL.as_ptr().write(5) };
    assert_eq!(LEVEL.get(), 5);
    LEVEL.set(3);
}

#[test]
fn errno_round_trip() {
    set_errno(34);
    assert_eq!(errno(), 34);
}
