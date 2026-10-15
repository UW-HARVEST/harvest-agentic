//! Scratch copies of a translated package, and the cargo runs made on them.
//!
//! Measurements rewrite source and change lint levels, so they never touch
//! the snapshot: the package is copied under the snapshot's own `target/`
//! (excluded from the frozen package, like the gtest build tree) and the
//! copies are removed afterwards. All runs share one target directory, so
//! dependencies are compiled once.
//!
//! Cargo inherits the environment the grader builds with — in particular the
//! rustup toolchain — so the measurement compiles with the same rustc as the
//! graded build. Only the variables that would change lint levels or
//! diagnostics are cleared.

use super::MeasureError;
use cargo_metadata::{CompilerMessage, Message, PackageId, Target, TargetKind};
use harvest_core::cargo_utils::{copy_directory_recursive, CargoToml};
use harvest_core::stage_manifest::{is_reserved_toplevel, BUILD_DIR};
use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Subdirectory of the snapshot's `target/` that holds the scratch area.
const SCRATCH_SUBDIR: &str = "harvest_quality";

/// Link, inside a package copy, to the untouched copy of the package.
const INTACT_LINK: &str = "harvest_intact";

/// Environment variables cleared for every measurement run: anything that
/// would add flags, change lint levels, or reconfigure clippy behind the
/// measurement's back.
const CLEARED_ENV: &[&str] = &[
    "RUSTFLAGS",
    "CARGO_ENCODED_RUSTFLAGS",
    "CARGO_BUILD_RUSTFLAGS",
    "RUSTC_BOOTSTRAP",
    "CLIPPY_ARGS",
    "CLIPPY_CONF_DIR",
    "TRACTOR_UNSAFE_OPS",
];

/// The scratch area of one snapshot; removed on drop.
pub struct ScratchArea {
    root: PathBuf,
}

impl ScratchArea {
    pub fn create(program_dir: &Path) -> Result<Self, MeasureError> {
        let root = program_dir.join(BUILD_DIR).join(SCRATCH_SUBDIR);
        if root.exists() {
            std::fs::remove_dir_all(&root).map_err(MeasureError::not_run("clear scratch area"))?;
        }
        std::fs::create_dir_all(&root).map_err(MeasureError::not_run("create scratch area"))?;
        Ok(Self { root })
    }

    /// Root of the scratch area, for auxiliary files of a measurement.
    pub fn path(&self) -> &Path {
        &self.root
    }

    /// Copies a package (the top level of `program_dir` minus reserved and
    /// hidden entries — a snapshot, or another copy) to a fresh directory
    /// named `name`.
    pub fn copy_package(&self, program_dir: &Path, name: &str) -> Result<Package, MeasureError> {
        let root = self.root.join(name);
        std::fs::create_dir_all(&root).map_err(MeasureError::not_run("create package copy"))?;
        let entries = std::fs::read_dir(program_dir)
            .map_err(MeasureError::not_run("read snapshot directory"))?;
        for entry in entries {
            let entry = entry.map_err(MeasureError::not_run("read snapshot directory"))?;
            let file_name = entry.file_name();
            if is_reserved_toplevel(&file_name) || file_name.to_string_lossy().starts_with('.') {
                continue;
            }
            let dest = root.join(&file_name);
            if entry.path().is_dir() {
                copy_directory_recursive(&entry.path(), &dest)
                    .map_err(MeasureError::not_run("copy package"))?;
            } else {
                std::fs::copy(entry.path(), &dest)
                    .map_err(MeasureError::not_run("copy package"))?;
            }
        }
        let manifest = root.join("Cargo.toml");
        let mut cargo_toml =
            CargoToml::open(&manifest).map_err(MeasureError::not_run("read Cargo.toml"))?;
        // Keep the copy out of any enclosing workspace (the snapshot may sit
        // inside the harvest checkout).
        cargo_toml.add_workspace();
        cargo_toml.absolutize_path_dependencies(program_dir);
        cargo_toml
            .save()
            .map_err(MeasureError::not_run("write Cargo.toml"))?;
        Package::load(root, self.root.join("target"))
    }
}

impl Drop for ScratchArea {
    fn drop(&mut self) {
        if let Err(e) = std::fs::remove_dir_all(&self.root) {
            log::warn!(
                "Failed to remove quality scratch area {}: {e}",
                self.root.display()
            );
        }
    }
}

/// A package copy with what cargo reports about it.
pub struct Package {
    pub root: PathBuf,
    target_dir: PathBuf,
    pub id: PackageId,
    name: String,
    pub lib: Option<Target>,
    pub bins: Vec<Target>,
    pub build_script: Option<Target>,
}

