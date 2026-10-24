//! Agentic translation tool for HARVEST.
//!
//! Translates a C project (as a [`RawSource`](full_source::RawSource)) into a Rust Cargo project
//! by invoking an external agent. After the agent finishes, the generated `Cargo.toml` is
//! post-processed to satisfy downstream tool expectations.
//! The translated project is stored in the IR as a [`CargoPackage`](full_source::CargoPackage).

use agent_runner::{AgentInvocation, AgentPhase, CaseDir};
use build_project_spec::{ProjectKind, ProjectSpec};
use full_source::{CargoPackage, RawSource};
use harvest_core::cargo_utils::{CargoToml, strip_for_lib};
use harvest_core::config::{AgentKind, ToolConfig, unknown_field_warning};
use harvest_core::tools::{RunContext, Tool};
use harvest_core::{Id, Representation};
use include_dir::{Dir, include_dir};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use tracing::{info, warn};

mod skills;

const PROMPT_KIRO_EXECUTABLE: &str = include_str!("prompt_kiro_executable.md");
const PROMPT_KIRO_LIBRARY: &str = include_str!("prompt_kiro_library.md");
const PROMPT_KIRO_CONFIGURABLE: &str = include_str!("prompt_kiro_configurable.md");
const PROMPT_TRANSLATE: &str = include_str!("prompt_translate.md");
const PROMPT_TRANSLATE_NO_PLAN: &str = include_str!("prompt_translate_no_plan.md");

const SCOUT: &str = include_str!("scout.md");
const SCOUT_TAB2RUST: &str = include_str!("scout_tab2rust.md");
const TAB2RUST_TRANSLATOR: &str = include_str!("tab2rust_translator.md");
const TAB2RUST_INVARIANT: &str = include_str!("tab2rust_invariant.md");
const SAFE_WRAPPER_DESIGNER: &str = include_str!("safe_wrapper_designer.md");
const SAFE_WRAPPER_TRANSLATOR: &str = include_str!("safe_wrapper_translator.md");
const SAFE_WRAPPER_DISPATCH: &str = include_str!("safe_wrapper_dispatch.md");
const SAFE_WRAPPER_INVARIANT: &str = include_str!("safe_wrapper_invariant.md");
static SAFE_WRAPPER_RT: Dir = include_dir!("$CARGO_MANIFEST_DIR/safe_wrapper_rt/src");

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
        let config: Config = context.config.tool()?;
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
        if !config.tab2rust {
            check_scout_option("tab2rust = false", config.no_plan, agent)?;
        }
        if config.safe_wrapper {
            check_scout_option("safe_wrapper = true", config.no_plan, agent)?;
        }
        let translate_prompt = load_prompt(&config, &project_spec.kind, agent);
        let no_plan = config.no_plan;

        // The agent works in case_dir/workspace/. The C source and, with scouts, the task files and
        // the tab2rust skill are inputs in case_dir/inputs/ (see agent_runner::CaseDir).
        let case = CaseDir::new()?;
        let workspace = case.workspace();
        raw_source.dir.materialize(case.c_src())?;
        info!("Working directory: {}", workspace.display());

        let scouts = sends_scouts(config.no_plan, agent);
        let tab2rust = scouts && config.tab2rust;
        let safe_wrapper = scouts && config.safe_wrapper;
        if scouts {
            write_scout_files(&case, tab2rust, safe_wrapper)?;
        }
        let reference_guard = case.lock_inputs()?;

        let rust_toolchain_context = agent_runner::detect_rust_toolchain_context(
            &context.config.input,
            config.test_corpus_root.as_deref(),
        )?;
        let workflow_hint = if config.workflow && agent == AgentKind::Claude {
            "You must use a workflow.\n\n"
        } else {
            ""
        };
        let translate_prompt =
            fill_safe_wrapper(&fill_tab2rust(translate_prompt, tab2rust), safe_wrapper)
                .replace("{WORKFLOW_HINT}", workflow_hint);

        agent_runner::invoke_agent(AgentInvocation {
            phase: AgentPhase::Translate,
            agent,
            case: &case,
            prompt: &translate_prompt,
            timeout_secs: config.timeout_secs,
            model: config.model.as_deref(),
            subagent_model: config.subagent_model.as_deref(),
            no_plan,
            teamclaude: context.config.teamclaude,
            extra_env: &config.env,
            output_log_path: config.output_log_path.as_deref(),
            rust_toolchain: &rust_toolchain_context,
        })?;

        // The plan prompt always asks for PLAN.md; the no-plan and Kiro prompts never do.
        if scouts && !workspace.join("PLAN.md").exists() {
            warn!("Agent did not produce a PLAN.md, although the prompt asks for one");
        }
        if !workspace.join("Cargo.toml").exists() {
            return Err("Agent did not produce a Cargo.toml".into());
        }

        post_process(&workspace, &project_spec.kind)?;
        info!("Translation complete");

        let dir = case.freeze(&reference_guard, config.reference_audit_dir.as_deref())?;
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
fn sends_scouts(no_plan: bool, agent: AgentKind) -> bool {
    agent != AgentKind::Kiro && !no_plan
}

