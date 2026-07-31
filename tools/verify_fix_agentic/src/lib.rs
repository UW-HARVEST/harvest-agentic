//! Agentic verify-and-fix tool.
//!
//! After an initial translation, this tool materializes the [`CargoPackage`](full_source::CargoPackage)
//! into a fresh working directory alongside the original C source, then invokes an external agent.
//! The agent compiles and runs both the C and Rust implementations against generated test inputs,
//! compares their outputs, and iteratively fixes the Rust code until the two agree (or the agent
//! gives up). This is dynamic, execution-based verification, not a static or formal analysis.

use agent_runner::{AgentInvocation, AgentPhase};
use full_source::{CargoPackage, RawSource};
use harvest_core::cmake_presets::{TestConfig, find_test_config};
use harvest_core::config::{AgentKind, unknown_field_warning};
use harvest_core::fs::RawDir;
use harvest_core::tools::{RunContext, Tool};
use harvest_core::{Id, Representation};
use serde::Deserialize;
use std::collections::HashMap;
use std::fs::{self, read_dir};
use std::path::{Path, PathBuf};
use tracing::{info, warn};

const PROMPT_KIRO_VERIFY: &str = include_str!("prompt_kiro_verify.md");
const PROMPT_VERIFY: &str = include_str!("prompt_verify.md");
const PROMPT_VERIFY_NO_PLAN: &str = include_str!("prompt_verify_no_plan.md");
const PROMPT_VERIFY_NO_PLAN_FILE: &str = include_str!("prompt_verify_no_plan_file.md");

/// Verification-method sections spliced into the standard prompt's
/// `{VERIFICATION_METHOD}` slot (see `load_verify_prompt`).
const METHOD_LIBLOADING: &str = include_str!("method_libloading.md");
const METHOD_GTEST: &str = include_str!("method_gtest.md");
/// FuzzTest guidance spliced into the gtest method's `{FUZZTEST_SECTION}` slot
/// when fuzzing is enabled; replaced with empty string otherwise.
const FUZZTEST_SECTION: &str = include_str!("fuzztest_section.md");

/// Directory (inside translated_rust/) holding the gtest/fuzztest verification
/// environment. Materialized for the gtest harness and stripped before freeze.
const VERIFY_ENV_DIR: &str = "verify_env";

/// FuzzTest release tag whose `cmake/BuildDependencies.cmake` was built against
/// GoogleTest v1.17.0 (matching the pin in the template CMakeLists).
const FUZZTEST_GIT_TAG: &str = "2026-06-29";

