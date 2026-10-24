//! Agentic verify-and-fix tool.
//!
//! After an initial translation, this tool materializes the [`CargoPackage`](full_source::CargoPackage)
//! into a fresh working directory alongside the original C source, then invokes an external agent.
//! The agent compiles and runs both the C and Rust implementations against generated test inputs,
//! compares their outputs, and iteratively fixes the Rust code until the two agree (or the agent
//! gives up). This is dynamic, execution-based verification, not a static or formal analysis.

use agent_runner::{AgentInvocation, AgentPhase, CaseDir};
use full_source::{CargoPackage, RawSource};
use harvest_core::cmake_presets::{TestConfig, find_test_config};
use harvest_core::config::{AgentKind, ToolConfig, unknown_field_warning};
use harvest_core::tools::{RunContext, Tool};
use harvest_core::{Id, Representation};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use tracing::{info, warn};

const PROMPT_KIRO_VERIFY: &str = include_str!("prompt_kiro_verify.md");
const PROMPT_VERIFY: &str = include_str!("prompt_verify.md");
const PROMPT_VERIFY_NO_PLAN: &str = include_str!("prompt_verify_no_plan.md");

/// Verification-method sections spliced into the standard prompt's
/// `{VERIFICATION_METHOD}` slot (see `load_verify_prompt`).
const METHOD_LIBLOADING: &str = include_str!("method_libloading.md");
const METHOD_GTEST: &str = include_str!("method_gtest.md");
/// FuzzTest instructions inserted into the gtest method only when
/// fuzzing is enabled.
const FUZZTEST_SECTION: &str = include_str!("fuzztest_section.md");

/// Directory (inside the workspace) holding the gtest/fuzztest verification
/// environment. Materialized for the gtest harness and stripped before freeze.
const VERIFY_ENV_DIR: &str = "verify_env";

/// FuzzTest release tag whose `cmake/BuildDependencies.cmake` was built against
/// GoogleTest v1.17.0 (matching the pin in the template CMakeLists).
const FUZZTEST_GIT_TAG: &str = "2026-06-29";

// verify_env scaffold templates (materialized into workspace/verify_env/).
const VE_CMAKELISTS: &str = include_str!("verify_env_template/CMakeLists.txt");
const VE_TESTS_CC: &str = include_str!("verify_env_template/verification_tests.cc");
const VE_DIFF_H: &str = include_str!("verify_env_template/harvest_diff.h");
const VE_RUST_LIB_H: &str = include_str!("verify_env_template/rust_lib.h");
const VE_BUILD_SH: &str = include_str!("verify_env_template/build.sh");
const VE_BUILD_FUZZ_SH: &str = include_str!("verify_env_template/build_fuzz.sh");
const VE_RUN_FUZZ_SH: &str = include_str!("verify_env_template/run_fuzz.sh");
const VE_RUN_TESTS_SH: &str = include_str!("verify_env_template/run_tests.sh");
const VE_MEMORY_SCOPE_SH: &str = include_str!("verify_env_template/memory_scope.sh");
const VE_README: &str = include_str!("verify_env_template/README.md");
const VE_README_FUZZ: &str = include_str!("verify_env_template/README_fuzz.md");

// Vendored FuzzTest reference docs (Apache-2.0), materialized under
// verify_env/docs/ only in fuzz mode for the agent to read on demand.
const VE_DOCS: &[(&str, &str)] = &[
    (
        "NOTICE.md",
        include_str!("verify_env_template/docs/NOTICE.md"),
    ),
    (
        "domains-reference.md",
        include_str!("verify_env_template/docs/domains-reference.md"),
    ),
    (
        "fuzz-test-macro.md",
        include_str!("verify_env_template/docs/fuzz-test-macro.md"),
    ),
    (
        "flags-reference.md",
        include_str!("verify_env_template/docs/flags-reference.md"),
    ),
    (
        "use-cases.md",
        include_str!("verify_env_template/docs/use-cases.md"),
    ),
];

pub struct VerifyFixAgentic;

