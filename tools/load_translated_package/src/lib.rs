//! Reconstructs a [`CargoPackage`](full_source::CargoPackage) from a pipeline
//! snapshot: the output directory of a previous run.
//!
//! This is the re-entry half of the pipeline's cross-process contract, and it
//! is driven entirely by the snapshot layout (see
//! [`harvest_core::stage_manifest`]): the package is every top-level entry
//! except the framework's own `.harvest/` directory and cargo's `target/`.
//! Nothing has to be listed anywhere, so a file added to the crate directory —
//! by a later stage, by hand, or by a tool nobody thought about — is part of
//! the crate.

use full_source::CargoPackage;
use harvest_core::fs::RawDir;
use harvest_core::stage_manifest::{self, StageManifest};
use harvest_core::tools::{RunContext, Tool};
use harvest_core::{Id, Representation};
use std::ffi::OsStr;
use std::fs::read_dir;
use std::path::{Path, PathBuf};
use tracing::info;

/// Hidden directories that agent runtimes leave behind (`.claude/`,
/// `.opencode/`, `.git/`, ...). Tools already strip these before freezing;
/// skipping them here too keeps a hand-edited snapshot loadable.
fn is_hidden_dir(name: &OsStr) -> bool {
    name.to_string_lossy().starts_with('.')
}

/// Loads the frozen `CargoPackage` from a snapshot program directory.
pub fn load_snapshot(directory: &Path) -> Result<CargoPackage, Box<dyn std::error::Error>> {
    if !stage_manifest::is_snapshot(directory) {
        return Err(format!(
            "{} is not a pipeline snapshot (no {}/{})",
            directory.display(),
            stage_manifest::HARVEST_META_DIR,
            stage_manifest::STAGE_MANIFEST_FILE
        )
        .into());
    }

    let (dir, directories, files) = RawDir::populate_from_toplevel_selected(
        read_dir(directory)?,
        &|name| !stage_manifest::is_reserved_toplevel(name),
        &is_hidden_dir,
    )?;
    if dir.toplevel_entries().next().is_none() {
        return Err(format!(
            "snapshot {} contains no crate files (only {}/)",
            directory.display(),
            stage_manifest::HARVEST_META_DIR
        )
        .into());
    }

    let stages = StageManifest::read_from_dir(directory)
        .map(|m| {
            m.stages
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("+")
        })
        .unwrap_or_else(|_| "unknown".to_owned());
    info!(
        "Loaded snapshot {} (stages: {stages}) with {directories} directories and {files} files",
        directory.display(),
    );
    Ok(CargoPackage { dir })
}

pub struct LoadTranslatedPackage {
    directory: PathBuf,
}

impl LoadTranslatedPackage {
    pub fn new(directory: &Path) -> LoadTranslatedPackage {
        LoadTranslatedPackage {
            directory: directory.into(),
        }
    }
}

impl Tool for LoadTranslatedPackage {
    fn name(&self) -> &'static str {
        "load_translated_package"
    }

    fn run(
        self: Box<Self>,
        _context: RunContext,
        _inputs: Vec<Id>,
    ) -> Result<Box<dyn Representation>, Box<dyn std::error::Error>> {
        Ok(Box::new(load_snapshot(&self.directory)?))
    }
}

#[cfg(all(not(miri), test))]
mod tests {
    use super::*;
    use harvest_core::config::{AgentKind, Stage};
    use std::fs;

    fn write_manifest(dir: &Path) {
        StageManifest {
            schema_version: 1,
            stages: vec![Stage::Translate],
            agent: Some(AgentKind::Claude),
            model: None,
            prompt_mode: "plan".to_owned(),
            harvest_version: "test".to_owned(),
            bench_program: "prog".to_owned(),
            bench_revision: None,
            test_corpus_root: None,
            test_corpus_revision: None,
            reference_modified: Vec::new(),
            created_unix: 0,
        }
        .write_to_dir(dir)
        .unwrap();
    }

    fn snapshot() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        fs::write(p.join("Cargo.toml"), "[package]").unwrap();
        fs::write(p.join("Cargo.lock"), "# lock").unwrap();
        fs::create_dir(p.join("src")).unwrap();
        fs::write(p.join("src/lib.rs"), "// rust").unwrap();
        write_manifest(p);
        // Framework-owned and build output: never part of the package.
        fs::create_dir_all(p.join(".harvest/c_src")).unwrap();
        fs::write(p.join(".harvest/c_src/a.c"), "int x;").unwrap();
        fs::create_dir_all(p.join("target/release")).unwrap();
        fs::write(p.join("target/release/junk"), "x").unwrap();
        dir
    }

    fn toplevel(package: &CargoPackage) -> Vec<String> {
        package
            .dir
            .toplevel_entries()
            .map(|n| n.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn package_is_everything_but_the_reserved_entries() {
        let dir = snapshot();
        let package = load_snapshot(dir.path()).unwrap();
        assert_eq!(toplevel(&package), ["Cargo.lock", "Cargo.toml", "src"]);
        assert!(package.dir.get_file("src/lib.rs").is_ok());
    }

    #[test]
    fn files_added_by_hand_are_picked_up() {
        let dir = snapshot();
        fs::write(dir.path().join("build.rs"), "fn main(){}").unwrap();
        fs::create_dir(dir.path().join("benches")).unwrap();
        fs::write(dir.path().join("benches/b.rs"), "// bench").unwrap();
        let package = load_snapshot(dir.path()).unwrap();
        assert_eq!(
            toplevel(&package),
            ["Cargo.lock", "Cargo.toml", "benches", "build.rs", "src"]
        );
    }

    #[test]
    fn a_directory_without_a_manifest_is_not_a_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("Cargo.toml"), "[package]").unwrap();
        let err = match load_snapshot(dir.path()) {
            Ok(_) => panic!("expected an error for a manifest-less directory"),
            Err(e) => e.to_string(),
        };
        assert!(err.contains("not a pipeline snapshot"), "{err}");
    }

    #[test]
    fn a_snapshot_with_no_crate_files_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        write_manifest(dir.path());
        let err = match load_snapshot(dir.path()) {
            Ok(_) => panic!("expected an error for a crate-less snapshot"),
            Err(e) => e.to_string(),
        };
        assert!(err.contains("no crate files"), "{err}");
    }
}
