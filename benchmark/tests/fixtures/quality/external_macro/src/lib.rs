use external_macro_dep::{rd, rd_all};
pub fn s1(p: *const u8) -> u8 { unsafe { rd!(p) } }
pub unsafe fn u1(p: *const u8) -> u8 { rd!(p) }
pub unsafe fn u2(p: *const u8) -> u8 { rd_all!(p) }
pub unsafe fn u3(p: *const u8) -> u8 { *p }
