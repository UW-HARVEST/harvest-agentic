#![allow(clippy::all)]
#![allow(non_camel_case_types, dead_code)]

mod other;

pub type ctx_ptr = *mut Ctx;

#[repr(C)]
pub struct Ctx {
    pub level: i32,
    pub pos: usize,
}

static mut COUNTER: u32 = 0;

/// 2 derefs (writes through ctx).
unsafe fn reset(ctx: *mut Ctx, level: i32) {
    (*ctx).level = level;
    (*ctx).pos = 0;
}

/// No unsafe operation at all: R2 candidate.
unsafe fn add(a: i32, b: i32) -> i32 {
    a.wrapping_add(b)
}

/// Pseudo-safe: safe signature, raw pointer parameter used in an unsafe block (1 deref).
pub fn peek(p: *const u8) -> u8 {
    unsafe { *p }
}

/// Exported, hidden pointer type: 1 call + 1 static_mut read + 1 static_mut write.
#[no_mangle]
pub unsafe extern "C" fn q_reset(ctx: ctx_ptr) -> i32 {
    reset(ctx, 3);
    COUNTER += 1;
    // A brace and a keyword in a string and a comment must not confuse the lexer: "unsafe { }"
    let _s = "fn fake() { unsafe { } }";
    let _c = '{';
    let _r = r#"}"#;
    0
}

/// Unneeded unsafe block (rustc unused_unsafe) inside an unsafe fn.
pub unsafe fn noisy() -> i32 {
    let x = 1;
    #[allow(unused_unsafe)]
    let y = unsafe { x + 1 };
    return y;
}

/// Range end in an unsafe block: stripping must keep it parsing.
pub fn sum_to(n: *const usize) -> usize {
    let mut s = 0;
    for i in 0..unsafe { *n } {
        s += i;
    }
    s
}
