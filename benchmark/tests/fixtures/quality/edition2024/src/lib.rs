#![allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" { fn abs(x: i32) -> i32; safe fn labs(x: i64) -> i64; }
static mut G: [u8; 4] = [0; 4];
/// "unsafe { in a doc string" should not be touched
#[unsafe(no_mangle)]
pub unsafe extern "C" fn e1(p: *const i32) -> i32 {
    let s = "unsafe { not code";                                   // string untouched
    let _g = &raw const G;                                          // safe in modern Rust: 0
    abs(*p) + s.len() as i32 + labs(1) as i32                       // [C][D] 2 ; labs is safe
}
#[unsafe(export_name = "e2_sym")]
pub extern "C" fn e2(p: *mut u8) { unsafe { *p.add(1) = 0; } }      // [D][C] 2
pub struct S(*const u8);
impl S { pub fn get(&self) -> u8 { unsafe { *self.0 } } }           // [D] 1
