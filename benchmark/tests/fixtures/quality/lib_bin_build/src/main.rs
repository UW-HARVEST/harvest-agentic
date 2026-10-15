mod helper;
mod util;

fn main() {
    let x = 7u8;
    println!("{} {}", util::read(&x), helper::read_twice(&x));
}
