use super::Ctx;

/// Calls reset from another file: reset has a caller elsewhere.
pub(crate) unsafe fn other_caller(c: *mut Ctx) {
    super_reset(c);
}

unsafe fn super_reset(c: *mut Ctx) {
    (*c).pos = 7;
}

pub(crate) fn call_reset_twice(c: *mut Ctx) {
    unsafe { crate::reset(c, 1) };
}
