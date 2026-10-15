pub fn check() {
    let x = 1u8;
    let p = &x as *const u8;
    assert_eq!(unsafe { *p }, 1);
}
