unsafe extern "C" { pub safe fn abs(x: i32) -> i32; pub fn labs(x: i64) -> i64; pub static environ: *const *const u8; }
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exported(p: *const i32) -> i32 { abs(*p) + unsafe { labs(1) as i32 } }
pub fn env0() -> *const *const u8 { unsafe { environ } }
pub unsafe fn closure_in_ufn(p: *const u8) -> u8 { let g = || unsafe { *p }; g() + *p }
