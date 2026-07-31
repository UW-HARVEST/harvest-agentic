//! Agentic conformance ("conform") tool — the third refinement stage.
//!
//! Unlike translate/verify (which run inside the IR pipeline on
//! `CargoPackage`/`RawSource` representations), conform operates on an
//! already-materialized, post-verify output folder on disk. It is a plain
//! function driven directly by the benchmark: fully decoupled from the first
//! two rounds, folder-in / folder-out.
//!
//! The distinguishing feature versus verify: the external test suite (a
//! GoogleTest suite or the tractor/cando2 runner+vectors) is **revealed** to
//! the agent, whose sole objective is to make every external test pass. The
//! research question this serves: if the external tests are provided, can a
//! third agent close the gap between "passes its own internally generated
//! tests" and "passes the external tests"?
//!
//! Everything the agent does happens inside a tempdir; the caller's input
//! folder is never modified. The caller re-copies a pristine test suite for
//! the independent final grading, so editing the tests cannot help the agent.

use agent_runner::{AgentInvocation, AgentPhase};
use harvest_core::cargo_utils::copy_directory_recursive;
use harvest_core::config::AgentKind;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use tracing::{info, warn};

const PROMPT_CONFORM: &str = include_str!("prompt_conform.md");

/// Which external test harness the agent must satisfy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConformHarness {
    /// GoogleTest suite (`gtest_suite/`, built via CMake against the cdylib).
    Gtest,
    /// cando2 library validation (`runner/` + `test_vectors/`).
    Lib,
    /// Executable validation (`driver` binary against `test_vectors/`).
    Bin,
}

/// Inputs for a single conform run over one translated program directory.
pub struct ConformParams<'a> {
    /// Post-verify translated program directory (read-only; never modified).
    pub input_project_dir: &'a Path,
    /// Where the refined Rust project + logs + report are written.
    pub output_project_dir: &'a Path,
    /// External test harness the agent must satisfy.
    pub harness: ConformHarness,
    /// Directory names that constitute the external test suite. They are copied
    /// into the agent's tempdir so it can build/run them, but are NOT copied
    /// back to the output (the caller re-adds a pristine copy for grading).
    pub test_dirs: &'a [String],
    pub agent: AgentKind,
    pub model: Option<&'a str>,
    pub timeout_secs: u64,
    pub env: &'a HashMap<String, String>,
    /// Benchmark output.log path so the agent's trace is appended to it.
    pub output_log_path: Option<&'a Path>,
}

/// Entries copied from the input project into the agent's working copy and,
/// where applicable, back out to the refined output. `c_src` is copied
/// specially (its stale `build/` is dropped). The external test dirs are added
/// separately from `params.test_dirs`.
const PROJECT_FILES: &[&str] = &[
    "Cargo.toml",
    "Cargo.lock",
    "PLAN.md",
    "plan_translate.md",
    "HYPOTHESES.md",
    "hypotheses_verify.md",
    "tool_wishlist.json",
];
const PROJECT_DIRS: &[&str] = &["src", "tests"];

/// Refined artifacts copied back from the tempdir into the output project dir.
/// The external test dirs and build artifacts are deliberately excluded.
const COPY_BACK_FILES: &[&str] = &[
    "Cargo.toml",
    "Cargo.lock",
    "PLAN.md",
    "plan_translate.md",
    "HYPOTHESES.md",
    "hypotheses_verify.md",
    "tool_wishlist.json",
    "CONFORM.md",
    "CONFORM_REPORT.md",
];
const COPY_BACK_DIRS: &[&str] = &["src", "tests"];