impl Tool for VerifyFixAgentic {
    fn name(&self) -> &'static str {
        "verify_fix_agentic"
    }

    fn run(
        self: Box<Self>,
        context: RunContext,
        inputs: Vec<Id>,
    ) -> Result<Box<dyn Representation>, Box<dyn std::error::Error>> {
        let config: Config = context.config.tool()?;
        config.validate();

        let agent = context.config.agentic_agent;
        let cargo_package = context
            .ir_snapshot
            .get::<CargoPackage>(inputs[0])
            .ok_or("No CargoPackage representation found in IR")?;
        let raw_source = context
            .ir_snapshot
            .get::<RawSource>(inputs[1])
            .ok_or("No RawSource representation found in IR")?;

        // The agent works in case_dir/workspace/, the materialized CargoPackage. The C source is
        // this stage's oracle, a read-only input in case_dir/inputs/ (see agent_runner::CaseDir).
        let case = CaseDir::new()?;
        let workspace = case.workspace();
        cargo_package.dir.materialize(&workspace)?;
        raw_source.dir.materialize(case.c_src())?;
        info!("Working directory: {}", workspace.display());

        let rust_toolchain_context = agent_runner::detect_rust_toolchain_context(
            &context.config.input,
            config.test_corpus_root.as_deref(),
        )?;

        // Look near the original input dir; the case_dir tempdir does not contain
        // CMakePresets.json because raw_source only mirrors `test_case/` content.
        let test_config = find_test_config(&context.config.input);
        let workflow_hint = if config.workflow && agent == AgentKind::Claude {
            "You must use a workflow.\n\n"
        } else {
            ""
        };
        let prompt = load_verify_prompt(&config, agent)
            .replace("{CMAKE_BUILD_FLAGS}", &test_config.cmake_flags)
            .replace("{ALL_CONFIGURATIONS}", &render_configurations(&test_config))
            .replace("{WORKFLOW_HINT}", workflow_hint);

        // Materialize the gtest/fuzztest verification environment when that
        // harness is in effect. It matches the prompt the agent will see: the
        // no-plan ablation prompt is libloading-only, so skip it there.
        let gtest_harness_active = config.verify_harness == VerifyHarness::Gtest
            && !config.no_plan
            && matches!(agent, AgentKind::Claude | AgentKind::OpenCode);
        if gtest_harness_active {
            materialize_verify_env(&case, config.fuzz)?;
            info!(
                "Materialized gtest verification environment (fuzz={})",
                config.fuzz
            );
        }

        let reference_guard = case.lock_inputs()?;
        agent_runner::invoke_agent(AgentInvocation {
            phase: AgentPhase::Verify,
            agent,
            case: &case,
            prompt: &prompt,
            timeout_secs: config.timeout_secs,
            model: config.model.as_deref(),
            subagent_model: config.subagent_model.as_deref(),
            no_plan: config.no_plan,
            teamclaude: context.config.teamclaude,
            extra_env: &config.env,
            output_log_path: config.output_log_path.as_deref(),
            rust_toolchain: &rust_toolchain_context,
        })?;
        info!("Verification complete");
        if !workspace.join("HYPOTHESES.md").exists() {
            info!(
                "Agent did not produce a HYPOTHESES.md (no bugs investigated, or skipped step 1)"
            );
        }

        // Keep the gtest/fuzztest verification environment in the frozen output (like the
        // libloading tests/ dir) so it is available for inspection, but strip its build directories
        // (build-*), which hold the fetched GoogleTest/FuzzTest/Abseil sources plus symlinks that
        // would otherwise bloat the output and break the symlink-free freeze.
        let verify_env_out = workspace.join(VERIFY_ENV_DIR);
        if verify_env_out.is_dir() {
            match strip_build_dirs(&verify_env_out) {
                Ok(n) if n > 0 => info!("Stripped {n} build dir(s) from verify_env/"),
                Ok(_) => {}
                Err(e) => warn!(
                    "Failed to strip build dirs from {}: {}",
                    verify_env_out.display(),
                    e
                ),
            }
        }
        let dir = case.freeze(&reference_guard, config.reference_audit_dir.as_deref())?;
        Ok(Box::new(CargoPackage { dir }))
    }
}

/// Builds the verification method, including FuzzTest guidance when enabled.
fn build_method_section(config: &Config) -> String {
    match config.verify_harness {
        VerifyHarness::Gtest => {
            let section = if config.fuzz { FUZZTEST_SECTION } else { "" };
            METHOD_GTEST.replace("{FUZZTEST_SECTION}", section)
        }
        VerifyHarness::Libloading => METHOD_LIBLOADING.to_owned(),
    }
}

/// Best-effort extraction of C preprocessor definitions from a project's
/// `c_src/CMakeLists.txt` `target_compile_definitions(...)` calls, so the
/// verify_env C build matches the flags the library is normally compiled with
/// (e.g. `XXH_NAMESPACE=LZ4_`). Returns a space-separated list for CMake, or an
/// empty string if none are found. The agent can correct it if the parse misses
/// something.
fn parse_c_compile_defs(cmakelists: &str) -> String {
    const MARKER: &str = "target_compile_definitions";
    const KEYWORDS: [&str; 3] = ["PRIVATE", "PUBLIC", "INTERFACE"];
    let mut defs: Vec<String> = Vec::new();
    let mut rest = cmakelists;
    while let Some(pos) = rest.find(MARKER) {
        rest = &rest[pos + MARKER.len()..];
        // Take the tokens inside this call's parentheses.
        let Some(open) = rest.find('(') else { break };
        let Some(close) = rest[open..].find(')') else {
            break;
        };
        let inside = &rest[open + 1..open + close];
        rest = &rest[open + close..];
        // First token is the target name; skip it and the visibility keywords.
        for tok in inside.split_whitespace().skip(1) {
            if KEYWORDS.contains(&tok) {
                continue;
            }
            let looks_like_def = tok
                .chars()
                .next()
                .map(|c| c.is_ascii_alphabetic() || c == '_')
                .unwrap_or(false)
                && tok
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '=');
            if looks_like_def && !defs.iter().any(|d| d == tok) {
                defs.push(tok.to_string());
            }
        }
    }
    defs.join(" ")
}

