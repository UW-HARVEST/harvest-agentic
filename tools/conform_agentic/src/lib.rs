//! Agentic conformance ("conform") tool — the third refinement stage.
//!
//! Like translate and verify, conform runs inside the IR pipeline: it consumes
//! the [`CargoPackage`] produced by an earlier stage (or loaded from a
//! snapshot), refines it, and produces a new `CargoPackage`.
//!
//! The distinguishing feature versus verify is its extra input: the
//! [`ExternalTestSuite`] is **revealed** to the agent, whose sole objective is
//! to make every external test pass. The research question this serves: if the
//! external tests are provided, can a third agent close the gap between
//! "passes its own internally generated tests" and "passes the external
//! tests"?
//!
//! Two properties are load-bearing and enforced here:
//!
//! - **The suite reaches only this stage.** It enters the pipeline as its own
//!   representation, and only this tool declares it as an input, so rounds 1
//!   and 2 cannot see the external tests by construction.
//! - **The agent's edits to the suite are discarded and reported.** The suite
//!   directories are read-only inputs guarded by
//!   [`ReferenceGuard`](harvest_core::fs::ReferenceGuard): they are stripped
//!   before the working directory is frozen, so an edited suite can neither be
//!   graded nor leak into a later stage, and an edit leaves a warning plus a
//!   preserved copy instead of no trace at all.

use agent_runner::{AgentInvocation, AgentPhase};
use full_source::{CargoPackage, ExternalTestSuite, RawSource, TestSuiteKind};
use harvest_core::config::unknown_field_warning;
use harvest_core::fs::{RawDir, ReferenceGuard, collect_symlinks, remove_hidden_entries};
use harvest_core::tools::{RunContext, Tool};
use harvest_core::{Id, Representation};
use serde::Deserialize;
use std::collections::HashMap;
use std::fs::{self, read_dir};
use std::path::{Path, PathBuf};
use tracing::{info, warn};

const PROMPT_CONFORM: &str = include_str!("prompt_conform.md");

/// The agent's persistent notes file (anti-compaction log) and its final
/// gap-analysis report. Both are copied out to sidecar paths so a later
/// stage's freeze cannot lose them.
const CONFORM_NOTES: &str = "CONFORM.md";
const CONFORM_REPORT: &str = "CONFORM_REPORT.md";

/// Tool-specific configuration, read from `[tools.conform_agentic]`.
#[derive(Debug, Deserialize)]
pub struct Config {
    /// Override path for the conform prompt.
    pub prompt_conform: Option<PathBuf>,

    /// Agent timeout in seconds.
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,

    /// Agent model to use. If absent, no --model flag is passed and the CLI
    /// uses its default.
    pub model: Option<String>,

    /// Extra environment variables to inject into the agent process.
    #[serde(default)]
    pub env: HashMap<String, String>,

    /// Destination path for the benchmark's output.log file, so the agent's
    /// full trace is appended to the same log as benchmark messages.
    pub output_log_path: Option<PathBuf>,

    /// Directory where a read-only reference the agent modified is preserved
    /// for review (`.harvest/rejected/<stage>/`). Injected by the benchmark.
    pub rejected_output_dir: Option<PathBuf>,

    /// The Test-Corpus / cando2 checkout defining the Rust toolchain contract.
    /// Injected from the stage manifest when resuming from a snapshot, whose
    /// C source no longer sits inside the corpus.
    pub test_corpus_root: Option<PathBuf>,

    /// Destination path for the agent's `CONFORM.md` notes.
    pub notes_output_path: Option<PathBuf>,

    /// Destination path for the agent's `CONFORM_REPORT.md` gap analysis.
    pub report_output_path: Option<PathBuf>,

    #[serde(flatten)]
    unknown: HashMap<String, serde_json::Value>,
}

fn default_timeout_secs() -> u64 {
    36000
}

impl Config {
    fn validate(&self) {
        unknown_field_warning("tools.conform_agentic", &self.unknown);
    }
}

pub struct ConformAgentic;