// verify_env scaffold templates (materialized into translated_rust/verify_env/).
const VE_CMAKELISTS: &str = include_str!("verify_env_template/CMakeLists.txt");
const VE_TESTS_CC: &str = include_str!("verify_env_template/verification_tests.cc");
const VE_DIFF_H: &str = include_str!("verify_env_template/harvest_diff.h");
const VE_RUST_LIB_H: &str = include_str!("verify_env_template/rust_lib.h");
const VE_BUILD_SH: &str = include_str!("verify_env_template/build.sh");
const VE_BUILD_FUZZ_SH: &str = include_str!("verify_env_template/build_fuzz.sh");
const VE_README: &str = include_str!("verify_env_template/README.md");

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
        let default_config = serde_json::Value::Object(Default::default());
        let config = Config::deserialize(
            context
                .config
                .tools
                .get("verify_fix_agentic")
                .unwrap_or(&default_config),
        )?;
        config.validate();

        // Wait until the specified timestamp if --wait-until was given
        if let Some(target_ts) = config.wait_until {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs();
            if now < target_ts {
                let wait_secs = target_ts - now;
                info!("Waiting {wait_secs}s until Unix timestamp {target_ts}");
                std::thread::sleep(std::time::Duration::from_secs(wait_secs));
                info!("Wait complete, starting verification");
            } else {
                info!(
                    "Target timestamp {target_ts} already passed (now={now}), starting immediately"
                );
            }
        }

        let agent = context.config.agentic_agent;
        if config.no_plan && config.no_plan_file {
            return Err(
                "tools.verify_fix_agentic: no_plan and no_plan_file are mutually exclusive".into(),
            );
        }

        let cargo_package = context
            .ir_snapshot
            .get::<CargoPackage>(inputs[0])
            .ok_or("No CargoPackage representation found in IR")?;
        let raw_source = context
            .ir_snapshot
            .get::<RawSource>(inputs[1])
            .ok_or("No RawSource representation found in IR")?;

        // case_dir/
        //   translated_rust/          <- materialized CargoPackage
        //     c_src/                  <- materialized RawSource (for agent reference)
        let work_dir = tempfile::tempdir()?;
        let case_dir = work_dir.path();
        let translated = case_dir.join("translated_rust");
        cargo_package.dir.materialize(&translated)?;

        let c_src_dir = translated.join("c_src");
        fs::create_dir_all(&c_src_dir)?;
        raw_source.dir.materialize(&c_src_dir)?;

        info!("Working directory: {}", case_dir.display());

        // Materialize agent tools if enabled and build the prompt section.
        // When disabled, {AGENT_TOOLS_SECTION} is replaced with an empty string so
        // the entire "Available Tools" block is absent from the prompt the agent sees.
        let agent_tools_section = if context.config.agent_tools {
            let dir = translated.join("agent_tools");
            agent_tools_embed::materialize_to(&dir)?;
            let docs = agent_tools_embed::collect_docs()
                .replace("{AGENT_TOOLS_DIR}", &dir.to_string_lossy());
            format!(
                "---\n\n## Available Tools\n\n\
                 The following tools are pre-installed in `{}/`. Use them when you\n\
                 need a precise answer about C behavior rather than reasoning from first principles.\n\n\
                 {}\n\n",
                dir.display(),
                docs
            )
        } else {
            String::new()
        };

        // The wishlist file lives inside translated_rust/ so the agent can write to it
        // without any special permissions. The absolute path is injected into the prompt.
        let local_wishlist = translated.join("tool_wishlist.json");
        let rust_toolchain_context =
            agent_runner::detect_rust_toolchain_context(&context.config.input)?;

        // Look near the original input dir; the case_dir tempdir does not contain
        // CMakePresets.json because raw_source only mirrors `test_case/` content.
        let test_config = find_test_config(&context.config.input);
        let workflow_hint = if config.workflow && agent == AgentKind::Claude {
            "You must use a workflow.\n\n".to_owned()
        } else {
            String::new()
        };
        let prompt = load_verify_prompt(&config, agent)?
            .replace(
                "{AGENT_BUG_WORKAROUNDS}",
                agent_runner::agent_bug_workarounds(agent),
            )
            .replace("{CMAKE_BUILD_FLAGS}", &test_config.cmake_flags)
            .replace("{ALL_CONFIGURATIONS}", &render_configurations(&test_config))
            .replace("{WISHLIST_PATH}", &local_wishlist.to_string_lossy())
            .replace("{AGENT_TOOLS_SECTION}", &agent_tools_section)
            .replace(
                "{RUST_TOOLCHAIN_CONTEXT}",
                &rust_toolchain_context.prompt_block,
            )
            .replace(
                "{WORKDIR_BOUNDARY}",
                &agent_runner::render_workdir_boundary(agent, &translated),
            )
            .replace("{WORKFLOW_HINT}", &workflow_hint)
            .replace(
                "{MODEL_LIMITS}",
                &match (agent, &config.model) {
                    (AgentKind::OpenCode, Some(model)) => {
                        let limits = agent_runner::load_opencode_model_limits(model)?;
                        agent_runner::render_model_limits_block(&limits)
                    }
                    _ => String::new(),
                },
            );

        // Materialize the gtest/fuzztest verification environment when that
        // harness is in effect. It matches the prompt the agent will see: the
        // no-plan ablation prompts are libloading-only, so skip it there.
        let gtest_harness_active = config.verify_harness == VerifyHarness::Gtest
            && !config.no_plan
            && !config.no_plan_file
            && matches!(agent, AgentKind::Claude | AgentKind::OpenCode);
        if gtest_harness_active {
            materialize_verify_env(&translated, &c_src_dir, config.fuzz)?;
            info!(
                "Materialized gtest verification environment (fuzz={})",
                config.fuzz
            );
        }

        // Kiro runs in case_dir (references translated_rust/ in prompt paths).
        // Claude and OpenCode run in translated_rust/ directly (references c_src/ and src/).
        let kiro_prompt;
        let (agent_work_dir, agent_prompt) = match agent {
            AgentKind::Kiro => {
                kiro_prompt = prompt.replace("{CASE_DIR}", &case_dir.to_string_lossy());
                (case_dir, kiro_prompt.as_str())
            }
            AgentKind::Claude | AgentKind::OpenCode => (translated.as_path(), prompt.as_str()),
        };
        agent_runner::invoke_agent(AgentInvocation {
            phase: AgentPhase::Verify,
            agent,
            work_dir: agent_work_dir,
            prompt: agent_prompt,
            timeout_secs: config.timeout_secs,
            model: config.model.as_deref(),
            no_plan: config.no_plan,
            no_plan_file: config.no_plan_file,
            extra_env: &config.env,
            output_log_path: config.output_log_path.as_deref(),
            rust_toolchain: Some(&rust_toolchain_context.required_version),
        })?;
        info!("Verification complete");

        // Append verify-phase wishlist entries to the translate-phase file (if any),
        // so the final output contains wishes from both phases in chronological order.
        if local_wishlist.exists() {
            if let Some(out_path) = &config.wishlist_output_path {
                match (
                    fs::read_to_string(&local_wishlist),
                    fs::read_to_string(out_path),
                ) {
                    (Ok(new_entries), Ok(existing)) => {
                        let merged = format!("{}{}", existing, new_entries);
                        if let Err(e) = fs::write(out_path, merged) {
                            warn!(
                                "Failed to append verify wishlist to {}: {}",
                                out_path.display(),
                                e
                            );
                        } else {
                            info!(
                                "Tool wishlist (verify phase) appended to {}",
                                out_path.display()
                            );
                        }
                    }
                    (Ok(new_entries), Err(_)) => {
                        // No translate-phase file yet — write fresh.
                        if let Err(e) = fs::write(out_path, new_entries) {
                            warn!(
                                "Failed to write verify wishlist to {}: {}",
                                out_path.display(),
                                e
                            );
                        } else {
                            info!("Tool wishlist written to {}", out_path.display());
                        }
                    }
                    (Err(e), _) => {
                        warn!(
                            "Failed to read local wishlist {}: {}",
                            local_wishlist.display(),
                            e
                        );
                    }
                }
            }
        }

        // Copy verify-phase HYPOTHESES.md out before the tempdir is dropped.
        // The agent maintains this as an append-only log of bug hypotheses across
        // its own compactions; absence is normal if no bugs needed investigation.
        let local_hypotheses = translated.join("HYPOTHESES.md");
        if local_hypotheses.exists() {
            if let Some(out_path) = &config.hypotheses_output_path {
                if let Err(e) = fs::copy(&local_hypotheses, out_path) {
                    warn!(
                        "Failed to copy HYPOTHESES.md to {}: {}",
                        out_path.display(),
                        e
                    );
                } else {
                    info!(
                        "Verification HYPOTHESES.md written to {}",
                        out_path.display()
                    );
                }
            }
        } else {
            info!(
                "Agent did not produce a HYPOTHESES.md (no bugs investigated, or skipped step 1)"
            );
        }

        // Sanitize translated_rust/ before freezing it into the IR.
        // 1) Remove hidden directories/files that leaked in from the agent runtime.
        // 2) Remove known intermediate build/source artifacts.
        // 3) Surface any remaining symlinks for diagnostics.
        remove_hidden_entries(&translated)?;

        let c_src_out = translated.join("c_src");
        if c_src_out.exists() {
            if let Err(e) = fs::remove_dir_all(&c_src_out) {
                warn!(
                    "Failed to remove c_src output dir {}: {}",
                    c_src_out.display(),
                    e
                );
            }
        }
        let target_out = translated.join("target");
        if target_out.exists() {
            if let Err(e) = fs::remove_dir_all(&target_out) {
                warn!(
                    "Failed to remove target output dir {}: {}",
                    target_out.display(),
                    e
                );
            }
        }

        // Keep the gtest/fuzztest verification environment in the frozen output
        // (like the libloading tests/ dir) so it is available for inspection,
        // but strip its build directories (build-*), which hold the fetched
        // GoogleTest/FuzzTest/Abseil sources plus symlinks that would otherwise
        // bloat the output and break the symlink-free freeze.
        let verify_env_out = translated.join(VERIFY_ENV_DIR);
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

        for entry in collect_symlinks(&translated) {
            warn!("translated_rust contains symlink: {}", entry);
        }

        let (dir, directories, files) = RawDir::populate_from(read_dir(&translated)?)?;
        info!("Produced CargoPackage with {directories} directories and {files} files");

        Ok(Box::new(CargoPackage { dir }))
    }
}