/// Materializes the gtest/fuzztest verification environment into
/// `workspace/verify_env/`, filling the CMake template for the requested mode
/// and pre-seeding the C compile definitions parsed from the C `CMakeLists.txt`.
/// The CMake variable `HARVEST_C_SRC` names the C source, by default `inputs/c_src` of the
/// checkout. The scripts build in `verify_env/build-*`, which the freeze strips.
fn materialize_verify_env(case: &CaseDir, fuzz: bool) -> Result<(), Box<dyn std::error::Error>> {
    let env_dir = case.workspace().join(VERIFY_ENV_DIR);
    fs::create_dir_all(&env_dir)?;
    let c_src_dir = case.c_src();

    let c_defs = fs::read_to_string(c_src_dir.join("CMakeLists.txt"))
        .map(|s| parse_c_compile_defs(&s))
        .unwrap_or_default();

    let (declare, makeavail, setup_flags, test_link) = if fuzz {
        (
            format!(
                "FetchContent_Declare(\n  fuzztest\n  GIT_REPOSITORY https://github.com/google/fuzztest.git\n  GIT_TAG        {FUZZTEST_GIT_TAG}\n)\n"
            ),
            " fuzztest".to_string(),
            "\n# Enable fuzzing instrumentation AFTER the frameworks are configured, so\n# gtest/fuzztest/abseil are not themselves instrumented — only what follows.\nfuzztest_setup_fuzzing_flags()\n# The call above also defines FUZZING_BUILD_MODE_UNSAFE_FOR_PRODUCTION, which\n# libraries use to skip checks (checksums, magic numbers) in fuzz builds. The\n# Rust must match the production C, so undo it. Compile options come after\n# CMAKE_C_FLAGS on the compile line, so this -U wins.\nadd_compile_options(-UFUZZING_BUILD_MODE_UNSAFE_FOR_PRODUCTION)\n".to_string(),
            "link_fuzztest(verification_tests)".to_string(),
        )
    } else {
        (
            String::new(),
            String::new(),
            String::new(),
            "target_link_libraries(verification_tests PRIVATE GTest::gtest_main)".to_string(),
        )
    };

    let cmakelists = VE_CMAKELISTS
        .replace("{FUZZTEST_DECLARE}", &declare)
        .replace("{FUZZTEST_MAKEAVAIL}", &makeavail)
        .replace("{FUZZTEST_SETUP_FLAGS}", &setup_flags)
        .replace("{TEST_LINK}", &test_link)
        .replace("{C_COMPILE_DEFS}", &c_defs);

    let readme = VE_README.replace("{FUZZTEST_README}", if fuzz { VE_README_FUZZ } else { "" });
    // (file name, content, executable)
    let mut files: Vec<(&str, &str, bool)> = vec![
        ("CMakeLists.txt", &cmakelists, false),
        ("verification_tests.cc", VE_TESTS_CC, false),
        ("harvest_diff.h", VE_DIFF_H, false),
        ("rust_lib.h", VE_RUST_LIB_H, false),
        ("README.md", &readme, false),
        ("build.sh", VE_BUILD_SH, true),
        ("run_tests.sh", VE_RUN_TESTS_SH, true),
        ("memory_scope.sh", VE_MEMORY_SCOPE_SH, false),
    ];
    if fuzz {
        files.push(("build_fuzz.sh", VE_BUILD_FUZZ_SH, true));
        files.push(("run_fuzz.sh", VE_RUN_FUZZ_SH, true));
    }
    for (name, text, executable) in files {
        if executable {
            write_script(&env_dir.join(name), text)?;
        } else {
            fs::write(env_dir.join(name), text)?;
        }
    }
    if fuzz {
        // Vendored FuzzTest reference docs for on-demand reading (fuzz only).
        let docs_dir = env_dir.join("docs");
        fs::create_dir_all(&docs_dir)?;
        for (name, contents) in VE_DOCS {
            fs::write(docs_dir.join(name), contents)?;
        }
    }
    Ok(())
}

