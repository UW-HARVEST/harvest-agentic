// include_dir! does not track the embedded files on stable Rust: rebuild when agent_skills/ or the
// framework items of the safe boundary change.
fn main() {
    println!("cargo:rerun-if-changed=../../agent_skills");
    println!("cargo:rerun-if-changed=safe_wrapper_rt/src");
}
