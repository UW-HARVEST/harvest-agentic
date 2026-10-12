//! Embeds the skills of `agent_skills/` into the binary. The translate stage gives a skill to an
//! agent by writing it into the agent's working directory with [`materialize`].

use include_dir::{Dir, include_dir};
use std::fs;
use std::io;
use std::path::Path;

static SKILLS: Dir = include_dir!("$CARGO_MANIFEST_DIR/../../agent_skills");

/// Directories of a skill that agents do not need: the skill's own tests and Python caches.
const SKIPPED_DIRS: &[&str] = &["tests", "__pycache__"];

/// Writes the skill `name` (a directory of `agent_skills/`) into `dest`.
pub fn materialize(name: &str, dest: &Path) -> io::Result<()> {
    let skill = SKILLS.get_dir(name).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("no skill {name} in agent_skills/"),
        )
    })?;
    write_dir(skill, Path::new(name), dest)
}

fn write_dir(dir: &Dir, skill_root: &Path, dest: &Path) -> io::Result<()> {
    for file in dir.files() {
        let rel = file
            .path()
            .strip_prefix(skill_root)
            .expect("skill file outside its skill");
        let out = dest.join(rel);
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(out, file.contents())?;
    }
    for sub in dir.dirs() {
        let skipped = sub
            .path()
            .file_name()
            .is_some_and(|n| SKIPPED_DIRS.iter().any(|s| n == *s));
        if !skipped {
            write_dir(sub, skill_root, dest)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn materializes_tab2rust_without_tests() {
        let dest = tempfile::tempdir().unwrap();
        materialize("tab2rust", dest.path()).unwrap();
        for file in [
            "SKILL.md",
            "references/tool-usage.md",
            "scripts/tab2rust.py",
            "scripts/tab2rust_lib/gdb_extract.py",
        ] {
            assert!(dest.path().join(file).is_file(), "{file} missing");
        }
        assert!(!dest.path().join("tests").exists());
    }

    #[test]
    fn unknown_skill_is_an_error() {
        let dest = tempfile::tempdir().unwrap();
        assert!(materialize("no_such_skill", dest.path()).is_err());
    }
}
