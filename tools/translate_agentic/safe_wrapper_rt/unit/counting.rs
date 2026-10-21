use crate::rt::*;
use std::cell::Cell;

/// Counts the calls of C malloc, and fails the allocations after `budget` of them.
#[derive(Default)]
pub struct Counting {
    pub mallocs: Cell<usize>,
    pub frees: Cell<usize>,
    pub reallocs: Cell<usize>,
    pub budget: Cell<Option<usize>>,
}

impl Counting {
    pub fn failing_after(n: usize) -> Self {
        let c = Counting::default();
        c.budget.set(Some(n));
        c
    }

    fn spend(&self) -> bool {
        match self.budget.get() {
            Some(0) => false,
            Some(n) => {
                self.budget.set(Some(n - 1));
                true
            }
            None => true,
        }
    }
}

unsafe impl LibAlloc for &Counting {
    fn malloc(&self, n: usize) -> *mut u8 {
        self.mallocs.set(self.mallocs.get() + 1);
        if !self.spend() {
            return std::ptr::null_mut();
        }
        Malloc.malloc(n)
    }

    unsafe fn free(&self, p: *mut u8) {
        self.frees.set(self.frees.get() + 1);
        unsafe { Malloc.free(p) }
    }

    unsafe fn realloc(&self, p: *mut u8, old: usize, n: usize) -> *mut u8 {
        self.reallocs.set(self.reallocs.get() + 1);
        if !self.spend() {
            return std::ptr::null_mut();
        }
        unsafe { Malloc.realloc(p, old, n) }
    }
}