/// Removes build output directories (`build-*`, e.g. build-test/build-fuzz) from
/// a verify_env directory so only the agent's source survives the freeze. These
/// hold fetched GoogleTest/FuzzTest/Abseil sources and symlinks. Returns the
/// number of build directories removed.
fn strip_build_dirs(env_dir: &Path) -> Result<usize, Box<dyn std::error::Error>> {
    let mut removed = 0;
    for entry in fs::read_dir(env_dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        if entry.file_name().to_string_lossy().starts_with("build-") {
            fs::remove_dir_all(entry.path())?;
            removed += 1;
        }
    }
    Ok(removed)
}

/// Writes a shell script and marks it executable on Unix.
fn write_script(path: &Path, contents: &str) -> Result<(), Box<dyn std::error::Error>> {
    fs::write(path, contents)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(path)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms)?;
    }
    Ok(())
}

fn load_verify_prompt(config: &Config, agent: AgentKind) -> String {
    match agent {
        // The no-plan ablation prompt is libloading-only by construction (it
        // predates the harness split); the harness/fuzz switch applies to the
        // standard prompt.
        AgentKind::Claude | AgentKind::OpenCode if config.no_plan => {
            PROMPT_VERIFY_NO_PLAN.to_owned()
        }
        AgentKind::Claude | AgentKind::OpenCode => {
            PROMPT_VERIFY.replace("{VERIFICATION_METHOD}", &build_method_section(config))
        }
        AgentKind::Kiro => PROMPT_KIRO_VERIFY.to_owned(),
    }
}

/// Render `TestConfig` as the markdown block that the verify prompt expects in
/// place of `{ALL_CONFIGURATIONS}`. Empty when there are no project knobs to
/// switch — in that case the conditional "If configurations are listed below"
/// section silently no-ops.
fn render_configurations(cfg: &TestConfig) -> String {
    if cfg.is_empty() {
        return String::new();
    }
    format!("Configurations to verify:\n{}", cfg.as_markdown_bullet())
}

/// Comparison mechanism the verification prompt describes.
#[derive(Debug, Deserialize, Serialize, Default, PartialEq, Eq, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub enum VerifyHarness {
    /// C++ GoogleTest environment with the C reference linked in and the Rust
    /// translation loaded via `dlopen`.
    #[default]
    Gtest,
    /// dlopen the C `.so` from a Rust integration test (the original method).
    Libloading,
}

/// Tool-specific configuration, read from `[tools.verify_fix_agentic]` in the HARVEST config.
#[derive(Debug, Deserialize, Serialize)]
pub struct Config {
    /// Agent timeout in seconds.
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,

    /// Agent model to use. If absent, no --model flag is passed and the CLI uses its default.
    /// Claude accepts short aliases ("sonnet", "opus", "haiku") or full model IDs.
    /// OpenCode expects provider/model format.
    pub model: Option<String>,

    /// Default model of the sub-agents that the main agent dispatches, in the
    /// same format as `model`. If absent, the sub-agents use the main model.
    /// Not supported with Kiro.
    pub subagent_model: Option<String>,

    /// If true, use the pre-anti-compaction prompt (no HYPOTHESES.md / Invariants
    /// / sub-agent push) and skip the `--append-system-prompt` flag. Intended
    /// for controlled experiments measuring the impact of the anti-compaction
    /// mechanism added in 883e2e2.
    #[serde(default)]
    pub no_plan: bool,

    /// Inject a prompt hint encouraging the agent to use dynamic workflows.
    /// Only meaningful with no_plan.
    #[serde(default)]
    pub workflow: bool,

    /// Which comparison mechanism the verification prompt describes.
    #[serde(default)]
    pub verify_harness: VerifyHarness,

    /// When `verify_harness = gtest`, also describe FuzzTest as an
    /// available capability and ship its scaffolding. On by default; set
    /// `fuzz = false` to verify with plain GoogleTest only.
    /// No effect under the libloading harness.
    #[serde(default = "default_fuzz")]
    pub fuzz: bool,

    /// Extra environment variables to inject into the agent process.
    /// Useful for CCR provider API keys, proxy settings, etc.
    /// Defined as a TOML table under `[tools.verify_fix_agentic.env]`.
    #[serde(default, skip_serializing)]
    pub env: HashMap<String, String>,

    /// Destination path for the benchmark's output.log file.
    /// Injected by the benchmark at runtime so the agent's full trace
    /// (JSON stream) is appended to the same log file as benchmark messages.
    pub output_log_path: Option<PathBuf>,

    /// Directory that records the read-only inputs the agent changed, one file each
    /// (`.harvest/reference_modified/<stage>/`). Injected by the benchmark.
    pub reference_audit_dir: Option<PathBuf>,

    /// The Test-Corpus / cando2 checkout defining the Rust toolchain contract.
    /// Injected from the stage manifest when resuming from a snapshot, whose
    /// C source no longer sits inside the corpus.
    pub test_corpus_root: Option<PathBuf>,

    #[serde(flatten)]
    unknown: HashMap<String, serde_json::Value>,
}

