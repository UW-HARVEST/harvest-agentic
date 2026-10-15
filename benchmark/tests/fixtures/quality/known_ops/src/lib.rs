#![allow(unsafe_op_in_unsafe_fn)]
#![allow(dead_code)]
// expected ops are annotated: [D]=deref raw ptr, [C]=call unsafe fn, [M]=mutable static, [E]=extern static, [U]=union field, [A]=asm
pub mod big;
static mut COUNTER: i32 = 0;
extern "C" { static environ: *const *const u8; fn abs(x: i32) -> i32; }
union U { a: u32, b: f32 }
unsafe fn helper(p: *const i32) -> i32 { *p }                      // [D] 1 (unsafe fn, lint)
#[no_mangle]
pub unsafe extern "C" fn exported(p: *mut i32, q: *const i32) -> i32 {
    *p = *q;                                                          // [D][D] 2
    COUNTER += 1;                                                     // [M] 1
    helper(q) + abs(*q)                                               // [C][C][D] 3
}                                                                     // exported total 6
pub fn safe_with_block(p: *const i32, u: U) -> u32 {
    let x = unsafe { *p };                                            // [D] 1
    let y = unsafe { u.a };                                           // [U] 1
    let e = unsafe { environ };                                       // [E] 1
    unsafe { core::arch::asm!("nop"); }                               // [A] 1
    let z = unsafe { let pp = &p as *const *const i32; **pp };        // [D][D] 2
    (x + z) as u32 + y + (e as usize as u32)                          // safe fn total 6
}
macro_rules! rd { ($p:expr) => { unsafe { *$p } }; }
pub fn via_macro(p: *const i32) -> i32 { rd!(p) + rd!(p) + rd!(p) }   // [D]x3 = 3
pub unsafe fn with_closure(p: *const i32) -> i32 { let f = || *p; f() } // [D] 1
