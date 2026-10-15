pub fn read_twice(p: *const u8) -> u16 {
    unsafe { *p as u16 * 2 }
}