/// Builds the verification-method section (`{VERIFICATION_METHOD}` slot of the
/// standard prompt) for the selected harness, splicing in the FuzzTest guidance
/// when fuzzing is enabled.
fn build_method_section(config: &Config) -> String {
    match config.verify_harness {
        VerifyHarness::Gtest => {
            let section = if config.fuzz { FUZZTEST_SECTION } else { "" };
            METHOD_GTEST.replace("{FUZZTEST_SECTION}", section)
        },
        VerifyHarness::Libloading => METHOD_LIBLOADING.to_owned()
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
/// `<translated>/verify_env/`, filling the CMake template for the requested mode
/// and pre-seeding the C compile definitions parsed from `c_src/CMakeLists.txt`.
fn materialize_verify_env(
    translated: &Path,
    c_src_dir: &Path,
    fuzz: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let env_dir = translated.join(VERIFY_ENV_DIR);
    fs::create_dir_all(&env_dir)?;

    let c_defs = fs::read_to_string(c_src_dir.join("CMakeLists.txt"))
        .map(|s| parse_c_compile_defs(&s))
        .unwrap_or_default();

    let (declare, makeavail, setup_flags, test_link) = if fuzz {
        (
            format!(
                "FetchContent_Declare(\n  fuzztest\n  GIT_REPOSITORY https://github.com/google/fuzztest.git\n  GIT_TAG        {FUZZTEST_GIT_TAG}\n)\n"
            ),
            " fuzztest".to_string(),
            "\n# Enable fuzzing instrumentation AFTER the frameworks are configured, so\n# gtest/fuzztest/abseil are not themselves instrumented — only what follows.\nfuzztest_setup_fuzzing_flags()\n".to_string(),
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

    fs::write(env_dir.join("CMakeLists.txt"), cmakelists)?;
    fs::write(env_dir.join("verification_tests.cc"), VE_TESTS_CC)?;
    fs::write(env_dir.join("harvest_diff.h"), VE_DIFF_H)?;
    fs::write(env_dir.join("rust_lib.h"), VE_RUST_LIB_H)?;
    fs::write(env_dir.join("README.md"), VE_README)?;
    write_script(&env_dir.join("build.sh"), VE_BUILD_SH)?;
    if fuzz {
        write_script(&env_dir.join("build_fuzz.sh"), VE_BUILD_FUZZ_SH)?;
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

fn load_verify_prompt(
    config: &Config,
    agent: AgentKind,
) -> Result<String, Box<dyn std::error::Error>> {
    match agent {
        AgentKind::Claude | AgentKind::OpenCode => match &config.prompt_verify {
            Some(p) => Ok(fs::read_to_string(p)?),
            // The no-plan ablation prompts are libloading-only by construction
            // (they predate the harness split); the harness/fuzz switch applies
            // to the standard prompt.
            None if config.no_plan => Ok(PROMPT_VERIFY_NO_PLAN.to_owned()),
            None if config.no_plan_file => Ok(PROMPT_VERIFY_NO_PLAN_FILE.to_owned()),
            None => {
                Ok(PROMPT_VERIFY.replace("{VERIFICATION_METHOD}", &build_method_section(config)))
            }
        },
        AgentKind::Kiro => match &config.prompt_kiro_verify {
            Some(p) => Ok(fs::read_to_string(p)?),
            None => Ok(PROMPT_KIRO_VERIFY.to_owned()),
        },
    }
}

/// Remove hidden entries (names starting with `.`) under `dir`, including nested ones.
/// This prevents agent-runtime artifacts like `.opencode/` from leaking into the IR.
fn remove_hidden_entries(dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let hidden = name.to_string_lossy().starts_with('.');
            if hidden {
                let path = entry.path();
                if let Err(e) = fs::remove_dir_all(&path) {
                    if path.is_dir() {
                        warn!(
                            "Failed to remove hidden directory {}: {}",
                            path.display(),
                            e
                        );
                    } else if let Err(e2) = fs::remove_file(&path) {
                        warn!("Failed to remove hidden entry {}: {}", path.display(), e2);
                    }
                }
            } else if entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false) {
                remove_hidden_entries(&entry.path())?;
            }
        }
    }
    Ok(())
}

/// Collects symlink paths under `dir` for debugging.
fn collect_symlinks(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if let Ok(metadata) = entry.metadata() {
                if metadata.file_type().is_symlink() {
                    out.push(path.display().to_string());
                } else if metadata.is_dir() {
                    out.extend(collect_symlinks(&path));
                }
            }
        }
    }
    out
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
#[derive(Debug, Deserialize, Default, PartialEq, Eq, Clone, Copy)]
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
#[derive(Debug, Deserialize)]
pub struct Config {
    /// Override path for the Kiro verification prompt.
    pub prompt_kiro_verify: Option<PathBuf>,

    /// Override path for the standard verification prompt.
    #[serde(alias = "prompt_claude_verify")]
    pub prompt_verify: Option<PathBuf>,

    /// Agent timeout in seconds.
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,

    /// Agent model to use. If absent, no --model flag is passed and the CLI uses its default.
    /// Claude accepts short aliases ("sonnet", "opus", "haiku") or full model IDs.
    /// OpenCode expects provider/model format.
    pub model: Option<String>,

    /// If true, use the pre-anti-compaction prompt (no HYPOTHESES.md / Invariants
    /// / sub-agent push) and skip the `--append-system-prompt` flag. Intended
    /// for controlled experiments measuring the impact of the anti-compaction
    /// mechanism added in 883e2e2.
    #[serde(default)]
    pub no_plan: bool,

    /// If true, use the ablation prompt that keeps the sub-agent push and
    /// context-management guidance but never mentions HYPOTHESES.md/PLAN.md
    /// or writing logs to disk (the agent may still do so spontaneously),
    /// and skip the `--append-system-prompt` flag. Isolates the effect of
    /// plan-file persistence from sub-agent usage. Mutually exclusive with
    /// `no_plan`.
    #[serde(default)]
    pub no_plan_file: bool,

    /// Inject a prompt hint encouraging the agent to use dynamic workflows.
    /// Only meaningful with no_plan.
    #[serde(default)]
    pub workflow: bool,

    /// Which comparison mechanism the verification prompt describes.
    #[serde(default)]
    pub verify_harness: VerifyHarness,

    /// When `verify_harness = gtest`, also describe FuzzTest as an
    /// available capability and ship its scaffolding.
    /// No effect under the libloading harness.
    #[serde(default)]
    pub fuzz: bool,

    /// Extra environment variables to inject into the agent process.
    /// Useful for CCR provider API keys, proxy settings, etc.
    /// Defined as a TOML table under `[tools.verify_fix_agentic.env]`.
    #[serde(default)]
    pub env: HashMap<String, String>,

    /// Destination path for the agent's tool wishlist file.
    /// Injected by the benchmark at runtime (set to <output_dir>/tool_wishlist.json).
    /// Verify-phase entries are appended to any existing translate-phase entries at this path.
    /// If absent, any wishlist the agent writes is silently discarded with the tempdir.
    pub wishlist_output_path: Option<PathBuf>,

    /// Destination path for the agent's verify-phase HYPOTHESES.md.
    /// Injected by the benchmark at runtime (set to <output_dir>/hypotheses_verify.md).
    /// If absent, any hypotheses log the agent writes is preserved only inside the
    /// CargoPackage and may be lost in downstream tooling.
    pub hypotheses_output_path: Option<PathBuf>,

    /// Unix timestamp. If set, the verification agent will sleep until this
    /// time before starting. Used to align with the 5-hour free window reset.
    /// If the current time is already past the timestamp, verification starts
    /// immediately.
    #[serde(default)]
    pub wait_until: Option<u64>,

    /// Destination path for the benchmark's output.log file.
    /// Injected by the benchmark at runtime so the agent's full trace
    /// (JSON stream) is appended to the same log file as benchmark messages.
    pub output_log_path: Option<PathBuf>,

    #[serde(flatten)]
    unknown: HashMap<String, serde_json::Value>,
}

fn default_timeout_secs() -> u64 {
    36000
}

impl Config {
    fn validate(&self) {
        unknown_field_warning("tools.verify_fix_agentic", &self.unknown);
        if self.fuzz && self.verify_harness != VerifyHarness::Gtest {
            warn!(
                "tools.verify_fix_agentic: fuzz has no effect without verify_harness = gtest; ignoring"
            );
        }
        if self.verify_harness == VerifyHarness::Gtest && (self.no_plan || self.no_plan_file) {
            warn!(
                "tools.verify_fix_agentic: verify_harness = gtest is not applied under no_plan/no_plan_file (those prompts are libloading-only); using libloading"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn gtest_only_cmake_has_no_fuzztest() {
        let tmp = tempfile::tempdir().unwrap();
        let translated = tmp.path();
        let c_src = translated.join("c_src");
        fs::create_dir_all(&c_src).unwrap();
        fs::write(
            c_src.join("CMakeLists.txt"),
            "target_compile_definitions(x PRIVATE XXH_NAMESPACE=LZ4_)\n",
        )
        .unwrap();

        materialize_verify_env(translated, &c_src, false).unwrap();
        let cml = fs::read_to_string(translated.join("verify_env/CMakeLists.txt")).unwrap();
        assert!(cml.contains("XXH_NAMESPACE=LZ4_"));
        assert!(cml.contains("GTest::gtest_main"));
        assert!(!cml.contains("fuzztest"));
        assert!(!translated.join("verify_env/build_fuzz.sh").exists());
    }

    // Escape hatch for the manual integration check: when VE_OUT is set,
    // materialize a real verify_env there (fuzz per VE_FUZZ=1) so it can be
    // built and run by hand. Ignored in normal test runs.
    #[test]
    #[ignore]
    fn materialize_to_ve_out() {
        let out = std::path::PathBuf::from(std::env::var("VE_OUT").unwrap());
        let fuzz = std::env::var("VE_FUZZ").ok().as_deref() == Some("1");
        materialize_verify_env(&out, &out.join("c_src"), fuzz).unwrap();
    }

    #[test]
    fn strip_build_dirs_keeps_source() {
        let tmp = tempfile::tempdir().unwrap();
        let env = tmp.path().join("verify_env");
        fs::create_dir_all(env.join("build-test/_deps")).unwrap();
        fs::create_dir_all(env.join("build-fuzz")).unwrap();
        fs::write(env.join("verification_tests.cc"), "// test").unwrap();
        fs::write(env.join("CMakeLists.txt"), "# cmake").unwrap();
        fs::write(env.join("build-test/_deps/junk.o"), "junk").unwrap();

        let removed = strip_build_dirs(&env).unwrap();
        assert_eq!(removed, 2, "both build-* dirs should be removed");
        assert!(env.join("verification_tests.cc").exists());
        assert!(env.join("CMakeLists.txt").exists());
        assert!(!env.join("build-test").exists());
        assert!(!env.join("build-fuzz").exists());
    }

    #[test]
    fn fuzz_cmake_wires_fuzztest() {
        let tmp = tempfile::tempdir().unwrap();
        let translated = tmp.path();
        let c_src = translated.join("c_src");
        fs::create_dir_all(&c_src).unwrap();
        fs::write(c_src.join("CMakeLists.txt"), "").unwrap();

        materialize_verify_env(translated, &c_src, true).unwrap();
        let cml = fs::read_to_string(translated.join("verify_env/CMakeLists.txt")).unwrap();
        assert!(cml.contains("link_fuzztest(verification_tests)"));
        assert!(cml.contains("fuzztest_setup_fuzzing_flags()"));
        assert!(cml.contains(FUZZTEST_GIT_TAG));
        assert!(!cml.contains("GTest::gtest_main"));
        assert!(translated.join("verify_env/build_fuzz.sh").exists());
    }
}