impl Tool for ConformAgentic {
    fn name(&self) -> &'static str {
        "conform_agentic"
    }

    fn run(
        self: Box<Self>,
        context: RunContext,
        inputs: Vec<Id>,
    ) -> Result<Box<dyn Representation>, Box<dyn std::error::Error>> {
        let default_config = serde_json::Value::Object(Default::default());
        let config = Config::deserialize(
            context
                .config
                .tools
                .get("conform_agentic")
                .unwrap_or(&default_config),
        )?;
        config.validate();

        let cargo_package = context
            .ir_snapshot
            .get::<CargoPackage>(inputs[0])
            .ok_or("No CargoPackage representation found in IR")?;
        let raw_source = context
            .ir_snapshot
            .get::<RawSource>(inputs[1])
            .ok_or("No RawSource representation found in IR")?;
        let suite = context
            .ir_snapshot
            .get::<ExternalTestSuite>(inputs[2])
            .ok_or("No ExternalTestSuite representation found in IR")?;

        // case_dir/
        //   translated_rust/        <- the Rust project to refine (work dir)
        //     c_src/                <- C reference (semantic ground truth)
        //     <suite dirs>/         <- external tests, revealed to the agent
        let work_dir = tempfile::tempdir()?;
        let case_dir = work_dir.path();
        let translated = case_dir.join("translated_rust");
        cargo_package.dir.materialize(&translated)?;

        let c_src_dir = translated.join("c_src");
        fs::create_dir_all(&c_src_dir)?;
        raw_source.dir.materialize(&c_src_dir)?;

        suite.dir.materialize(&translated)?;
        // Both the C source and the suite are read-only inputs. Guarding the
        // suite is what keeps this stage honest: an agent that "passes" by
        // editing the tests is caught here rather than rewarded, since the
        // edit is discarded and reported instead of frozen.
        let references: Vec<&str> = std::iter::once("c_src")
            .chain(suite.kind.dirs().iter().copied())
            .collect();
        let reference_guard = ReferenceGuard::capture(&translated, &references)?;
        info!(
            "Conform working directory: {} ({} suite revealed)",
            case_dir.display(),
            suite.kind
        );

        let agent = context.config.agentic_agent;
        let rust_toolchain_context = agent_runner::detect_rust_toolchain_context(
            &context.config.input,
            config.test_corpus_root.as_deref(),
        )?;
        let model_limits = match (agent, &config.model) {
            (harvest_core::config::AgentKind::OpenCode, Some(model)) => {
                let limits = agent_runner::load_opencode_model_limits(model)?;
                agent_runner::render_model_limits_block(&limits)
            }
            _ => String::new(),
        };
        let prompt_template = match &config.prompt_conform {
            Some(path) => fs::read_to_string(path)?,
            None => PROMPT_CONFORM.to_owned(),
        };
        let prompt = prompt_template
            .replace(
                "{AGENT_BUG_WORKAROUNDS}",
                agent_runner::agent_bug_workarounds(agent),
            )
            .replace(
                "{CONFORM_TEST_INSTRUCTIONS}",
                &test_instructions(suite.kind, &rust_toolchain_context.required_version),
            )
            .replace("{EXTERNAL_TEST_DIRS}", &suite_dir_list(suite.kind))
            .replace(
                "{RUST_TOOLCHAIN_CONTEXT}",
                &rust_toolchain_context.prompt_block,
            )
            .replace(
                "{WORKDIR_BOUNDARY}",
                &agent_runner::render_workdir_boundary(agent, &translated),
            )
            .replace("{MODEL_LIMITS}", &model_limits);

        agent_runner::invoke_agent(AgentInvocation {
            phase: AgentPhase::Conform,
            agent,
            work_dir: &translated,
            prompt: &prompt,
            timeout_secs: config.timeout_secs,
            model: config.model.as_deref(),
            no_plan: false,
            no_plan_file: false,
            extra_env: &config.env,
            output_log_path: config.output_log_path.as_deref(),
            rust_toolchain: Some(&rust_toolchain_context.required_version),
        })?;
        info!("Conformance refinement complete");

        copy_out(
            &translated,
            CONFORM_NOTES,
            config.notes_output_path.as_deref(),
        );
        copy_out(
            &translated,
            CONFORM_REPORT,
            config.report_output_path.as_deref(),
        );
        if !translated.join(CONFORM_REPORT).is_file() {
            warn!(
                "conform agent did not produce {CONFORM_REPORT} (internal-vs-external gap analysis missing)"
            );
        }

        // Sanitize the working directory before freezing it into the IR.
        remove_hidden_entries(&translated)?;
        reference_guard.strip(&translated, config.rejected_output_dir.as_deref())?;
        let target_out = translated.join("target");
        if target_out.exists()
            && let Err(e) = fs::remove_dir_all(&target_out)
        {
            warn!(
                "Failed to remove {} before freeze: {e}",
                target_out.display()
            );
        }
        for entry in collect_symlinks(&translated) {
            warn!("translated_rust contains symlink: {}", entry);
        }

        let (dir, directories, files) = RawDir::populate_from(read_dir(&translated)?)?;
        info!("Produced CargoPackage with {directories} directories and {files} files");
        Ok(Box::new(CargoPackage { dir }))
    }
}

