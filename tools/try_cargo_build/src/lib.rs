//! Checks if a generated Rust project builds by materializing
//! it to a tempdir and running `cargo build --release`.
pub use cargo_metadata::{Artifact, CompilerMessage};
use full_source::CargoPackage;
use harvest_core::cargo_utils::CargoToml;
use harvest_core::tools::{RunContext, Tool};
use harvest_core::{Id, Representation};
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::info;

pub struct TryCargoBuild;
// Either a vector of compiled artifact filenames (on success)
// or a string containing error messages (on failure).
pub type BuildResult = Result<Vec<PathBuf>, Vec<CompilerMessage>>;

/// Validates that the generated Rust project builds by running `cargo build --release`,
/// with the default features, as the grader builds the default configuration.
/// Note: It has a bit of a confusing return type:
/// - If the project builds successfully, it returns Ok(Ok(artifact_filenames)).
/// - If the project fails to build, it returns Ok(Err(error_message)).
/// - If there is an error running cargo, it returns Err.
fn try_cargo_build(project_path: &PathBuf) -> Result<CargoBuildResult, Box<dyn std::error::Error>> {
    info!("Validating that the generated Rust project builds...");

    let mut cargo = CargoToml::open(&project_path.join("Cargo.toml"))?;
    cargo.add_workspace();
    cargo.normalize_name(project_path);
    cargo.save()?;

    let output = Command::new("cargo")
        .arg("build")
        .arg("--release")
        .arg("--message-format=json")
        .current_dir(project_path)
        .output()
        .map_err(|e| {
            format!(
                "Failed to run cargo build in {}: {}",
                project_path.display(),
                e
            )
        })?;

    let mut artifacts = vec![];
    let mut diagnostics = vec![];
    let mut success = false;
    for message in cargo_metadata::Message::parse_stream(output.stdout.as_slice()) {
        let message = message?;
        match message {
            // Compiled artifacts for a particular target
            cargo_metadata::Message::CompilerArtifact(artifact) => artifacts.push(artifact),
            cargo_metadata::Message::CompilerMessage(compiler_message) => {
                diagnostics.push(compiler_message)
            }
            cargo_metadata::Message::BuildFinished(build_finished) => {
                success = build_finished.success
            }
            // Ignore the following variants for now
            cargo_metadata::Message::BuildScriptExecuted(_) => {}
            cargo_metadata::Message::TextLine(_) => {}
            // Non-exhaustive pattern, so need a catch-all
            _ => {}
        }
    }

    if success {
        info!("Project builds successfully!");
    }
    Ok(CargoBuildResult {
        artifacts,
        diagnostics,
        success,
        err: String::from_utf8(output.stderr)?,
    })
}

impl Tool for TryCargoBuild {
    fn name(&self) -> &'static str {
        "try_cargo_build"
    }

    fn run(
        self: Box<Self>,
        context: RunContext,
        inputs: Vec<Id>,
    ) -> Result<Box<dyn Representation>, Box<dyn std::error::Error>> {
        // Get cargo package representation (the first and only arg of try_cargo_build)
        let cargo_package = context
            .ir_snapshot
            .get::<CargoPackage>(inputs[0])
            .ok_or("No CargoPackage representation found in IR")?;
        let output_path = context.config.output.clone();
        cargo_package.materialize(&output_path)?;

        // Validate that the Rust project builds
        Ok(Box::new(try_cargo_build(&output_path)?))
    }
}

/// A Representation that contains the results of running `cargo build`.
#[derive(Clone)]
pub struct CargoBuildResult {
    pub artifacts: Vec<Artifact>,
    pub diagnostics: Vec<CompilerMessage>,
    pub success: bool,
    pub err: String,
}

impl std::fmt::Display for CargoBuildResult {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        writeln!(f, "Built Rust artifact:")?;
        if self.success {
            writeln!(f, "  Build succeeded. Artifacts:")?;
            for filename in self.artifacts.iter().flat_map(|a| &a.filenames) {
                writeln!(f, "    {}", filename)?;
            }
            Ok(())
        } else {
            writeln!(f, "  Build failed: {}", self.err)
        }
    }
}

impl Representation for CargoBuildResult {
    fn name(&self) -> &'static str {
        "cargo_build_result"
    }

    fn materialize(&self, _path: &Path) -> std::io::Result<()> {
        Ok(())
    }
}
