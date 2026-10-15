//! Code quality of a translated crate, graded alongside its tests: safety
//! (operations that require `unsafe`) and idiomaticity (rustc warnings,
//! clippy lints, cognitive complexity). Both follow DARPA TRACTOR's automated
//! metrics; see [`safety`] and [`idiom`] for the methods.
//!
//! Measurements run on scratch copies of the snapshot's package and never
//! modify it. Each program gets `.harvest/quality.json` (the full report)
//! and `.harvest/unsafe_ops.tsv` (one row per unsafe operation, for audit);
//! the run root gets `quality.csv` with one headline row per program. A
//! measurement that cannot be made is recorded as such and never affects the
//! test grade.

pub mod idiom;
mod report;
pub mod safety;
mod scratch;
pub mod source;
mod strip;

use idiom::IdiomReport;
use safety::SafetyReport;
use serde::Serialize;
use std::path::{Path, PathBuf};

/// Full report, in the snapshot's `.harvest/`.
pub const REPORT_FILE: &str = "quality.json";
/// Unsafe operations one per row, in the snapshot's `.harvest/`.
pub const UNSAFE_OPS_FILE: &str = "unsafe_ops.tsv";
/// Headline row per program, at the run's output root.
pub const SUMMARY_CSV: &str = "quality.csv";

/// Why a measurement has no result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MeasureError {
    /// The measurement could not be carried out (no manifest, cargo or
    /// clippy unavailable, I/O).
    NotRun(String),
    /// The crate has compile errors under the measurement. For safety this
    /// includes any error the rewrite provokes besides E0133, because rustc
    /// then skips whole functions and the count would be silently low.
    DoesNotCompile(Vec<String>),
}

impl MeasureError {
    /// For `map_err`: the error, prefixed with what was being done, as
    /// [`MeasureError::NotRun`].
    fn not_run<E: std::fmt::Display>(what: &'static str) -> impl Fn(E) -> Self {
        move |e| Self::NotRun(format!("{what}: {e}"))
    }
}

/// A measurement's result, as recorded in the report.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Outcome<T> {
    Measured { result: T },
    DoesNotCompile { errors: Vec<String> },
    NotRun { reason: String },
}

impl<T> Outcome<T> {
    pub fn result(&self) -> Option<&T> {
        match self {
            Outcome::Measured { result } => Some(result),
            _ => None,
        }
    }

    /// The result, or a one-line account of why there is none.
    pub fn as_result(&self) -> Result<&T, String> {
        match self {
            Outcome::Measured { result } => Ok(result),
            Outcome::DoesNotCompile { errors } => Err(format!(
                "not measured, {} compile error(s); first: {}",
                errors.len(),
                errors
                    .first()
                    .map(|e| e.lines().next().unwrap_or(""))
                    .unwrap_or("")
            )),
            Outcome::NotRun { reason } => Err(format!("not measured: {reason}")),
        }
    }
}

impl<T> From<Result<T, MeasureError>> for Outcome<T> {
    fn from(r: Result<T, MeasureError>) -> Self {
        match r {
            Ok(result) => Outcome::Measured { result },
            Err(MeasureError::DoesNotCompile(errors)) => Outcome::DoesNotCompile { errors },
            Err(MeasureError::NotRun(reason)) => Outcome::NotRun { reason },
        }
    }
}

/// Code quality of one package.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct QualityReport {
    pub schema_version: u32,
    /// `rustc --version` the measurement compiled with.
    pub rustc: Option<String>,
    /// Lines carrying code in the files the library, binaries and build
    /// script compile, each file once (comments and blank lines excluded):
    /// the denominator of the per-1000-lines rates.
    pub rust_code_lines: usize,
    pub safety: Outcome<SafetyReport>,
    pub idiomaticity: Outcome<IdiomReport>,
}

impl QualityReport {
    /// `count` per 1000 code lines.
    pub fn per_kloc(&self, count: usize) -> Option<f64> {
        (self.rust_code_lines > 0).then(|| count as f64 * 1000.0 / self.rust_code_lines as f64)
    }
}

