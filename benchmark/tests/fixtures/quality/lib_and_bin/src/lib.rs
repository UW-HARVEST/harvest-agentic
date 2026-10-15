// A: fragment duplicated by a local macro -> 2 ops with identical span
macro_rules! twice { ($e:expr) => {{ let a = $e; let b = $e; a + b }} }
pub fn a_safe(p: *const u32) -> u32 { unsafe { twice!(*p) } }
pub unsafe fn a_ufn(p: *const u32) -> u32 { twice!(*p) }
pub unsafe fn a_ufn3(p: *const u32) -> u32 { twice!(*p) + twice!(*p) }
// A2: C-style MAX macro
macro_rules! max { ($a:expr, $b:expr) => { if $a > $b { $a } else { $b } } }
pub fn a2(p: *const i32, q: *const i32) -> i32 { unsafe { max!(*p, *q) } }
// B: builtin macro asm!
pub fn b_safe() { unsafe { core::arch::asm!("nop") } }
pub unsafe fn b_ufn() { core::arch::asm!("nop") }
// C: const used in an array length
pub const unsafe fn three() -> usize { 3 }
pub const N: usize = unsafe { three() };
pub fn c(p: *const u8) -> u8 { let a = [0u8; N]; let v = unsafe { *p }; v + a[0] }
// D: deprecated-safe fn in edition 2021, inside and outside an unsafe block
pub fn d() { unsafe { std::env::set_var("A", "B") } }
pub fn d2() { std::env::remove_var("A") }
// E: baseline, statics, union
pub static mut COUNTER: u32 = 0;
pub union U { a: u32, b: f32 }
pub fn e(p: *mut u8, u: U) -> u32 { unsafe { *p = 1; *p.add(1) = 2; COUNTER += 1; u.a } }
// F: range-end misparse case (needs the `'z:` label)
pub struct Ctx { pub n: usize }
pub fn f(ctx: *const Ctx) -> usize { let mut s = 0; for i in 0..unsafe { (*ctx).n } { s += i; } s }
