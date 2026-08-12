//! Agentic translation tool for HARVEST.
//!
//! Translates a C project (as a [`RawSource`](full_source::RawSource)) into a Rust Cargo project
//! by invoking an external agent. After the agent finishes, the generated `Cargo.toml` is
//! post-processed to satisfy downstream tool expectations.
//! The translated project is stored in the IR as a [`CargoPackage`](full_source::CargoPackage).

use agent_runner::{AgentInvocation, AgentPhase};
use build_project_spec::{ProjectKind, ProjectSpec};
use full_source::{CargoPackage, RawSource};
use harvest_core::cargo_utils::{CargoToml, strip_for_lib};
use harvest_core::config::{AgentKind, unknown_field_warning};
use harvest_core::fs::{RawDir, ReferenceGuard, collect_symlinks, remove_hidden_entries};
use harvest_core::tools::{RunContext, Tool};
use harvest_core::{Id, Representation};
use serde::Deserialize;
use std::collections::HashMap;
use std::fs::{self, read_dir};
use std::path::{Path, PathBuf};
use tracing::{info, warn};

/// Guard that preserves a temp directory when a run fails, but removes it on success.
struct TempDirGuard {
    inner: Option<tempfile::TempDir>,
}

impl TempDirGuard {
    fn new(inner: tempfile::TempDir) -> Self {
        Self { inner: Some(inner) }
    }

    fn path(&self) -> &Path {
        self.inner
            .as_ref()
            .expect("tempdir guard already consumed")
            .path()
    }

    fn finish(mut self) {
        self.inner = None;
    }
}

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        if let Some(inner) = self.inner.take() {
            let path = inner.keep();
            if path.exists() {
                if let Err(e) = fs::remove_dir_all(&path) {
                    warn!(
                        "Failed to remove preserved tempdir {}: {}",
                        path.display(),
                        e
                    );
                } else {
                    info!("Removed tempdir {}", path.display());
                }
            }
        }
    }
}

const PROMPT_KIRO_EXECUTABLE: &str = include_str!("prompt_kiro_executable.md");
const PROMPT_KIRO_LIBRARY: &str = include_str!("prompt_kiro_library.md");
const PROMPT_KIRO_CONFIGURABLE: &str = include_str!("prompt_kiro_configurable.md");
const PROMPT_TRANSLATE: &str = include_str!("prompt_translate.md");
const PROMPT_TRANSLATE_NO_PLAN: &str = include_str!("prompt_translate_no_plan.md");
const PROMPT_TRANSLATE_NO_PLAN_FILE: &str = include_str!("prompt_translate_no_plan_file.md");

pub struct TranslateAgentic;