/// The diagnostics of one cargo run.
pub struct CargoRun {
    pub messages: Vec<CompilerMessage>,
    /// Targets of this package that compiled.
    pub compiled: Vec<Target>,
    pub success: bool,
    pub stderr: String,
}

impl Package {
    fn load(root: PathBuf, target_dir: PathBuf) -> Result<Self, MeasureError> {
        let metadata = cargo_metadata::MetadataCommand::new()
            .manifest_path(root.join("Cargo.toml"))
            .no_deps()
            .exec()
            .map_err(MeasureError::not_run("cargo metadata"))?;
        let package = metadata
            .root_package()
            .ok_or_else(|| MeasureError::NotRun("cargo metadata: no root package".into()))?;
        let is_lib = |t: &Target| {
            t.kind.iter().any(|k| {
                matches!(
                    k,
                    TargetKind::Lib
                        | TargetKind::RLib
                        | TargetKind::DyLib
                        | TargetKind::CDyLib
                        | TargetKind::StaticLib
                        | TargetKind::ProcMacro
                )
            })
        };
        Ok(Self {
            lib: package.targets.iter().find(|t| is_lib(t)).cloned(),
            bins: package
                .targets
                .iter()
                .filter(|t| t.kind.contains(&TargetKind::Bin))
                .cloned()
                .collect(),
            build_script: package
                .targets
                .iter()
                .find(|t| t.kind.contains(&TargetKind::CustomBuild))
                .cloned(),
            id: package.id.clone(),
            name: package.name.to_string(),
            root,
            target_dir,
        })
    }

    /// A `cargo` command in the package copy, with the measurement's
    /// environment.
    pub fn cargo(&self, args: &[&str]) -> Command {
        let mut cmd = tool_command("cargo", &self.root);
        cmd.args(args).env("CARGO_TARGET_DIR", &self.target_dir);
        cmd
    }

