//! The Rust toolchain contract: the version the agent and the grader must both use.

use std::fs;
use std::path::{Path, PathBuf};
use tracing::warn;

use crate::command_stdout;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RustToolchainContext {
    pub required_version: String,
    pub prompt_block: String,
    /// The Test-Corpus / cando2 checkout the contract was read from, if one
    /// was found. Recorded in the stage manifest so a run resuming from a
    /// snapshot can find it again without the C source's original location.
    pub test_corpus_root: Option<PathBuf>,
}

/// Detects the Rust toolchain contract for a run.
///
/// The version comes from the Test-Corpus / cando2 checkout, found by walking
/// up from `input_path`. That walk only works while the C source still sits
/// inside the corpus, which stops being true once a run resumes from a
/// self-contained snapshot (whose C source lives under the snapshot's own
/// `.harvest/c_src/`). So the corpus root is detected once, on the run that
/// starts from the bench directory, recorded in the stage manifest, and passed
/// back in as `test_corpus_root` by later runs. The resolved root is reported
/// in [`RustToolchainContext::test_corpus_root`] for the caller to persist.
pub fn detect_rust_toolchain_context(
    input_path: &Path,
    test_corpus_root: Option<&Path>,
) -> Result<RustToolchainContext, Box<dyn std::error::Error>> {
    let repo_root = std::env::current_dir()?;
    let root_toolchain = read_toolchain_channel(&repo_root);
    let test_corpus_root = match test_corpus_root {
        Some(root) if root.is_dir() => Some(root.to_path_buf()),
        Some(root) => {
            warn!(
                "recorded Test-Corpus root {} no longer exists; falling back to detection",
                root.display()
            );
            find_test_corpus_root(input_path, &repo_root)
        }
        None => find_test_corpus_root(input_path, &repo_root),
    };
    let test_corpus_toolchain = test_corpus_root
        .as_ref()
        .and_then(|root| read_toolchain_channel(root));
    let cando2_cargo = test_corpus_root
        .as_ref()
        .map(|root| root.join("tools/cando2/Cargo.toml"));
    let cando2_rust_version = cando2_cargo
        .as_ref()
        .and_then(|path| read_cargo_rust_version(path));

    let required_version = cando2_rust_version
        .clone()
        .or_else(|| test_corpus_toolchain.clone())
        .or_else(|| root_toolchain.clone())
        .ok_or("Unable to determine required Rust toolchain version from rust-toolchain.toml or cando2 Cargo.toml")?;

    check_version_match(
        "HARVEST rust-toolchain.toml",
        root_toolchain.as_deref(),
        &required_version,
    )?;
    check_version_match(
        "Test-Corpus rust-toolchain.toml",
        test_corpus_toolchain.as_deref(),
        &required_version,
    )?;
    check_version_match(
        "cando2 rust-version",
        cando2_rust_version.as_deref(),
        &required_version,
    )?;

    let rustc_version = command_stdout("rustc", &["--version"])?;
    let cargo_version = command_stdout("cargo", &["--version"])?;
    let active_rustc = parse_rustc_semver(&rustc_version).ok_or_else(|| {
        format!(
            "Could not parse rustc version from `{}`",
            rustc_version.trim()
        )
    })?;
    if active_rustc != required_version {
        return Err(format!(
            "Rust toolchain mismatch: required {required_version} from Test-Corpus/cando2 contract, but active rustc is {active_rustc} (`{}`)",
            rustc_version.trim()
        )
        .into());
    }

    let prompt_block = format!(
        "### Rust toolchain contract\n\
         - Required Rust toolchain: `{required_version}`\n\
         - Detected rustc: `{}`\n\
         - Detected cargo: `{}`\n\
         - All self-tests and build checks MUST use this exact toolchain. Run Cargo as `RUSTUP_TOOLCHAIN={required_version} cargo ...` (or verify `rustc --version` reports `{required_version}` first). If a different Rust version is active, stop and report an environment/toolchain problem instead of treating test failures as translation bugs.",
        rustc_version.trim(),
        cargo_version.trim(),
    );

    Ok(RustToolchainContext {
        required_version,
        prompt_block,
        test_corpus_root,
    })
}

/// Locates the Test-Corpus / cando2 checkout for `input_path` without
/// performing the toolchain checks, so a caller can record it (see
/// [`detect_rust_toolchain_context`]) before any agent runs.
pub fn locate_test_corpus(input_path: &Path) -> Option<PathBuf> {
    let repo_root = std::env::current_dir().ok()?;
    find_test_corpus_root(input_path, &repo_root)
}

fn find_test_corpus_root(input_path: &Path, repo_root: &Path) -> Option<PathBuf> {
    let absolute = if input_path.is_absolute() {
        input_path.to_path_buf()
    } else {
        repo_root.join(input_path)
    };
    for ancestor in absolute.ancestors() {
        if ancestor.file_name().and_then(|s| s.to_str()) == Some("Test-Corpus") {
            return Some(ancestor.to_path_buf());
        }
    }
    let candidate = repo_root.join("Test-Corpus");
    candidate.exists().then_some(candidate)
}

fn read_toolchain_channel(dir: &Path) -> Option<String> {
    read_quoted_value(&dir.join("rust-toolchain.toml"), "channel")
        .or_else(|| fs::read_to_string(dir.join("rust-toolchain")).ok())
        .map(|s| s.trim().trim_matches('"').to_string())
        .filter(|s| !s.is_empty())
}

fn read_cargo_rust_version(path: &Path) -> Option<String> {
    read_quoted_value(path, "rust-version")
}

fn read_quoted_value(path: &Path, key: &str) -> Option<String> {
    let content = fs::read_to_string(path).ok()?;
    for line in content.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with(key) {
            continue;
        }
        let (_, value) = trimmed.split_once('=')?;
        return Some(value.trim().trim_matches('"').to_string()).filter(|s| !s.is_empty());
    }
    None
}

fn check_version_match(
    label: &str,
    found: Option<&str>,
    required: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(found) = found
        && found != required
    {
        return Err(format!(
            "Rust toolchain contract mismatch: {label} is {found}, required version is {required}"
        )
        .into());
    }
    Ok(())
}

fn parse_rustc_semver(version: &str) -> Option<String> {
    version.split_whitespace().nth(1).map(|v| v.to_string())
}
