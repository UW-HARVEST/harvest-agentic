//! Helpers shared by the code-quality integration tests.

#![allow(dead_code)] // each test binary uses a different subset

use std::path::{Path, PathBuf};

/// Root of the code-quality fixture crates.
pub fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/quality")
}

/// Copies a directory tree, skipping `target/` and hidden entries.
pub fn copy_tree(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name();
        if name == "target" || name.to_string_lossy().starts_with('.') {
            continue;
        }
        let path = entry.path();
        if path.is_dir() {
            copy_tree(&path, &dst.join(&name));
        } else {
            std::fs::copy(&path, dst.join(&name)).unwrap();
        }
    }
}

/// A private copy of all fixtures (so path dependencies between them
/// resolve); the crate `name` is at `copy.path().join(name)`.
pub fn fixtures_copy() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    copy_tree(&fixtures_root(), dir.path());
    dir
}

/// Writes a one-file library crate into `dir/name`.
pub fn write_crate(dir: &Path, name: &str, lib_rs: &str) -> PathBuf {
    let root = dir.join(name);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n"),
    )
    .unwrap();
    std::fs::write(root.join("src/lib.rs"), lib_rs).unwrap();
    root
}
