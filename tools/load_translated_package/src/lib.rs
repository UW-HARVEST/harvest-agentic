//! Reconstructs a [`CargoPackage`](full_source::CargoPackage) from a pipeline
//! snapshot: the output directory of a previous translate (or verify) run.
//!
//! This is the re-entry half of the pipeline's cross-process contract. A run
//! freezes its final `CargoPackage` by materializing it into the output
//! program directory and stamping a `harvest_stage.json` manifest whose
//! `package_entries` list names the top-level entries that belong to the
//! package. This tool reads that manifest and lifts exactly those entries
//! back into the IR, so a later invocation can run the verify stage (or grade)
//! against a frozen translation without re-running the translator.

use full_source::CargoPackage;
use harvest_core::fs::RawDir;
use harvest_core::stage_manifest::{STAGE_MANIFEST_FILE, StageManifest};
use harvest_core::tools::{RunContext, Tool};
use harvest_core::{Id, Representation};
use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fs::read_dir;
use std::path::{Path, PathBuf};
use tracing::{info, warn};

/// Directories that are never part of a frozen package, at any depth:
/// cargo build output and hidden runtime directories (.claude/, .opencode/,
/// .git/, ...) that may appear if a snapshot was graded or inspected in place.
fn is_runtime_dir(name: &OsStr) -> bool {
    let name = name.to_string_lossy();
    name == "target" || name.starts_with('.')
}

/// Loads the frozen `CargoPackage` from a snapshot program directory, driven
/// by the manifest's `package_entries`.
pub fn load_snapshot(directory: &Path) -> Result<CargoPackage, Box<dyn std::error::Error>> {
    let manifest = StageManifest::read_from_dir(directory).map_err(|e| {
        format!(
            "cannot load snapshot {}: no readable {STAGE_MANIFEST_FILE} ({e}). \
             Only outputs produced by a stage-aware run can be used as stage input.",
            directory.display()
        )
    })?;

    let entries: BTreeSet<String> = manifest.package_entries.iter().cloned().collect();
    if entries.is_empty() {
        return Err(format!(
            "{} in {} lists no package_entries",
            STAGE_MANIFEST_FILE,
            directory.display()
        )
        .into());
    }

    // Warn about manifest entries missing on disk (e.g. a manually pruned
    // snapshot); loading proceeds with what exists.
    for name in &entries {
        if !directory.join(name).exists() {
            warn!(
                "manifest lists package entry {name:?}, but it does not exist in {}",
                directory.display()
            );
        }
    }

    let (dir, directories, files) = RawDir::populate_from_toplevel_selected(
        read_dir(directory)?,
        &|name| entries.contains(&*name.to_string_lossy()),
        &is_runtime_dir,
    )?;
    info!(
        "Loaded snapshot {} (stages: {}) with {directories} directories and {files} files",
        directory.display(),
        manifest
            .stages
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("+"),
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

    fn write_manifest(dir: &Path, package_entries: &[&str]) {
        let manifest = StageManifest {
            schema_version: 1,
            stages: vec![Stage::Translate],
            agent: AgentKind::Claude,
            model: None,
            prompt_mode: "plan".to_owned(),
            harvest_version: "test".to_owned(),
            bench_program_dir: PathBuf::from("/bench/prog"),
            test_case_hash: "sha256:test".to_owned(),
            package_entries: package_entries.iter().map(|s| s.to_string()).collect(),
            created_unix: 0,
        };
        manifest.write_to_dir(dir).unwrap();
    }

    #[test]
    fn loads_only_package_entries() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("Cargo.toml"), "[package]").unwrap();
        fs::create_dir(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/lib.rs"), "// rust").unwrap();
        // Sidecars that must not be lifted:
        fs::create_dir(dir.path().join("c_src")).unwrap();
        fs::write(dir.path().join("c_src/a.c"), "int x;").unwrap();
        fs::create_dir(dir.path().join("gtest_suite")).unwrap();
        fs::write(dir.path().join("plan_translate.md"), "plan").unwrap();
        fs::create_dir(dir.path().join("target")).unwrap();
        write_manifest(dir.path(), &["Cargo.toml", "src"]);

        let package = load_snapshot(dir.path()).unwrap();
        let names: Vec<String> = package
            .dir
            .toplevel_entries()
            .map(|n| n.to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["Cargo.toml", "src"]);
        assert!(package.dir.get_file("src/lib.rs").is_ok());
    }

    #[test]
    fn target_dir_is_never_lifted_even_if_listed() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("Cargo.toml"), "[package]").unwrap();
        fs::create_dir(dir.path().join("target")).unwrap();
        fs::write(dir.path().join("target/junk"), "x").unwrap();
        write_manifest(dir.path(), &["Cargo.toml", "target"]);

        let package = load_snapshot(dir.path()).unwrap();
        let names: Vec<String> = package
            .dir
            .toplevel_entries()
            .map(|n| n.to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["Cargo.toml"]);
    }

    #[test]
    fn missing_manifest_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("Cargo.toml"), "[package]").unwrap();
        let err = match load_snapshot(dir.path()) {
            Ok(_) => panic!("expected an error for a manifest-less snapshot"),
            Err(e) => e.to_string(),
        };
        assert!(err.contains(STAGE_MANIFEST_FILE), "{err}");
    }
}
