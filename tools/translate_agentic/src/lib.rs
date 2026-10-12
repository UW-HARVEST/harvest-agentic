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
use harvest_core::fs::{
    RawDir, ReferenceGuard, collect_symlinks, remove_hidden_entries, remove_worktree_dir,
};
use harvest_core::tools::{RunContext, Tool};
use harvest_core::{Id, Representation};
use serde::Deserialize;
use std::collections::HashMap;
use std::fs::{self, read_dir};
use std::io;
use std::path::{Path, PathBuf};
use tracing::{info, warn};

mod skills;

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

const SCOUT: &str = include_str!("scout.md");
const SCOUT_TAB2RUST: &str = include_str!("scout_tab2rust.md");
const TAB2RUST_TRANSLATOR: &str = include_str!("tab2rust_translator.md");
const TAB2RUST_INVARIANT: &str = include_str!("tab2rust_invariant.md");

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
        let translate_prompt = load_prompt(&config, &project_spec.kind, agent);
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

        // Scouts explore c_src/ for the main agent. Their task, and with tab2rust the skill and
        // the rules for its users, go into the working directory. scout/ also keeps the reports.
        let scouts = sends_scouts(&config, agent);
        let tab2rust = scouts && config.tab2rust;
        if scouts {
            write_scout_files(&translated, tab2rust)?;
        }

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
        let translate_prompt = fill_tab2rust(translate_prompt, tab2rust, &translated)
            .replace(
                "{AGENT_BUG_WORKAROUNDS}",
                agent_runner::agent_bug_workarounds(agent),
            )
            .replace("{GIT_DISCIPLINE}", agent_runner::git_discipline())
            .replace("{WISHLIST_PATH}", &local_wishlist.to_string_lossy())
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
        // rewrite of the CargoPackage. The plan prompt always asks for PLAN.md;
        // the no-plan and Kiro prompts never do.
        let local_plan = translated.join("PLAN.md");
        if local_plan.exists() {
            if let Some(out_path) = &config.plan_output_path {
                if let Err(e) = fs::copy(&local_plan, out_path) {
                    warn!("Failed to copy PLAN.md to {}: {}", out_path.display(), e);
                } else {
                    info!("Translation PLAN.md written to {}", out_path.display());
                }
            }
        } else if scouts {
            warn!("Agent did not produce a PLAN.md, although the prompt asks for one");
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

        if let Err(e) = remove_worktree_dir(&translated) {
            warn!("Failed to remove leftover worktree dir: {e}");
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
            // Keep the agent's [lib].name; without one, use the name that Cargo would derive
            // from the package name ('-' becomes '_': a lib target name cannot contain '-').
            if let Some(name) = cargo.lib_name() {
                cargo.set_lib(&name.replace('-', "_"));
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
fn load_prompt(config: &Config, kind: &ProjectKind, agent: AgentKind) -> &'static str {
    match agent {
        AgentKind::Claude | AgentKind::OpenCode if config.no_plan => PROMPT_TRANSLATE_NO_PLAN,
        AgentKind::Claude | AgentKind::OpenCode => PROMPT_TRANSLATE,
        AgentKind::Kiro => match kind {
            ProjectKind::Executable => PROMPT_KIRO_EXECUTABLE,
            ProjectKind::Library => PROMPT_KIRO_LIBRARY,
            ProjectKind::Configurable => PROMPT_KIRO_CONFIGURABLE,
        },
    }
}

/// Whether the prompt sends scouts: the plan prompt of Claude and OpenCode.
fn sends_scouts(config: &Config, agent: AgentKind) -> bool {
    agent != AgentKind::Kiro && !config.no_plan
}

/// The scout task, with the tab2rust part when tab2rust is on.
fn render_scout(tab2rust: bool, work_dir: &Path) -> String {
    let scout = SCOUT.replace(
        "{TAB2RUST_SCOUT}",
        if tab2rust { SCOUT_TAB2RUST } else { "" },
    );
    fill_tab2rust(&scout, tab2rust, work_dir)
}

/// Fills the tab2rust placeholder of a prompt: the data-table rule, or nothing.
/// `{WORK_DIR}` becomes the absolute working directory: sub-agents that work in a git worktree
/// reach the build in `scout/tab2rust/c-build/` (untracked) only by an absolute path.
fn fill_tab2rust(prompt: &str, tab2rust: bool, work_dir: &Path) -> String {
    let invariant = if tab2rust { TAB2RUST_INVARIANT } else { "" };
    prompt
        .replace("{TAB2RUST_INVARIANT}", invariant)
        .replace("{WORK_DIR}", &work_dir.to_string_lossy())
}

/// Writes `scout/SCOUT.md` and, with tab2rust, the skill (`agent_skills/tab2rust/`) and
/// `scout/tab2rust/TRANSLATOR.md`.
fn write_scout_files(translated: &Path, tab2rust: bool) -> io::Result<()> {
    let scout = translated.join("scout");
    fs::create_dir_all(&scout)?;
    fs::write(scout.join("SCOUT.md"), render_scout(tab2rust, translated))?;
    if tab2rust {
        skills::materialize("tab2rust", &translated.join("agent_skills/tab2rust"))?;
        fs::create_dir_all(scout.join("tab2rust"))?;
        fs::write(
            scout.join("tab2rust/TRANSLATOR.md"),
            fill_tab2rust(TAB2RUST_TRANSLATOR, true, translated),
        )?;
    }
    Ok(())
}

/// Tool-specific configuration, read from `[tools.translate_agentic]` in the HARVEST config.
#[derive(Debug, Deserialize)]
pub struct Config {
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

    /// Inject a prompt hint encouraging the agent to use dynamic workflows
    /// (Claude Code's multi-agent orchestration). Only meaningful with no_plan.
    #[serde(default)]
    pub workflow: bool,

    /// Give the scouts and the translation sub-agents the tab2rust skill
    /// (`agent_skills/tab2rust/`): scout 1 finds the C data tables, and the sub-agents generate
    /// them as Rust. On by default; the benchmark flag `--no-tab2rust` turns it off. Only the
    /// prompts that send scouts use it.
    #[serde(default = "default_tab2rust")]
    pub tab2rust: bool,

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

fn default_tab2rust() -> bool {
    true
}

fn default_timeout_secs() -> u64 {
    agent_runner::DEFAULT_AGENT_TIMEOUT_SECS
}

impl Config {
    fn validate(&self) {
        unknown_field_warning("tools.translate_agentic", &self.unknown);
    }
}
