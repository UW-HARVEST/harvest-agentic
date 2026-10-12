// include_dir! does not track the embedded files on stable Rust: rebuild when agent_skills/ changes.
fn main() {
    println!("cargo:rerun-if-changed=../../agent_skills");
}