fn default_timeout_secs() -> u64 {
    agent_runner::DEFAULT_AGENT_TIMEOUT_SECS
}

fn default_fuzz() -> bool {
    true
}

impl ToolConfig for Config {
    const KEY: &'static str = "verify_fix_agentic";
}

impl Config {
    fn validate(&self) {
        unknown_field_warning(&format!("tools.{}", Self::KEY), &self.unknown);
        if self.verify_harness == VerifyHarness::Gtest && self.no_plan {
            warn!(
                "tools.verify_fix_agentic: verify_harness = gtest is not applied under no_plan (that prompt is libloading-only); using libloading"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Exercise the Linux runner with small shell stand-ins, without FuzzTest,
    // Python, a systemd user manager, or large allocations. Miri cannot spawn.
    #[cfg(all(target_os = "linux", not(miri)))]
    mod fuzz_runner {
        use super::*;
        use std::process::Command;

        fn fixture() -> tempfile::TempDir {
            let tmp = tempfile::Builder::new()
                .prefix("fuzz runner ")
                .tempdir()
                .unwrap();
            let root = tmp.path();
            fs::create_dir(root.join("build-fuzz")).unwrap();
            fs::create_dir(root.join("bin")).unwrap();
            fs::write(root.join("fake rust.so"), "").unwrap();
            write_script(&root.join("run_fuzz.sh"), VE_RUN_FUZZ_SH).unwrap();
            fs::write(root.join("memory_scope.sh"), VE_MEMORY_SCOPE_SH).unwrap();
            write_script(
                &root.join("build-fuzz/verification_tests"),
                r#"#!/bin/sh
printf '%s\n' "$@" > "$OBSERVED"
exit "$FAKE_EXIT"
"#,
            )
            .unwrap();
            write_script(
                &root.join("bin/systemd-run"),
                r#"#!/bin/sh
printf '%s\n' "$@" > "$SCOPE_ARGS"
exit 86
"#,
            )
            .unwrap();
            tmp
        }

        fn command(root: &Path) -> Command {
            let mut cmd = Command::new("timeout");
            cmd.args(["--kill-after=1s", "10s", "bash"])
                .arg(root.join("run_fuzz.sh"))
                .arg("Suite.Property")
                .current_dir(root);
            for (key, _) in std::env::vars_os() {
                if key.to_string_lossy().starts_with("FUZZ_") {
                    cmd.env_remove(key);
                }
            }
            let mut paths = vec![root.join("bin")];
            paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
            cmd.env("PATH", std::env::join_paths(paths).unwrap())
                .env("RUST_LIB_PATH", root.join("fake rust.so"))
                .env("OBSERVED", root.join("observed"))
                .env("SCOPE_ARGS", root.join("scope_args"))
                .env("FAKE_EXIT", "0")
                .env("FUZZ_HARD_LIMIT_MB", "0");
            cmd
        }

        #[test]
        fn budgets_use_runner_defaults_and_environment_overrides() {
            let tmp = fixture();
            let root = tmp.path();
            let output = command(root).output().unwrap();
            assert!(output.status.success(), "{output:?}");
            assert_eq!(
                fs::read_to_string(root.join("observed")).unwrap(),
                "--fuzz=Suite.Property\n--fuzz_for=300s\n--rss_limit_mb=2048\n--time_limit_per_input=10s\n"
            );

            fs::write(
                root.join("run_fuzz.sh"),
                VE_RUN_FUZZ_SH.replace(":-2048", ":-1024"),
            )
            .unwrap();
            assert!(command(root).output().unwrap().status.success());
            assert!(
                fs::read_to_string(root.join("observed"))
                    .unwrap()
                    .contains("--rss_limit_mb=1024\n")
            );

            let output = command(root)
                .env("FUZZ_RSS_LIMIT_MB", "512")
                .env("FUZZ_DURATION_SECONDS", "30")
                .env("FUZZ_INPUT_TIMEOUT_SECONDS", "2")
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
            assert_eq!(
                fs::read_to_string(root.join("observed")).unwrap(),
                "--fuzz=Suite.Property\n--fuzz_for=30s\n--rss_limit_mb=512\n--time_limit_per_input=2s\n"
            );
        }

        #[test]
        fn failure_exit_code_survives_logging() {
            let tmp = fixture();
            let output = command(tmp.path()).env("FAKE_EXIT", "42").output().unwrap();
            assert_eq!(output.status.code(), Some(42), "{output:?}");
            let artifacts = fs::read_dir(tmp.path().join("fuzz-artifacts"))
                .unwrap()
                .next()
                .unwrap()
                .unwrap()
                .path();
            assert_eq!(
                fs::read_to_string(artifacts.join("exit_status")).unwrap(),
                "42\n"
            );
            assert!(String::from_utf8_lossy(&output.stderr).contains("incomplete"));
        }

        #[test]
        fn invalid_configuration_never_starts_binary() {
            let tmp = fixture();
            for (name, value) in [
                ("FUZZ_RSS_LIMIT_MB", "0"),
                ("FUZZ_RSS_LIMIT_MB", "-1"),
                ("FUZZ_RSS_LIMIT_MB", "99999999999999999"),
                ("FUZZ_DURATION_SECONDS", "inf"),
                ("FUZZ_INPUT_TIMEOUT_SECONDS", "0"),
                ("FUZZ_HARD_LIMIT_MB", "2048"),
                ("RUST_LIB_PATH", "relative.so"),
            ] {
                let output = command(tmp.path()).env(name, value).output().unwrap();
                assert_eq!(output.status.code(), Some(2), "{name}={value}: {output:?}");
                assert!(!tmp.path().join("observed").exists());
            }
            let output = command(tmp.path())
                .arg("--rss_limit_mb=0")
                .output()
                .unwrap();
            assert_eq!(output.status.code(), Some(2));
            assert!(!tmp.path().join("observed").exists());
        }

        #[test]
        fn cgroup_failure_never_falls_back() {
            let tmp = fixture();
            let output = command(tmp.path())
                .env_remove("FUZZ_HARD_LIMIT_MB")
                .output()
                .unwrap();
            assert_eq!(output.status.code(), Some(86), "{output:?}");
            assert!(!tmp.path().join("observed").exists());
            let args = fs::read_to_string(tmp.path().join("scope_args")).unwrap();
            for expected in [
                "MemoryMax=3072M",
                "MemorySwapMax=0",
                "--kill-after=5s",
                "330s",
            ] {
                assert!(
                    args.lines().any(|arg| arg == expected),
                    "missing {expected}: {args}"
                );
            }
        }
    }

    // Same approach for the unit-test runner: shell stand-ins for the test
    // binary and systemd-run, no real cgroup and no large allocations.
    #[cfg(all(target_os = "linux", not(miri)))]
    mod test_runner {
        use super::*;
        use std::process::Command;

        fn fixture() -> tempfile::TempDir {
            let tmp = tempfile::Builder::new()
                .prefix("test runner ")
                .tempdir()
                .unwrap();
            let root = tmp.path();
            fs::create_dir(root.join("build-test")).unwrap();
            fs::create_dir(root.join("bin")).unwrap();
            fs::write(root.join("fake rust.so"), "").unwrap();
            write_script(&root.join("run_tests.sh"), VE_RUN_TESTS_SH).unwrap();
            fs::write(root.join("memory_scope.sh"), VE_MEMORY_SCOPE_SH).unwrap();
            // Records its arguments and the address-space limit it runs under.
            write_script(
                &root.join("build-test/verification_tests"),
                r#"#!/bin/sh
printf '%s\n' "$@" > "$OBSERVED"
ulimit -v > "$OBSERVED_LIMIT"
exit "$FAKE_EXIT"
"#,
            )
            .unwrap();
            write_script(
                &root.join("bin/systemd-run"),
                r#"#!/bin/sh
printf '%s\n' "$@" > "$SCOPE_ARGS"
exit 86
"#,
            )
            .unwrap();
            tmp
        }

        fn command(root: &Path) -> Command {
            let mut cmd = Command::new("timeout");
            cmd.args(["--kill-after=1s", "10s", "bash"])
                .arg(root.join("run_tests.sh"))
                .current_dir(root);
            for (key, _) in std::env::vars_os() {
                if key.to_string_lossy().starts_with("TEST_") {
                    cmd.env_remove(key);
                }
            }
            let mut paths = vec![root.join("bin")];
            paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
            cmd.env("PATH", std::env::join_paths(paths).unwrap())
                .env("RUST_LIB_PATH", root.join("fake rust.so"))
                .env("OBSERVED", root.join("observed"))
                .env("OBSERVED_LIMIT", root.join("observed_limit"))
                .env("SCOPE_ARGS", root.join("scope_args"))
                .env("FAKE_EXIT", "0")
                .env("TEST_HARD_LIMIT_MB", "0");
            cmd
        }

        #[test]
        fn passes_arguments_and_applies_address_space_limit() {
            let tmp = fixture();
            let root = tmp.path();
            let output = command(root)
                .args(["--gtest_filter=Suite.*", "--gtest_brief=1"])
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
            assert_eq!(
                fs::read_to_string(root.join("observed")).unwrap(),
                "--gtest_filter=Suite.*\n--gtest_brief=1\n"
            );
            assert_eq!(
                fs::read_to_string(root.join("observed_limit")).unwrap(),
                "2097152\n"
            );

            // An override applies, and the test binary's exit code passes through.
            let output = command(root)
                .env("TEST_AS_LIMIT_MB", "512")
                .env("FAKE_EXIT", "42")
                .output()
                .unwrap();
            assert_eq!(output.status.code(), Some(42), "{output:?}");
            assert_eq!(
                fs::read_to_string(root.join("observed_limit")).unwrap(),
                "524288\n"
            );
        }

        #[test]
        fn invalid_configuration_never_starts_binary() {
            let tmp = fixture();
            for (name, value) in [
                // The cgroup limit must stay above the per-process limit.
                ("TEST_HARD_LIMIT_MB", "2048"),
                ("RUST_LIB_PATH", "relative.so"),
            ] {
                let output = command(tmp.path()).env(name, value).output().unwrap();
                assert_eq!(output.status.code(), Some(2), "{name}={value}: {output:?}");
                assert!(!tmp.path().join("observed").exists());
            }
        }

        #[test]
        fn cgroup_failure_never_falls_back() {
            let tmp = fixture();
            let output = command(tmp.path())
                .env_remove("TEST_HARD_LIMIT_MB")
                .output()
                .unwrap();
            assert_eq!(output.status.code(), Some(86), "{output:?}");
            assert!(!tmp.path().join("observed").exists());
            let args = fs::read_to_string(tmp.path().join("scope_args")).unwrap();
            for expected in [
                "MemoryMax=3072M",
                "MemorySwapMax=0",
                "--kill-after=5s",
                "600s",
            ] {
                assert!(
                    args.lines().any(|arg| arg == expected),
                    "missing {expected}: {args}"
                );
            }
        }
    }

    #[test]
    fn parses_lz4_style_compile_defs() {
        let cmake = "\
add_library(lz4 SHARED src/lz4.c)
target_include_directories(lz4 PUBLIC include PRIVATE src)
target_compile_definitions(lz4 PRIVATE XXH_NAMESPACE=LZ4_)
target_compile_definitions(lz4 PRIVATE LZ4_HEAPMODE=0 LZ4F_HEAPMODE=0)
";
        assert_eq!(
            parse_c_compile_defs(cmake),
            "XXH_NAMESPACE=LZ4_ LZ4_HEAPMODE=0 LZ4F_HEAPMODE=0"
        );
    }

    #[test]
    fn no_compile_defs_yields_empty() {
        let cmake = "add_library(foo SHARED src/foo.c)\n";
        assert_eq!(parse_c_compile_defs(cmake), "");
    }

    // The two tests below call `materialize_verify_env`, which marks build.sh
    // executable. Miri has no shim for `chmod` — an unsupported foreign call, not
    // an isolation error, so `-Zmiri-disable-isolation` does not help. Skipped
    // under Miri only; the normal `cargo test` run still covers them.
    #[test]
    #[cfg_attr(
        miri,
        ignore = "materialize_verify_env chmods build.sh; Miri has no chmod shim"
    )]
    fn gtest_only_cmake_has_no_fuzztest() {
        let case = CaseDir::new().unwrap();
        let workspace = &case.workspace();
        let c_src = case.c_src();
        fs::create_dir_all(&c_src).unwrap();
        fs::write(
            c_src.join("CMakeLists.txt"),
            "target_compile_definitions(x PRIVATE XXH_NAMESPACE=LZ4_)\n",
        )
        .unwrap();

        materialize_verify_env(&case, false).unwrap();
        let cml = fs::read_to_string(workspace.join("verify_env/CMakeLists.txt")).unwrap();
        assert!(cml.contains("XXH_NAMESPACE=LZ4_"));
        // The C source is inputs/c_src of the checkout, a default that -DHARVEST_C_SRC replaces.
        assert!(cml.contains(
            "set(HARVEST_C_SRC \"${CMAKE_CURRENT_SOURCE_DIR}/../inputs/c_src\" CACHE PATH"
        ));
        assert!(cml.contains("\"${HARVEST_C_SRC}/src/*.c\""));
        assert!(cml.contains("GTest::gtest_main"));
        assert!(!cml.to_ascii_lowercase().contains("fuzz"));
        assert!(!workspace.join("verify_env/build_fuzz.sh").exists());
        assert!(!workspace.join("verify_env/run_fuzz.sh").exists());
        assert!(workspace.join("verify_env/run_tests.sh").exists());
        assert!(workspace.join("verify_env/memory_scope.sh").exists());
        // Inspect every delivered file, not only CMake: comments and README
        // text must not reveal the optional mechanism to the control agent.
        for entry in fs::read_dir(workspace.join("verify_env")).unwrap() {
            let path = entry.unwrap().path();
            assert!(path.is_file(), "unexpected scaffold directory: {path:?}");
            assert!(!path.file_name().unwrap().to_string_lossy().contains("fuzz"));
            let text = fs::read_to_string(&path).unwrap().to_ascii_lowercase();
            for hint in ["fuzz", "coverage-guided", "property-based"] {
                assert!(!text.contains(hint), "{path:?} reveals {hint}");
            }
        }
        case.finish();
    }

    // Escape hatch for the manual integration check: when VE_OUT is set,
    // materialize a real verify_env there (fuzz per VE_FUZZ=1) so it can be
    // built and run by hand. Ignored in normal test runs.
    #[test]
    #[ignore]
    fn materialize_to_ve_out() {
        // VE_OUT holds the C source in c_src/. The test copies it into a new case_dir, writes
        // verify_env/ in its workspace and keeps the case_dir: its path goes to stderr.
        let out = std::path::PathBuf::from(std::env::var("VE_OUT").unwrap());
        let fuzz = std::env::var("VE_FUZZ").ok().as_deref() == Some("1");
        let case = CaseDir::new().unwrap();
        harvest_core::cargo_utils::copy_directory_recursive(&out.join("c_src"), &case.c_src())
            .unwrap();
        materialize_verify_env(&case, fuzz).unwrap();
        drop(case); // not finished: kept
    }

    #[test]
    fn strip_build_dirs_keeps_source() {
        let tmp = tempfile::tempdir().unwrap();
        let env = tmp.path().join("verify_env");
        fs::create_dir_all(env.join("build-test/_deps")).unwrap();
        fs::create_dir_all(env.join("build-fuzz")).unwrap();
        fs::create_dir_all(env.join("fuzz-artifacts/example/reproducers")).unwrap();
        fs::write(
            env.join("fuzz-artifacts/example/reproducers/input"),
            "repro",
        )
        .unwrap();
        fs::write(env.join("verification_tests.cc"), "// test").unwrap();
        fs::write(env.join("CMakeLists.txt"), "# cmake").unwrap();
        fs::write(env.join("build-test/_deps/junk.o"), "junk").unwrap();

        let removed = strip_build_dirs(&env).unwrap();
        assert_eq!(removed, 2, "both build-* dirs should be removed");
        assert!(env.join("verification_tests.cc").exists());
        assert!(env.join("CMakeLists.txt").exists());
        assert!(!env.join("build-test").exists());
        assert!(!env.join("build-fuzz").exists());
        assert!(
            env.join("fuzz-artifacts/example/reproducers/input")
                .exists()
        );
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "materialize_verify_env chmods build.sh; Miri has no chmod shim"
    )]
    fn fuzz_cmake_wires_fuzztest() {
        let case = CaseDir::new().unwrap();
        let workspace = &case.workspace();
        let c_src = case.c_src();
        fs::create_dir_all(&c_src).unwrap();
        fs::write(c_src.join("CMakeLists.txt"), "").unwrap();

        materialize_verify_env(&case, true).unwrap();
        let cml = fs::read_to_string(workspace.join("verify_env/CMakeLists.txt")).unwrap();
        assert!(cml.contains("link_fuzztest(verification_tests)"));
        assert!(cml.contains("fuzztest_setup_fuzzing_flags()"));
        assert!(cml.contains("-UFUZZING_BUILD_MODE_UNSAFE_FOR_PRODUCTION"));
        assert!(cml.contains(FUZZTEST_GIT_TAG));
        assert!(!cml.contains("GTest::gtest_main"));
        assert!(workspace.join("verify_env/build_fuzz.sh").exists());
        assert!(workspace.join("verify_env/run_fuzz.sh").exists());
        assert!(workspace.join("verify_env/run_tests.sh").exists());
        assert!(workspace.join("verify_env/memory_scope.sh").exists());
        case.finish();
    }