    /// Runs a cargo command made by [`Package::cargo`] with
    /// `--message-format=json` among its arguments and returns the compiler
    /// messages of this package.
    pub fn run(&self, mut cmd: Command) -> Result<CargoRun, MeasureError> {
        let output = cmd.output().map_err(MeasureError::not_run("run cargo"))?;
        let mut messages = Vec::new();
        let mut compiled = Vec::new();
        for message in Message::parse_stream(output.stdout.as_slice()) {
            match message.map_err(MeasureError::not_run("read cargo output"))? {
                Message::CompilerMessage(m) if m.package_id == self.id => messages.push(m),
                Message::CompilerArtifact(a) if a.package_id == self.id => compiled.push(a.target),
                _ => {}
            }
        }
        Ok(CargoRun {
            messages,
            compiled,
            success: output.status.success(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }

    /// Makes the library (when `lib`) and the build script of this copy
    /// compile from `pristine`'s untouched sources, through a link to it, while
    /// every other target compiles from the copy's own files.
    pub fn compile_from_intact(&self, pristine: &Package, lib: bool) -> Result<(), MeasureError> {
        let redirects: Vec<(&[&str], &Target)> = [
            (&["lib", "path"][..], pristine.lib.as_ref().filter(|_| lib)),
            (&["package", "build"][..], pristine.build_script.as_ref()),
        ]
        .into_iter()
        .filter_map(|(key, target)| Some((key, target?)))
        .collect();
        if redirects.is_empty() {
            return Ok(());
        }
        std::os::unix::fs::symlink(&pristine.root, self.root.join(INTACT_LINK))
            .map_err(MeasureError::not_run("link intact sources"))?;
        let manifest = self.root.join("Cargo.toml");
        let text =
            std::fs::read_to_string(&manifest).map_err(MeasureError::not_run("read Cargo.toml"))?;
        let mut doc: toml_edit::DocumentMut = text
            .parse()
            .map_err(MeasureError::not_run("parse Cargo.toml"))?;
        for (key, target) in redirects {
            // A source outside the package is never rewritten.
            if let Ok(rel) = target.src_path.as_std_path().strip_prefix(&pristine.root) {
                let path = Path::new(INTACT_LINK).join(rel);
                doc[key[0]][key[1]] = toml_edit::value(path.to_string_lossy().into_owned());
            }
        }
        std::fs::write(&manifest, doc.to_string())
            .map_err(MeasureError::not_run("write Cargo.toml"))
    }

    /// The package's own `.rs` files that `cargo check` compiles for its
    /// library, binaries and build script, package-relative. A plain check
    /// compiles every target the patched rustc reports on (a clippy run can
    /// stop at the library on the crate's own `#![deny(…)]`). The files come
    /// from the dep-info it leaves in the shared target directory (`deps/`
    /// for the library and binaries, `build/<package>-<hash>/` for the build
    /// script), which rustc writes before analysis, so a target that fails
    /// to compile still lists them. An error when the check reached none of
    /// the crate.
    pub fn compiled_sources(&self) -> Result<BTreeSet<String>, MeasureError> {
        let check = self.run(self.cargo(&["check", "--message-format=json"]))?;
        let crates: HashSet<String> = self.lib.iter().chain(&self.bins).map(crate_name).collect();
        let is_ours = |file_name: &str, crates: &HashSet<String>| {
            file_name
                .strip_suffix(".d")
                .and_then(|stem| stem.rsplit_once('-'))
                .is_some_and(|(krate, _hash)| crates.contains(krate))
        };
        let mut dep_infos: Vec<PathBuf> = read_dir_paths(&self.target_dir.join("debug/deps"))
            .into_iter()
            .filter(|p| is_ours(&file_name(p), &crates))
            .collect();
        if self.build_script.is_some() {
            let build_crate = HashSet::from(["build_script_build".to_owned()]);
            for dir in read_dir_paths(&self.target_dir.join("debug/build")) {
                if file_name(&dir)
                    .rsplit_once('-')
                    .is_some_and(|(pkg, _)| pkg == self.name)
                {
                    dep_infos.extend(
                        read_dir_paths(&dir)
                            .into_iter()
                            .filter(|p| is_ours(&file_name(p), &build_crate)),
                    );
                }
            }
        }
        let mut files = BTreeSet::new();
        for dep_info in dep_infos {
            let Ok(text) = std::fs::read_to_string(&dep_info) else {
                continue;
            };
            for source in dep_info_sources(&text) {
                let path = Path::new(&source);
                let rel = path.strip_prefix(&self.root).unwrap_or(path);
                // Only files inside the package: they are rewritten in place.
                let inside = rel
                    .components()
                    .all(|c| matches!(c, std::path::Component::Normal(_)));
                if inside && rel.extension().is_some_and(|e| e == "rs") {
                    files.insert(rel.to_string_lossy().replace('\\', "/"));
                }
            }
        }
        if files.is_empty() {
            return Err(MeasureError::NotRun(format!(
                "cargo check compiled none of the crate:\n{}",
                check.stderr
            )));
        }
        Ok(files)
    }

    /// Whether a diagnostic's file is one of this package's own files.
    /// Cargo reports them relative to the package root; dependency and
    /// standard-library sources come as absolute paths.
    pub fn owns_file(&self, file_name: &str) -> bool {
        let path = Path::new(file_name);
        path.is_relative() || path.starts_with(&self.root)
    }
}

/// A toolchain command (`cargo`, `rustc`, `clippy-driver`) run in `dir`,
/// which decides the rustup toolchain, without the variables that would
/// change flags, lint levels or clippy's configuration behind the
/// measurement's back.
pub fn tool_command(program: &str, dir: &Path) -> Command {
    let mut cmd = Command::new(program);
    cmd.current_dir(dir);
    for var in CLEARED_ENV {
        cmd.env_remove(var);
    }
    cmd
}

/// `rustc --version` as resolved in `dir` — the same toolchain resolution
/// cargo performs there.
pub fn rustc_version(dir: &Path) -> Option<String> {
    let out = tool_command("rustc", dir).arg("--version").output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

/// The entries of `dir`; none when it cannot be read.
fn read_dir_paths(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .map(|entries| entries.flatten().map(|e| e.path()).collect())
        .unwrap_or_default()
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

/// The source files of a Makefile-style dep-info file: rustc ends it with
/// one `path:` rule per input.
fn dep_info_sources(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| line.strip_suffix(':'))
        .filter(|path| !path.contains(": "))
        .map(|path| path.replace("\\ ", " "))
        .collect()
}

/// The crate name rustc compiles a target under (`-` becomes `_`), which is
/// what a crate-scoped `RUSTC_BOOTSTRAP` must name.
pub fn crate_name(target: &Target) -> String {
    target.name.replace('-', "_")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dep_info_lists_one_rule_per_input() {
        let text = "/t/deps/x-1.d: src/lib.rs src/a\\ b.rs\n\n\
                    /t/deps/libx-1.rmeta: src/lib.rs src/a\\ b.rs\n\n\
                    src/lib.rs:\nsrc/a\\ b.rs:\n";
        assert_eq!(dep_info_sources(text), ["src/lib.rs", "src/a b.rs"]);
    }
}
