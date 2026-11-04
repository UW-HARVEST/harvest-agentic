// include_dir! does not track the embedded files on stable Rust: rebuild when skill/ changes.
fn main() {
    println!("cargo:rerun-if-changed=skill");
}