/// Measures the package of the snapshot program directory `program_dir`.
pub fn measure(program_dir: &Path) -> QualityReport {
    let (rust_code_lines, safety, idiomaticity) = match prepare(program_dir) {
        Err(e) => (0, Err(e.clone()), Err(e)),
        Ok((scratch, pkg)) => {
            let idiomaticity = idiom::measure(&scratch, &pkg);
            // Only the files the crate compiles count: the package directory
            // also holds files it does not, such as agent tooling.
            let files = pkg.compiled_sources().and_then(|paths| {
                paths
                    .into_iter()
                    .map(|rel_path| {
                        let text = std::fs::read_to_string(pkg.root.join(&rel_path))
                            .map_err(MeasureError::not_run("read source"))?;
                        Ok(source::analyze(rel_path, &text))
                    })
                    .collect::<Result<Vec<_>, _>>()
            });
            match files {
                Ok(files) => (
                    files.iter().map(|f| f.code_lines).sum(),
                    safety::measure(&scratch, &pkg, &files),
                    idiomaticity,
                ),
                Err(e) => (0, Err(e), idiomaticity),
            }
        }
    };
    QualityReport {
        schema_version: 1,
        rustc: scratch::rustc_version(program_dir),
        rust_code_lines,
        safety: safety.into(),
        idiomaticity: idiomaticity.into(),
    }
}

/// The scratch area and the untouched copy of the package every
/// measurement starts from.
fn prepare(program_dir: &Path) -> Result<(scratch::ScratchArea, scratch::Package), MeasureError> {
    if !program_dir.join("Cargo.toml").is_file() {
        return Err(MeasureError::NotRun("no Cargo.toml".into()));
    }
    // Cargo runs inside the copies, so every path handed to it (target
    // directory, clippy configuration, path dependencies) must be absolute.
    let program_dir =
        std::path::absolute(program_dir).map_err(MeasureError::not_run("resolve path"))?;
    let scratch = scratch::ScratchArea::create(&program_dir)?;
    let pkg = scratch.copy_package(&program_dir, "pristine")?;
    Ok((scratch, pkg))
}

/// Measures every program directory, writes each one's report, logs a
/// summary line per program, and writes the run's summary CSV to `csv_path`
/// if given. Failures are logged, never returned: quality never changes a
/// run's outcome.
pub fn grade_programs(program_dirs: &[PathBuf], csv_path: Option<&Path>) {
    log::info!("\n{}", "=".repeat(80));
    log::info!("Code quality (safety and idiomaticity)");
    log::info!("{}", "=".repeat(80));
    let mut rows = Vec::new();
    for dir in program_dirs {
        let name = dir
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        log::info!("Measuring code quality of {name}...");
        let report = measure(dir);
        if let Err(e) = report::write(dir, &report) {
            log::warn!("  Failed to write the quality report of {name}: {e}");
        }
        report::log_report(&report);
        rows.push((name, report));
    }
    if let Some(csv_path) = csv_path {
        match report::write_summary_csv(csv_path, &rows) {
            Ok(()) => log::info!("  Quality summary: {}", csv_path.display()),
            Err(e) => log::warn!("  Failed to write {}: {e}", csv_path.display()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outcome_records_why_a_measurement_is_missing() {
        let o: Outcome<u8> = Err(MeasureError::DoesNotCompile(vec!["E0425".into()])).into();
        assert_eq!(
            serde_json::to_value(&o).unwrap(),
            serde_json::json!({"status": "does_not_compile", "errors": ["E0425"]})
        );
        let o: Outcome<u8> = Ok(3).into();
        assert_eq!(
            serde_json::to_value(&o).unwrap(),
            serde_json::json!({"status": "measured", "result": 3})
        );
    }

    #[test]
    fn rates_are_per_thousand_code_lines() {
        let r = QualityReport {
            schema_version: 1,
            rustc: None,
            rust_code_lines: 2000,
            safety: Outcome::NotRun { reason: "x".into() },
            idiomaticity: Outcome::NotRun { reason: "x".into() },
        };
        assert_eq!(r.per_kloc(500), Some(250.0));
        let empty = QualityReport {
            rust_code_lines: 0,
            ..r
        };
        assert_eq!(empty.per_kloc(5), None);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn a_directory_without_a_manifest_is_not_run() {
        let dir = tempfile::tempdir().unwrap();
        let report = measure(dir.path());
        assert!(matches!(report.safety, Outcome::NotRun { .. }));
        assert!(matches!(report.idiomaticity, Outcome::NotRun { .. }));
    }
}
