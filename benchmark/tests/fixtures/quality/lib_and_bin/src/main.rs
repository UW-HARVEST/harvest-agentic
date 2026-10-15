fn main() { let x = 5u8; let p = &x as *const u8; let v = unsafe { *p }; println!("{}", v); }
