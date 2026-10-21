use super::Counting;
use crate::rt::*;

fn double(cap: usize, need: usize) -> usize {
    (cap * 2).max(need).max(4)
}

#[test]
fn grows_by_the_policy() {
    let c = Counting::default();
    let mut v = CVec::new_in(&c, double);
    assert_eq!(c.reallocs.get(), 0);
    for i in 0..9u32 {
        v.try_push(i).unwrap();
    }
    assert_eq!(v.capacity(), 16);
    assert_eq!((c.mallocs.get(), c.reallocs.get()), (1, 2)); // 4, then 8 and 16
    v.try_extend(&[9, 10]).unwrap();
    assert_eq!(v[10], 10);
    drop(v);
    assert_eq!(c.frees.get(), 1);
}

#[test]
fn failed_growth_keeps_the_values() {
    let c = Counting::failing_after(1);
    let mut v = CVec::new_in(&c, double);
    for i in 0..4u8 {
        v.try_push(i).unwrap();
    }
    assert_eq!(v.try_push(4), Err(Oom));
    assert_eq!(&v[..], &[0, 1, 2, 3]);
}

/// Hooks with malloc and free only: the default realloc copies the values.
struct NoRealloc;

unsafe impl LibAlloc for NoRealloc {
    fn malloc(&self, n: usize) -> *mut u8 {
        Malloc.malloc(n)
    }

    unsafe fn free(&self, p: *mut u8) {
        unsafe { Malloc.free(p) }
    }
}

#[test]
fn hooks_without_realloc() {
    let mut v = CVec::new_in(NoRealloc, double);
    for i in 0..20u16 {
        v.try_push(i).unwrap();
    }
    assert_eq!(v[19], 19);
    v.truncate(2);
    assert_eq!(v.pop(), Some(1));
    assert_eq!(&v[..], &[0]);
}

/// A realloc that returns a block with the wrong alignment for u64.
struct BadRealloc;

unsafe impl LibAlloc for BadRealloc {
    fn malloc(&self, n: usize) -> *mut u8 {
        Malloc.malloc(n)
    }

    unsafe fn free(&self, p: *mut u8) {
        unsafe { Malloc.free(p.with_addr(p.addr() & !7)) }
    }

    unsafe fn realloc(&self, p: *mut u8, old: usize, n: usize) -> *mut u8 {
        unsafe { Malloc.realloc(p, old, n + 8).add(4) }
    }
}

#[test]
fn unaligned_realloc_panics_without_a_double_free() {
    let mut v = CVec::new_in(BadRealloc, double);
    v.try_extend(&[1u64, 2, 3, 4]).unwrap();
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| v.try_push(5)));
    assert!(r.is_err());
    assert_eq!(v.len(), 0);
}

#[test]
fn into_raw_gives_the_block_to_c() {
    let mut v = CVec::new(double);
    v.try_extend(b"hi\0").unwrap();
    let p = v.into_raw();
    // SAFETY: the test plays the C caller, which reads the string and frees it.
    unsafe {
        assert_eq!(std::ffi::CStr::from_ptr(p.cast()).to_bytes(), b"hi");
        let v = CVec::from_raw_parts_in(p, 3, 4, Malloc, double);
        assert_eq!(v.len(), 3);
    }
}