impl Tool for TranslateAgentic {
    fn name(&self) -> &'static str {
        "translate_agentic"
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
                .get("translate_agentic")
                .unwrap_or(&default_config),
        )?;
        config.validate();

        let raw_source = context
            .ir_snapshot
            .get::<RawSource>(inputs[0])
            .ok_or("No RawSource representation found in IR")?;
        let project_spec = context
            .ir_snapshot
            .get::<ProjectSpec>(inputs[1])
            .ok_or("No ProjectSpec representation found in IR")?;

        let agent = context.config.agentic_agent;
        if config.no_plan && config.no_plan_file {
            return Err(
                "tools.translate_agentic: no_plan and no_plan_file are mutually exclusive".into(),
            );
        }
        let translate_prompt = load_prompt(&config, &project_spec.kind, agent)?;
        let no_plan = config.no_plan;

        // Set up a working directory that mirrors the layout the agent expects:
        //   case_dir/translated_rust/c_src/  <- materialized C source
        let work_dir = tempfile::tempdir()?;
        let guard = TempDirGuard::new(work_dir);
        let case_dir = guard.path().to_path_buf();
        eprintln!(
            "[translate_agentic] preserved tempdir for debugging: {}",
            case_dir.display()
        );
        let c_src_dir = case_dir.join("translated_rust/c_src");
        fs::create_dir_all(&c_src_dir)?;
        raw_source.dir.materialize(&c_src_dir)?;

        info!("Working directory: {}", case_dir.display());

        let translated = case_dir.join("translated_rust");
        // c_src/ is a read-only input.
        let reference_guard = ReferenceGuard::capture(&translated, &["c_src"])?;

        // Materialize agent tools if enabled and build the prompt section.
        // When disabled, {AGENT_TOOLS_SECTION} is replaced with an empty string so
        // the entire "Available Tools" block is absent from the prompt the agent sees.
        let agent_tools_section = if context.config.agent_tools {
            let dir = translated.join("agent_tools");
            agent_tools_embed::materialize_to(&dir)?;
            let docs = agent_tools_embed::collect_docs()
                .replace("{AGENT_TOOLS_DIR}", &dir.to_string_lossy());
            format!(
                "### Available Tools\n\n\
                 The following tools are pre-installed in `{}/`. Use them when you\n\
                 need a precise answer about C behavior rather than reasoning from first principles.\n\n\
                 {}\n",
                dir.display(),
                docs
            )
        } else {
            String::new()
        };

        // The wishlist file lives inside the agent's working directory so the agent
        // can write to it without any special permissions. We inject the absolute
        // path into the prompt so the agent knows exactly where to append entries.
        let local_wishlist = translated.join("tool_wishlist.json");
        let rust_toolchain_context = agent_runner::detect_rust_toolchain_context(
            &context.config.input,
            config.test_corpus_root.as_deref(),
        )?;
        let workflow_hint = if config.workflow && agent == AgentKind::Claude {
            "You must use a workflow.\n\n".to_owned()
        } else {
            String::new()
        };
        let translate_prompt = translate_prompt
            .replace(
                "{AGENT_BUG_WORKAROUNDS}",
                agent_runner::agent_bug_workarounds(agent),
            )
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

        agent_runner::invoke_agent(AgentInvocation {
            phase: AgentPhase::Translate,
            agent,
            work_dir: &translated,
            prompt: &translate_prompt,
            timeout_secs: config.timeout_secs,
            model: config.model.as_deref(),
            no_plan,
            no_plan_file: config.no_plan_file,
            extra_env: &config.env,
            output_log_path: config.output_log_path.as_deref(),
            rust_toolchain: Some(&rust_toolchain_context.required_version),
        })?;

        // Copy the wishlist out before the tempdir is dropped.
        if local_wishlist.exists()
            && let Some(out_path) = &config.wishlist_output_path
        {
            if let Err(e) = fs::copy(&local_wishlist, out_path) {
                warn!(
                    "Failed to copy tool wishlist to {}: {}",
                    out_path.display(),
                    e
                );
            } else {
                info!("Tool wishlist written to {}", out_path.display());
            }
        }

        // Copy PLAN.md out before the tempdir is dropped. Translate-phase PLAN.md
        // is preserved under a dedicated name so it survives any verify-phase
        // rewrite of the CargoPackage. PLAN.md only exists if the agent decided
        // it was in the Medium/Large regime; absence is normal for small projects.
        let local_plan = translated.join("PLAN.md");
        if local_plan.exists() {
            if let Some(out_path) = &config.plan_output_path {
                if let Err(e) = fs::copy(&local_plan, out_path) {
                    warn!("Failed to copy PLAN.md to {}: {}", out_path.display(), e);
                } else {
                    info!("Translation PLAN.md written to {}", out_path.display());
                }
            }
        } else {
            info!("Agent did not produce a PLAN.md (likely small-regime project)");
        }

        if !translated.join("Cargo.toml").exists() {
            return Err("Agent did not produce a Cargo.toml".into());
        }

        post_process(&translated, &project_spec.kind)?;
        info!("Translation complete");

        // Sanitize translated_rust/ before freezing it into the IR.
        // 1) Remove hidden directories/files that leaked in from the agent runtime.
        // 2) Discard the read-only references, reporting any the agent edited.
        // 3) Remove intermediate build artifacts.
        // 4) Surface any remaining symlinks for diagnostics.
        remove_hidden_entries(&translated)?;
        reference_guard.strip(&translated, config.rejected_output_dir.as_deref())?;

        let target_out = translated.join("target");
        if target_out.exists()
            && let Err(e) = fs::remove_dir_all(&target_out)
        {
            warn!(
                "Failed to remove target output dir {}: {}",
                target_out.display(),
                e
            );
        }

        for entry in collect_symlinks(&translated) {
            warn!("translated_rust contains symlink: {}", entry);
        }

        let (dir, directories, files) = RawDir::populate_from(read_dir(&translated)?)?;
        info!("Produced CargoPackage with {directories} directories and {files} files");

        guard.finish();
        Ok(Box::new(CargoPackage { dir }))
    }
}

/// Applies standard Cargo.toml fixups after the agent finishes.
fn post_process(
    translated: &Path,
    project_kind: &ProjectKind,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut cargo = CargoToml::open(&translated.join("Cargo.toml"))?;
    cargo.add_workspace();
    match project_kind {
        ProjectKind::Library => {
            cargo.remove_bin();
            // Read the package name from the in-memory doc before overwriting [lib].
            if let Some(name) = cargo.package_name() {
                cargo.set_lib(&name);
            }
            cargo.save()?;
            strip_for_lib(translated)?;
        }
        ProjectKind::Executable => {
            cargo.set_bin_driver();
            cargo.save()?;
        }
        ProjectKind::Configurable => {
            // The configurable prompt instructs the agent to produce both [lib] and [[bin]].
            // Only ensure workspace is present (already added above); leave the rest intact.
            cargo.save()?;
        }
    }
    Ok(())
}