/// Copies an agent-written file out of the working directory before the
/// tempdir is dropped. Absence is normal (the agent may skip the step).
fn copy_out(translated: &Path, name: &str, dest: Option<&Path>) {
    let src = translated.join(name);
    let Some(dest) = dest else { return };
    if !src.is_file() {
        return;
    }
    match fs::copy(&src, dest) {
        Ok(_) => info!("{name} written to {}", dest.display()),
        Err(e) => warn!("Failed to copy {name} to {}: {e}", dest.display()),
    }
}

/// Renders the suite's directories as the backtick-quoted list the prompt
/// splices into `{EXTERNAL_TEST_DIRS}`.
fn suite_dir_list(kind: TestSuiteKind) -> String {
    kind.dirs()
        .iter()
        .map(|d| format!("`{d}/`"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Builds the harness-specific "how to build and run the external tests" block
/// injected into the prompt. Mirrors exactly what the benchmark grader runs so
/// the agent iterates against the same commands.
fn test_instructions(kind: TestSuiteKind, toolchain: &str) -> String {
    match kind {
        TestSuiteKind::Gtest => format!(
            "The external tests are a **GoogleTest suite** in `gtest_suite/`. The\n\
             grader builds your crate as a `cdylib`, then builds the suite against\n\
             that `.so` and runs **each test in its own process** (a crash or\n\
             timeout in one test never hides the others). Reproduce it exactly:\n\n\
             ```bash\n\
             # 1. Build your crate as a cdylib (Cargo.toml must set crate-type = [\"cdylib\"]).\n\
             RUSTUP_TOOLCHAIN={tc} cargo build --release\n\
             #    The .so lands in target/release/lib<crate>.so\n\n\
             # 2. Configure + build the suite against YOUR freshly built library.\n\
             cmake -S gtest_suite -B target/gtest_build -DCMAKE_BUILD_TYPE=Release \\\n\
                   -DTEST_LIB_PATH=$(pwd)/target/release/lib<crate>.so\n\
             cmake --build target/gtest_build -j\n\n\
             # 3. Enumerate tests (parameterized cases expand here).\n\
             LD_LIBRARY_PATH=$(pwd)/target/release \\\n\
                 ./target/gtest_build/harvest_gtest --gtest_list_tests\n\n\
             # 4. Run one test, in its own process, exactly like the grader.\n\
             LD_LIBRARY_PATH=$(pwd)/target/release \\\n\
                 ./target/gtest_build/harvest_gtest --gtest_filter='SuiteName.TestName'\n\
             ```\n\n\
             Read the failing test's source under `gtest_suite/` to learn the\n\
             exact behavior it asserts, then trace back into `c_src/` for the\n\
             reference semantics. Some tests are heavy (tens of seconds) — that is\n\
             not a hang. Every public symbol the C library exports must also be\n\
             exported by your crate, or the suite fails to link.",
            tc = toolchain
        ),
        TestSuiteKind::Lib => format!(
            "The external tests use the **cando2 library runner** (`runner/` +\n\
             `test_vectors/`). The grader builds your crate as a `cdylib` and, for\n\
             each vector, runs the compiled runner with `RUST_ARTIFACTS=1` so it\n\
             loads YOUR `.so`. Reproduce it:\n\n\
             ```bash\n\
             RUSTUP_TOOLCHAIN={tc} cargo build --release          # cdylib -> target/release/lib<crate>.so\n\
             RUSTUP_TOOLCHAIN={tc} cargo build --release --manifest-path runner/Cargo.toml\n\
             # Per vector (bare filename; cando2 prepends test_vectors/):\n\
             RUST_ARTIFACTS=1 LD_LIBRARY_PATH=$(pwd)/target/release \\\n\
                 ./runner/target/release/<runner-bin> -t $(pwd) -v <vector>.json --rust lib\n\
             ```\n\n\
             A vector passes when the runner exits 0. Read `runner/src/main.rs`\n\
             and the vector JSON to see what state is compared, and `c_src/` for\n\
             the reference semantics.",
            tc = toolchain
        ),
        TestSuiteKind::Bin => format!(
            "The external tests are **executable/stdout** vectors in\n\
             `test_vectors/`. The grader builds the `driver` binary and, for each\n\
             vector, feeds `argv`/`stdin` and compares stdout to the expected\n\
             pattern. Reproduce it:\n\n\
             ```bash\n\
             RUSTUP_TOOLCHAIN={tc} cargo build --release          # -> target/release/driver\n\
             ./target/release/driver <argv...>  < <stdin>          # compare stdout to the vector\n\
             ```\n\n\
             Each `test_vectors/*.json` gives `argv`, `stdin`, and the expected\n\
             `stdout`. Read `c_src/` for the reference behavior.",
            tc = toolchain
        ),
    }
}
