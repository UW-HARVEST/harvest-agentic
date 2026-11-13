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
const SLL_SCOUT: &str = include_str!("sll_scout.md");
const SLL_TRANSLATOR: &str = include_str!("sll_translator.md");
const SLL_DISPATCH: &str = include_str!("sll_dispatch.md");
const SLL_INVARIANT: &str = include_str!("sll_invariant.md");
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
        if let Some(report) = &config.sll_report {
            check_scout_option("sll_report", config.no_plan, agent)?;
            check_sll_option("sll_report", config.safe_wrapper)?;
            // Checked before case_dir exists: a failed run keeps its case_dir.
            if !report.is_file() {
                return Err(format!("sll_report {} is not a file", report.display()).into());
            }
        }
        let translate_prompt = load_prompt(&config, &project_spec.kind, agent);
        let no_plan = config.no_plan;

        // The agent works in case_dir/workspace/. The C source and, with scouts, the task files,
        // the skills and the list report are inputs in case_dir/inputs/ (see agent_runner::CaseDir).
        let case = CaseDir::new()?;
        let workspace = case.workspace();
        raw_source.dir.materialize(case.c_src())?;
        info!("Working directory: {}", workspace.display());

        let scouts = sends_scouts(config.no_plan, agent);
        let tab2rust = scouts && config.tab2rust;
        let safe_wrapper = scouts && config.safe_wrapper;
        let sll_report = config.sll_report.as_deref().filter(|_| scouts);
        if scouts {
            write_scout_files(&case, tab2rust, safe_wrapper, sll_report)?;
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
            fill_safe_wrapper(&fill_tab2rust(translate_prompt, tab2rust), safe_wrapper);
        let translate_prompt = fill_sll(&translate_prompt, sll_report.is_some())
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
    // The artifacts to lay out: (library, executable). A harvest-bench program takes the
    // layout of the DARPA kind with the same artifacts.
    let artifacts = match project_kind {
        ProjectKind::Library => Some((true, false)),
        ProjectKind::Executable => Some((false, true)),
        ProjectKind::HarvestBench(program) => Some((program.lib.is_some(), program.bin.is_some())),
        ProjectKind::Configurable => None,
    };
    match artifacts {
        Some((true, false)) => {
            cargo.remove_bin();
            // Keep the agent's [lib].name; without one, use the name that Cargo would derive
            // from the package name ('-' becomes '_': a lib target name cannot contain '-').
            if let Some(name) = cargo.lib_name() {
                cargo.set_lib(&name.replace('-', "_"), &["cdylib"]);
            }
            cargo.save()?;
            strip_for_lib(translated)?;
        }
        Some((false, true)) => {
            cargo.set_bin_driver();
            cargo.save()?;
        }
        Some(_) => {
            // A library and an executable. Each target is set up only when the agent produced
            // it, so that whichever artifact exists builds and is graded; the tests of the
            // other type fail as a missing artifact. Not covered: a compile error in either
            // target fails the whole build, and executables auto-discovered in src/bin/ are
            // not renamed.
            if cargo.has_lib_target()
                && let Some(name) = cargo.lib_name()
            {
                // The executable links the library, which therefore is also an rlib: a bin
                // target cannot use a cdylib-only crate. Named as for a library alone.
                cargo.set_lib(&name.replace('-', "_"), &["cdylib", "rlib"]);
            }
            if cargo.has_bin_target() {
                cargo.set_bin_driver();
            }
            cargo.save()?;
        }
        None => {
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
            // Kiro has no prompt for a program with both: it translates the library.
            ProjectKind::HarvestBench(program) if program.lib.is_none() => PROMPT_KIRO_EXECUTABLE,
            ProjectKind::HarvestBench(_) => PROMPT_KIRO_LIBRARY,
        },
    }
}

/// Whether the prompt sends scouts: the plan prompt of Claude and OpenCode.
fn sends_scouts(no_plan: bool, agent: AgentKind) -> bool {
    agent != AgentKind::Kiro && !no_plan
}

/// Refuses an option of the scout wave (`tab2rust = false`, `safe_wrapper = true`, `sll_report`)
/// where it has no effect: only the prompt that sends scouts (the plan prompt of Claude and
/// OpenCode) uses tab2rust, the Safe API Wrapper Designer and the list scout. The translate stage
/// checks the resolved config; the benchmark checks its command line before the run starts.
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

/// Refuses a list report (`option`) together with the Safe API Wrapper Designer. The list rules and
/// the boundary design give the translation sub-agents different rules for the same code, so this
/// version does not combine them. Called from the same two places as [`check_scout_option`].
pub fn check_sll_option(option: &str, safe_wrapper: bool) -> Result<(), String> {
    if !safe_wrapper {
        return Ok(());
    }
    Err(format!(
        "{option} cannot be used with the Safe API Wrapper Designer (safe_wrapper): this version \
         does not combine the two"
    ))
}

