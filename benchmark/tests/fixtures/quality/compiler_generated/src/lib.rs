use std::fmt::Write;
#[derive(PartialEq, PartialOrd, Eq, Ord, Debug)]
pub enum E { A(u8), B(u16), C }
#[derive(PartialEq)]
pub enum Plain { X, Y }
#[derive(PartialEq, PartialOrd)]
pub enum E2 { A(u8), B(u16) }
pub fn direct(x: u8) -> String { std::fmt::format(format_args!("{}", x)) }
macro_rules! my_fmt { ($($t:tt)*) => { std::fmt::format(format_args!($($t)*)) } }
pub fn local_macro(x: u8) -> String { my_fmt!("{} {}", x, x) }
pub fn via_std(x: u8) -> String { let mut s = format!("{}", x); write!(s, "{}", x).unwrap(); println!("{x}"); s }
pub fn literal_only() -> String { format!("hello") }
pub async fn inner() -> u8 { 1 }
pub async fn outer() -> u8 { inner().await + inner().await }
#[derive(PartialEq)]
pub enum Three { A(u8), B(u16), C(u32) }
#[derive(PartialEq, Eq, Hash)]
pub enum Named { A { x: u8 }, B { y: u8 } }
#[derive(PartialEq)]
pub enum One { A(u8) }
