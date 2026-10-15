mod build_helper;

fn main() {
    build_helper::check();
    let y = 2u8;
    let q = &y as *const u8;
    assert_eq!(unsafe { *q }, 2);
}