/// Loads the translate prompt, selecting between Kiro (per-kind) and Claude (unified) variants.
fn load_prompt(
    config: &Config,
    kind: &ProjectKind,
    agent: AgentKind,
) -> Result<String, Box<dyn std::error::Error>> {
    match agent {
        AgentKind::Claude | AgentKind::OpenCode => match &config.prompt_translate {
            Some(p) => Ok(fs::read_to_string(p)?),
            None if config.no_plan => Ok(PROMPT_TRANSLATE_NO_PLAN.to_owned()),
            None if config.no_plan_file => Ok(PROMPT_TRANSLATE_NO_PLAN_FILE.to_owned()),
            None => Ok(PROMPT_TRANSLATE.to_owned()),
        },
        AgentKind::Kiro => {
            let (config_path, builtin) = match kind {
                ProjectKind::Executable => (&config.prompt_kiro_executable, PROMPT_KIRO_EXECUTABLE),
                ProjectKind::Library => (&config.prompt_kiro_library, PROMPT_KIRO_LIBRARY),
                ProjectKind::Configurable => {
                    (&config.prompt_kiro_configurable, PROMPT_KIRO_CONFIGURABLE)
                }
            };
            match config_path {
                Some(p) => Ok(fs::read_to_string(p)?),
                None => Ok(builtin.to_owned()),
            }
        }
    }
}

/// Tool-specific configuration, read from `[tools.translate_agentic]` in the HARVEST config.
#[derive(Debug, Deserialize)]
pub struct Config {
    /// Override path for the Kiro executable translation prompt.
    #[serde(alias = "prompt_executable")]
    pub prompt_kiro_executable: Option<PathBuf>,

    /// Override path for the Kiro library translation prompt.
    #[serde(alias = "prompt_library")]
    pub prompt_kiro_library: Option<PathBuf>,

    /// Override path for the Kiro configurable translation prompt.
    #[serde(alias = "prompt_configurable")]
    pub prompt_kiro_configurable: Option<PathBuf>,

    /// Override path for the standard unified translation prompt.
    #[serde(alias = "prompt_claude_translate")]
    pub prompt_translate: Option<PathBuf>,

    /// Agent timeout in seconds.
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,

    /// Agent model to use. If absent, no --model flag is passed and the CLI uses its default.
    /// Claude accepts short aliases ("sonnet", "opus", "haiku") or full model IDs.
    /// When Claude is set to a "provider,model" format (e.g. "openrouter,deepseek/deepseek-v4-pro"),
    /// Claude Code is routed through claude-code-router. OpenCode expects provider/model format.
    pub model: Option<String>,

    /// If true, use the pre-anti-compaction prompt (no PLAN.md / Invariants /
    /// sub-agent push) and skip the `--append-system-prompt` flag. Intended for
    /// controlled experiments measuring the impact of the anti-compaction
    /// mechanism added in 883e2e2.
    #[serde(default)]
    pub no_plan: bool,

    /// If true, use the ablation prompt that keeps the sub-agent push and
    /// context-management guidance but never mentions PLAN.md or writing
    /// plans to disk (the agent may still do so spontaneously), and skip the
    /// `--append-system-prompt` flag. Isolates the effect of plan-file
    /// persistence from sub-agent usage. Mutually exclusive with `no_plan`.
    #[serde(default)]
    pub no_plan_file: bool,

    /// Inject a prompt hint encouraging the agent to use dynamic workflows
    /// (Claude Code's multi-agent orchestration). Only meaningful with no_plan.
    #[serde(default)]
    pub workflow: bool,

    /// Extra environment variables to inject into the agent process.
    /// Useful for CCR provider API keys, proxy settings, etc.
    /// Defined as a TOML table under `[tools.translate_agentic.env]`.
    /// Example:
    ///   [tools.translate_agentic.env]
    ///   OPENROUTER_API_KEY = "sk-or-..."
    ///   OPENCODE_GO_API_KEY = "..."
    #[serde(default)]
    pub env: HashMap<String, String>,

    /// Destination path for the agent's tool wishlist file.
    /// Injected by the benchmark at runtime (set to <output_dir>/tool_wishlist.json).
    /// If absent, any wishlist the agent writes is silently discarded with the tempdir.
    pub wishlist_output_path: Option<PathBuf>,

    /// Destination path for the agent's translate-phase PLAN.md.
    /// Injected by the benchmark at runtime (set to <output_dir>/plan_translate.md).
    /// If absent, any plan the agent writes is preserved only inside the CargoPackage
    /// and may be lost when verify rewrites it.
    pub plan_output_path: Option<PathBuf>,

    /// Destination path for the benchmark's output.log file.
    /// Injected by the benchmark at runtime so the agent's full trace
    /// (JSON stream) is appended to the same log file as benchmark messages.
    pub output_log_path: Option<PathBuf>,

    /// Directory where a read-only reference the agent modified is preserved
    /// for review (`.harvest/rejected/<stage>/`). Injected by the benchmark.
    pub rejected_output_dir: Option<PathBuf>,

    /// The Test-Corpus / cando2 checkout defining the Rust toolchain contract.
    /// Injected from the stage manifest when resuming from a snapshot, whose
    /// C source no longer sits inside the corpus.
    pub test_corpus_root: Option<PathBuf>,

    #[serde(flatten)]
    unknown: HashMap<String, serde_json::Value>,
}

fn default_timeout_secs() -> u64 {
    36000
}

impl Config {
    fn validate(&self) {
        unknown_field_warning("tools.translate_agentic", &self.unknown);
    }
}