/// The scout task, with the Cargo feature rule, and the tab2rust part when tab2rust is on.
fn render_scout(tab2rust: bool) -> String {
    let scout = SCOUT
        .replace(
            "{TAB2RUST_SCOUT}",
            if tab2rust { SCOUT_TAB2RUST } else { "" },
        )
        .replace("{CARGO_FEATURES}", agent_runner::cargo_features());
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

/// Fills the list placeholders of a prompt: the list scout in the scout wave and the list rule, or
/// nothing. Each placeholder is on the line of its safe-wrapper placeholder, so that a prompt
/// without a list report has no trace of the option, not even an empty line. The two options are
/// never on together ([`check_sll_option`]), so the list scout has the item number of the designer.
fn fill_sll(prompt: &str, sll: bool) -> String {
    let (dispatch, invariant) = if sll {
        (SLL_DISPATCH, SLL_INVARIANT)
    } else {
        ("", "")
    };
    prompt
        .replace("{SLL_DISPATCH}", dispatch)
        .replace("{SLL_INVARIANT}", invariant)
}

/// Writes the scout inputs into `inputs/`: `prompts/SCOUT.md`; with tab2rust, the skill
/// (`agent_skills/tab2rust/`) and `prompts/TRANSLATOR.md`; with safe_wrapper,
/// `prompts/SAFE_WRAPPER_DESIGNER.md`, `prompts/SAFE_WRAPPER_TRANSLATOR.md` and the framework items
/// (`safe_wrapper_rt/`), from which the designer copies the parts that its design uses; with a
/// list report, its unchanged copy `extra_info/sll/report.json`, the skill (`agent_skills/sll/`),
/// `prompts/SLL_SCOUT.md` and `prompts/SLL_TRANSLATOR.md`. The scouts, the designer and the list
/// scout write their reports in `workspace/scout/`, which is created here.
fn write_scout_files(
    case: &CaseDir,
    tab2rust: bool,
    safe_wrapper: bool,
    sll_report: Option<&Path>,
) -> io::Result<()> {
    let prompts = case.inputs().join("prompts");
    fs::create_dir_all(&prompts)?;
    fs::write(prompts.join("SCOUT.md"), render_scout(tab2rust))?;
    if tab2rust {
        case.write_skill("tab2rust")?;
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
    if let Some(report) = sll_report {
        let dir = case.inputs().join("extra_info/sll");
        fs::create_dir_all(&dir)?;
        fs::copy(report, dir.join("report.json"))?;
        case.write_skill("sll")?;
        fs::write(prompts.join("SLL_SCOUT.md"), SLL_SCOUT)?;
        fs::write(prompts.join("SLL_TRANSLATOR.md"), SLL_TRANSLATOR)?;
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

    /// The report of a list analysis of the C program: its singly linked list types, and whether
    /// its functions keep the chain shape. With it, the stage gives the agents the report and the
    /// sll skill (`agent_skills/sll/`), and sends a list scout with the scouts: it decides the Rust
    /// representation of each list type, and the translation sub-agents follow it. The stage only
    /// checks that the file exists. The benchmark option `--extra-info sll=<PATH>` sets it for
    /// each program. Only the prompts that send scouts use it, and not together with
    /// `safe_wrapper`.
    pub sll_report: Option<PathBuf>,

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

#[cfg(test)]
mod tests {
    use super::*;

    /// A crate as an agent leaves it: `[package]` and the given source files.
    fn agent_crate(sources: &[&str]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"my-toy\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .unwrap();
        for source in sources {
            let path = dir.path().join(source);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, "").unwrap();
        }
        dir
    }

    /// A harvest-bench program, from its `program` block.
    fn harvest_bench(program: &str) -> ProjectKind {
        ProjectKind::HarvestBench(serde_json::from_str(program).unwrap())
    }

    const LIB_AND_BIN: &str = r#"{"lib": {}, "bin": {"name": "toy"}}"#;

    fn post_processed(dir: &Path, kind: &ProjectKind) -> toml_edit::DocumentMut {
        post_process(dir, kind).unwrap();
        fs::read_to_string(dir.join("Cargo.toml"))
            .unwrap()
            .parse()
            .unwrap()
    }

    fn crate_types(doc: &toml_edit::DocumentMut) -> Vec<&str> {
        doc["lib"]["crate-type"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t.as_str().unwrap())
            .collect()
    }

    fn bin_name(doc: &toml_edit::DocumentMut) -> Option<&str> {
        doc.get("bin")?.as_array_of_tables()?.get(0)?["name"].as_str()
    }

    /// The crate after post-processing: Cargo.toml and the relative paths of all files.
    fn result(dir: &Path) -> (String, Vec<PathBuf>) {
        fn walk(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) {
            for entry in fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    walk(root, &path, out);
                } else {
                    out.push(path.strip_prefix(root).unwrap().to_path_buf());
                }
            }
        }
        let mut files = Vec::new();
        walk(dir, dir, &mut files);
        files.sort();
        (fs::read_to_string(dir.join("Cargo.toml")).unwrap(), files)
    }

    #[test]
    fn scout_task_has_no_placeholder_left() {
        for tab2rust in [false, true] {
            assert!(!render_scout(tab2rust).contains("{CARGO_FEATURES}"));
        }
    }

    /// Step 0.2 of a plan prompt: the scout wave.
    fn scout_step(prompt: &str) -> &str {
        let start = prompt.find("### 0.2").unwrap();
        &prompt[start..prompt.find("### 0.3").unwrap()]
    }

    #[test]
    fn prompt_without_list_report_is_the_prompt_without_the_placeholders() {
        // Each placeholder is on the line of another one, so its removal leaves no empty line.
        assert!(PROMPT_TRANSLATE.contains("{SAFE_WRAPPER_DISPATCH}{SLL_DISPATCH}\n"));
        assert!(PROMPT_TRANSLATE.contains("{SLL_INVARIANT}{SAFE_WRAPPER_INVARIANT}\n"));
        let before = PROMPT_TRANSLATE
            .replace("{SLL_DISPATCH}", "")
            .replace("{SLL_INVARIANT}", "");
        for safe_wrapper in [false, true] {
            let prompt = fill_sll(&fill_safe_wrapper(PROMPT_TRANSLATE, safe_wrapper), false);
            assert_eq!(prompt, fill_safe_wrapper(&before, safe_wrapper));
        }
    }

    #[test]
    fn list_scout_is_item_5_of_the_scout_step() {
        let prompt = fill_sll(&fill_safe_wrapper(PROMPT_TRANSLATE, false), true);
        let step = scout_step(&prompt);
        assert!(step.contains("\n4. "));
        assert!(step.contains("\n5. In the same message as the scouts, also send one list scout"));
        assert_eq!(step.matches("\n5. ").count(), 1);
        assert!(prompt.contains("\n### Linked list types\n"));
        assert!(!prompt.contains("{SLL_"));
    }

    #[test]
    fn list_report_and_safe_wrapper_are_not_combined() {
        check_sll_option("sll_report", false).unwrap();
        let err = check_sll_option("sll_report", true).unwrap_err();
        assert!(err.starts_with("sll_report cannot be used with"), "{err}");
    }

    #[test]
    #[cfg_attr(miri, ignore)] // CaseDir::new has no statvfs shim
    fn list_report_adds_its_inputs_to_the_scout_files() {
        let report = tempfile::NamedTempFile::new().unwrap();
        fs::write(report.path(), "never parsed").unwrap();
        let copy = "extra_info/sll/report.json";
        let sll_files = [
            copy,
            "agent_skills/sll/SKILL.md",
            "agent_skills/sll/references/rewriting.md",
            "agent_skills/sll/scripts/sll_cards.py",
            "prompts/SLL_SCOUT.md",
            "prompts/SLL_TRANSLATOR.md",
        ];
        for sll_report in [None, Some(report.path())] {
            let case = CaseDir::new().unwrap();
            write_scout_files(&case, false, false, sll_report).unwrap();
            let inputs = case.inputs();
            for file in sll_files {
                assert_eq!(inputs.join(file).is_file(), sll_report.is_some(), "{file}");
            }
            if sll_report.is_some() {
                let copied = fs::read_to_string(inputs.join(copy)).unwrap();
                assert_eq!(copied, "never parsed");
            }
            case.finish();
        }
    }

    #[test]
    fn single_artifact_harvest_bench_programs_match_the_darpa_kinds() {
        let sources = ["src/lib.rs", "src/main.rs", "tests/it.rs"];
        for (program, darpa) in [
            (r#"{"lib": {}}"#, ProjectKind::Library),
            (r#"{"bin": {"name": "toy"}}"#, ProjectKind::Executable),
        ] {
            let (a, b) = (agent_crate(&sources), agent_crate(&sources));
            post_process(a.path(), &harvest_bench(program)).unwrap();
            post_process(b.path(), &darpa).unwrap();
            assert_eq!(result(a.path()), result(b.path()), "{program}");
        }
    }

    #[test]
    fn library_and_executable_keeps_both_targets() {
        let dir = agent_crate(&["src/lib.rs", "src/main.rs"]);
        let doc = post_processed(dir.path(), &harvest_bench(LIB_AND_BIN));
        assert_eq!(doc["lib"]["name"].as_str(), Some("my_toy"));
        assert_eq!(crate_types(&doc), ["cdylib", "rlib"]);
        assert_eq!(bin_name(&doc), Some("driver"));
        assert!(dir.path().join("src/main.rs").exists(), "main.rs stripped");
    }

    #[test]
    fn library_and_executable_without_main_adds_no_bin() {
        let dir = agent_crate(&["src/lib.rs"]);
        let doc = post_processed(dir.path(), &harvest_bench(LIB_AND_BIN));
        assert_eq!(crate_types(&doc), ["cdylib", "rlib"]);
        assert!(doc.get("bin").is_none());
    }

    #[test]
    fn library_and_executable_without_lib_adds_no_lib() {
        let dir = agent_crate(&["src/main.rs"]);
        let doc = post_processed(dir.path(), &harvest_bench(LIB_AND_BIN));
        assert!(doc.get("lib").is_none());
        assert_eq!(bin_name(&doc), Some("driver"));
    }
}