/// Refuses an option of the scout wave (`tab2rust = false`, `safe_wrapper = true`) where it has
/// no effect: only the prompt that sends scouts (the plan prompt of Claude and OpenCode) uses
/// tab2rust and the Safe API Wrapper Designer. The translate stage checks the resolved config; the
/// benchmark checks its command line before the run starts.
pub fn check_scout_option(option: &str, no_plan: bool, agent: AgentKind) -> Result<(), String> {
    if sends_scouts(no_plan, agent) {
        return Ok(());
    }
    Err(format!(
        "{option} has no effect with agent {agent}{}: only the plan prompt of Claude and \
         OpenCode sends scouts",
        if no_plan { " and no_plan" } else { "" }
    ))
}

/// The scout task, with the tab2rust part when tab2rust is on.
fn render_scout(tab2rust: bool) -> String {
    let scout = SCOUT.replace(
        "{TAB2RUST_SCOUT}",
        if tab2rust { SCOUT_TAB2RUST } else { "" },
    );
    fill_tab2rust(&scout, tab2rust)
}

/// Fills the tab2rust placeholder of a prompt: the data-table rule, or nothing.
fn fill_tab2rust(prompt: &str, tab2rust: bool) -> String {
    let invariant = if tab2rust { TAB2RUST_INVARIANT } else { "" };
    prompt.replace("{TAB2RUST_INVARIANT}", invariant)
}

/// Fills the safe-wrapper placeholders of a prompt: the designer in the scout wave and the design
/// rule, or nothing.
fn fill_safe_wrapper(prompt: &str, safe_wrapper: bool) -> String {
    let (dispatch, invariant) = if safe_wrapper {
        (SAFE_WRAPPER_DISPATCH, SAFE_WRAPPER_INVARIANT)
    } else {
        ("", "")
    };
    prompt
        .replace("{SAFE_WRAPPER_DISPATCH}", dispatch)
        .replace("{SAFE_WRAPPER_INVARIANT}", invariant)
}

/// Writes the scout inputs into `inputs/`: `prompts/SCOUT.md`; with tab2rust, the skill
/// (`agent_skills/tab2rust/`) and `prompts/TRANSLATOR.md`; with safe_wrapper,
/// `prompts/SAFE_WRAPPER_DESIGNER.md`, `prompts/SAFE_WRAPPER_TRANSLATOR.md` and the framework items
/// (`safe_wrapper_rt/`), from which the designer copies the parts that its design uses. The scouts
/// and the designer write their reports in `workspace/scout/`, which is created here.
fn write_scout_files(case: &CaseDir, tab2rust: bool, safe_wrapper: bool) -> io::Result<()> {
    let prompts = case.inputs().join("prompts");
    fs::create_dir_all(&prompts)?;
    fs::write(prompts.join("SCOUT.md"), render_scout(tab2rust))?;
    if tab2rust {
        skills::materialize("tab2rust", &case.inputs().join("agent_skills/tab2rust"))?;
        fs::write(prompts.join("TRANSLATOR.md"), TAB2RUST_TRANSLATOR)?;
    }
    if safe_wrapper {
        let rt = case.inputs().join("safe_wrapper_rt");
        fs::create_dir_all(&rt)?;
        SAFE_WRAPPER_RT.extract(&rt)?;
        fs::write(
            prompts.join("SAFE_WRAPPER_DESIGNER.md"),
            SAFE_WRAPPER_DESIGNER,
        )?;
        fs::write(
            prompts.join("SAFE_WRAPPER_TRANSLATOR.md"),
            SAFE_WRAPPER_TRANSLATOR,
        )?;
    }
    fs::create_dir_all(case.workspace().join("scout"))
}

/// Tool-specific configuration, read from `[tools.translate_agentic]` in the HARVEST config.
/// Serialized for the stage manifest, without `env` (it can hold API keys).
#[derive(Debug, Deserialize, Serialize)]
pub struct Config {
    /// Agent timeout in seconds.
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,

    /// Agent model to use. If absent, no --model flag is passed and the CLI uses its default.
    /// Claude accepts short aliases ("sonnet", "opus", "haiku") or full model IDs.
    /// When Claude is set to a "provider,model" format (e.g. "openrouter,deepseek/deepseek-v4-pro"),
    /// Claude Code is routed through claude-code-router. OpenCode expects provider/model format.
    pub model: Option<String>,

    /// Default model of the sub-agents that the main agent dispatches, in the
    /// same format as `model`. If absent, the sub-agents use the main model.
    /// Not supported with Kiro.
    pub subagent_model: Option<String>,

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

    /// Send a Safe API Wrapper Designer with the scouts: it designs the safe Rust boundary of the C
    /// library (wrapper types, API wrappers, an internal module under `forbid(unsafe_code)`), and
    /// the translation sub-agents follow the design. Off by default; the benchmark flag
    /// `--safe-wrapper` turns it on. Only the prompts that send scouts use it.
    #[serde(default)]
    pub safe_wrapper: bool,

    /// Extra environment variables to inject into the agent process.
    /// Useful for CCR provider API keys, proxy settings, etc.
    /// Defined as a TOML table under `[tools.translate_agentic.env]`.
    /// Example:
    ///   [tools.translate_agentic.env]
    ///   OPENROUTER_API_KEY = "sk-or-..."
    ///   OPENCODE_GO_API_KEY = "..."
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

fn default_tab2rust() -> bool {
    true
}

fn default_timeout_secs() -> u64 {
    agent_runner::DEFAULT_AGENT_TIMEOUT_SECS
}

impl ToolConfig for Config {
    const KEY: &'static str = "translate_agentic";
}

impl Config {
    fn validate(&self) {
        unknown_field_warning(&format!("tools.{}", Self::KEY), &self.unknown);
    }
}