/// Runs one conform refinement over `params.input_project_dir`, writing the
/// refined Rust project (plus `CONFORM.md` and `CONFORM_REPORT.md`) into
/// `params.output_project_dir`. Does not run the final grading — the caller
/// re-copies pristine tests and validates independently.
pub fn run(params: ConformParams) -> Result<(), Box<dyn std::error::Error>> {
    // case_dir/
    //   translated_rust/        <- the Rust project to refine (work_dir)
    //     c_src/                <- C reference (semantic ground truth)
    //     <test_dirs>/          <- external tests, revealed to the agent
    let work_dir = tempfile::tempdir()?;
    let case_dir = work_dir.path();
    let translated = case_dir.join("translated_rust");
    fs::create_dir_all(&translated)?;

    stage_input(params.input_project_dir, &translated, params.test_dirs)?;
    info!("Conform working directory: {}", case_dir.display());

    let rust_toolchain_context =
        agent_runner::detect_rust_toolchain_context(params.input_project_dir)?;

    let model_limits = match (params.agent, params.model) {
        (AgentKind::OpenCode, Some(model)) => {
            let limits = agent_runner::load_opencode_model_limits(model)?;
            agent_runner::render_model_limits_block(&limits)
        }
        _ => String::new(),
    };

    let prompt = PROMPT_CONFORM
        .replace(
            "{AGENT_BUG_WORKAROUNDS}",
            agent_runner::agent_bug_workarounds(params.agent),
        )
        .replace(
            "{CONFORM_TEST_INSTRUCTIONS}",
            &test_instructions(params.harness, &rust_toolchain_context.required_version),
        )
        .replace("{EXTERNAL_TEST_DIRS}", &params.test_dirs.join(", "))
        .replace(
            "{RUST_TOOLCHAIN_CONTEXT}",
            &rust_toolchain_context.prompt_block,
        )
        .replace(
            "{WORKDIR_BOUNDARY}",
            &agent_runner::render_workdir_boundary(params.agent, &translated),
        )
        .replace("{MODEL_LIMITS}", &model_limits);

    agent_runner::invoke_agent(AgentInvocation {
        phase: AgentPhase::Conform,
        agent: params.agent,
        work_dir: &translated,
        prompt: &prompt,
        timeout_secs: params.timeout_secs,
        model: params.model,
        no_plan: false,
        no_plan_file: false,
        extra_env: params.env,
        output_log_path: params.output_log_path,
        rust_toolchain: Some(&rust_toolchain_context.required_version),
    })?;
    info!("Conformance refinement complete");

    copy_back(&translated, params.output_project_dir)?;
    Ok(())
}

/// Copies the parts of the input project the agent needs into `translated`,
/// skipping build artifacts and stale benchmark output. The external test
/// dirs are copied so the agent can build and run them.
fn stage_input(
    input: &Path,
    translated: &Path,
    test_dirs: &[String],
) -> Result<(), Box<dyn std::error::Error>> {
    for name in PROJECT_FILES {
        let src = input.join(name);
        if src.is_file() {
            fs::copy(&src, translated.join(name))?;
        }
    }
    for name in PROJECT_DIRS {
        let src = input.join(name);
        if src.is_dir() {
            copy_directory_recursive(&src, &translated.join(name))?;
        }
    }
    // c_src is the C reference; drop its stale build/ so the agent rebuilds.
    let c_src = input.join("c_src");
    if c_src.is_dir() {
        let dst = translated.join("c_src");
        copy_directory_recursive(&c_src, &dst)?;
        let build = dst.join("build");
        if build.exists() {
            let _ = fs::remove_dir_all(&build);
        }
    }
    // The external test suite, revealed to the agent.
    for name in test_dirs {
        let src = input.join(name);
        if src.is_dir() {
            copy_directory_recursive(&src, &translated.join(name))?;
        } else {
            warn!("expected external test dir {} not found in input", name);
        }
    }
    Ok(())
}

/// Copies the refined Rust project and the conform artifacts back out. Excludes
/// the external test dirs (graded from a pristine copy), c_src, and target/.
fn copy_back(translated: &Path, output: &Path) -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(output)?;
    for name in COPY_BACK_FILES {
        let src = translated.join(name);
        if src.is_file() {
            fs::copy(&src, output.join(name))?;
        }
    }
    for name in COPY_BACK_DIRS {
        let src = translated.join(name);
        if src.is_dir() {
            let dst = output.join(name);
            if dst.exists() {
                fs::remove_dir_all(&dst)?;
            }
            copy_directory_recursive(&src, &dst)?;
        }
    }
    if !translated.join("CONFORM_REPORT.md").is_file() {
        warn!(
            "conform agent did not produce CONFORM_REPORT.md (internal-vs-external gap analysis missing)"
        );
    }
    Ok(())
}

/// Builds the harness-specific "how to build and run the external tests" block
/// injected into the prompt. Mirrors exactly what the benchmark grader runs so
/// the agent iterates against the same commands.
fn test_instructions(harness: ConformHarness, toolchain: &str) -> String {
    match harness {
        ConformHarness::Gtest => format!(
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
        ConformHarness::Lib => format!(
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
        ConformHarness::Bin => format!(
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