    #[test]
    fn claude_and_opencode_prompts_carry_bug_workarounds() {
        for prompt in [PROMPT_VERIFY, PROMPT_VERIFY_NO_PLAN] {
            assert!(prompt.contains("{AGENT_BUG_WORKAROUNDS}"));
        }
    }

    #[test]
    fn fuzz_is_enabled_by_default() {
        let config: Config = serde_json::from_value(serde_json::json!({})).unwrap();
        assert!(config.fuzz);
        assert!(build_method_section(&config).contains("FuzzTest"));
    }

    #[test]
    fn disabled_fuzz_is_not_disclosed_in_builtin_prompts() {
        for agent in [AgentKind::Claude, AgentKind::OpenCode, AgentKind::Kiro] {
            for harness in ["gtest", "libloading"] {
                for mode in ["standard", "no_plan"] {
                    let config: Config = serde_json::from_value(serde_json::json!({
                        "fuzz": false, "verify_harness": harness,
                        "no_plan": mode == "no_plan",
                    }))
                    .unwrap();
                    let prompt = load_verify_prompt(&config, agent).to_ascii_lowercase();
                    for hint in ["fuzz", "coverage-guided", "property-based"] {
                        assert!(
                            !prompt.contains(hint),
                            "{agent:?}/{harness}/{mode} reveals {hint}"
                        );
                    }
                }
            }
        }
    }
}
