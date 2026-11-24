//! Agentic translation tool for HARVEST.
//!
//! Translates a C project (as a [`RawSource`](full_source::RawSource)) into a Rust Cargo project
//! by invoking an external agent. After the agent finishes, the generated `Cargo.toml` is
//! post-processed to satisfy downstream tool expectations.
//! The translated project is stored in the IR as a [`CargoPackage`](full_source::CargoPackage).

use agent_runner::{AdditionalPrompt, AgentInvocation, AgentPhase, CaseDir};
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

/// The whole task of the main agent when the stage runs without a plan file and without scouts.
const PROMPT_TRANSLATE_NO_PLAN: &str = include_str!("prompt_translate_no_plan.md");

/// The state file of the main agent, which the framework writes into the workspace as `PLAN.md`
/// before the agent starts: the sections that the agent fills in. It holds no rules.
const PLAN_MD: &str = include_str!("plan_md.md");

/// The prompt files of the stage, by their name in `inputs/prompts/translate/`. Each rule has
/// one copy; a file names the files that its reader needs.
///
/// The rules of the translation, for every prompt of the stage.
const RULE_FILES: [(&str, &str); 2] = [
    (
        "translation_rules.md",
        include_str!("../prompts/translation_rules.md"),
    ),
    (
        "completion_gate.md",
        include_str!("../prompts/completion_gate.md"),
    ),
];
/// The main agent and the sub-agent roles of the plan prompt.
const PLAN_FILES: [(&str, &str); 4] = [
    ("main.md", include_str!("../prompts/main.md")),
    ("scout.md", include_str!("../prompts/scout.md")),
    ("translator.md", include_str!("../prompts/translator.md")),
    ("reviewer.md", include_str!("../prompts/reviewer.md")),
];
/// With tab2rust: the part of the main agent, an additional prompt file of the run.
const TAB2RUST_MAIN: (&str, &str, &str) = (
    "tab2rust_main.md",
    "the C data tables, which a tool generates (tab2rust)",
    include_str!("../prompts/tab2rust_main.md"),
);
/// With tab2rust: the parts of scout 1 and of a translator.
const TAB2RUST_FILES: [(&str, &str); 2] = [
    (
        "tab2rust_scout.md",
        include_str!("../prompts/tab2rust_scout.md"),
    ),
    ("tab2rust.md", include_str!("../prompts/tab2rust.md")),
];
/// With safe_wrapper: the part of the main agent, an additional prompt file of the run.
const SAFE_WRAPPER_MAIN: (&str, &str, &str) = (
    "safe_wrapper_main.md",
    "the safe API wrapper design of this run",
    include_str!("../prompts/safe_wrapper_main.md"),
);
/// With safe_wrapper: the designer role and the part of a translator.
const SAFE_WRAPPER_FILES: [(&str, &str); 2] = [
    (
        "safe_wrapper_designer.md",
        include_str!("../prompts/safe_wrapper_designer.md"),
    ),
    (
        "safe_wrapper_translator.md",
        include_str!("../prompts/safe_wrapper_translator.md"),
    ),
];
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
        let no_plan = config.no_plan;
        if !config.tab2rust {
            check_scout_option("tab2rust = false", no_plan)?;
        }
        if config.safe_wrapper {
            check_scout_option("safe_wrapper = true", no_plan)?;
        }

        // The agent works in case_dir/workspace/. The C source, the prompt files and, with the plan
        // prompt, the tab2rust skill are inputs in case_dir/inputs/ (see agent_runner::CaseDir).
        let case = CaseDir::new()?;
        let workspace = case.workspace();
        raw_source.dir.materialize(case.c_src())?;
        info!("Working directory: {}", workspace.display());

        // The plan prompt sends scouts; the no-plan prompt does not.
        let scouts = !no_plan;
        let tab2rust = scouts && config.tab2rust;
        // check_scout_option has refused safe_wrapper with no_plan.
        let safe_wrapper = config.safe_wrapper;

        let rust_toolchain_context = agent_runner::detect_rust_toolchain_context(
            &context.config.input,
            config.test_corpus_root.as_deref(),
        )?;
        let workflow_hint = if config.workflow && agent == AgentKind::Claude {
            "You must use a workflow.\n\n"
        } else {
            ""
        };
        let translate_prompt = if no_plan {
            PROMPT_TRANSLATE_NO_PLAN.replace("{WORKFLOW_HINT}", workflow_hint)
        } else {
            agent_runner::main_prompt(AgentPhase::Translate)
        };

        let additional = additional_prompts(tab2rust, safe_wrapper, config.tool_wishlist);
        let prepared = agent_runner::prepare_agent(AgentInvocation {
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
            additional_prompts: &additional,
            stage_facts: &[],
        })?;
        write_stage_files(&case, scouts, tab2rust, safe_wrapper)?;
        let reference_guard = case.lock_inputs()?;
        prepared.run()?;

        // The framework wrote PLAN.md for the plan prompt; the no-plan prompt has none.
        if !no_plan && !workspace.join("PLAN.md").exists() {
            warn!("The agent removed PLAN.md, which the framework wrote");
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
            // The agent decides the targets of a configurable project from its C source.
            // Only ensure workspace is present (already added above); leave the rest intact.
            cargo.save()?;
        }
    }
    Ok(())
}

/// Refuses an option of the scout wave (`tab2rust = false`, `safe_wrapper = true`) where it has
/// no effect: only the prompt that sends scouts (the plan prompt) uses tab2rust and the Safe API
/// Wrapper Designer.
fn check_scout_option(option: &str, no_plan: bool) -> Result<(), String> {
    if no_plan {
        return Err(format!(
            "{option} has no effect with no_plan: only the plan prompt sends scouts"
        ));
    }
    Ok(())
}

/// The prompt files that the options of the run add for the main agent. `agent_runner` writes
/// them and lists them in `run.md`. Each file of an option names the files of the option for the
/// sub-agents.
fn additional_prompts(
    tab2rust: bool,
    safe_wrapper: bool,
    tool_wishlist: bool,
) -> Vec<AdditionalPrompt> {
    let stage = AgentPhase::Translate.prompt_dir();
    let options = [(tab2rust, TAB2RUST_MAIN), (safe_wrapper, SAFE_WRAPPER_MAIN)];
    let mut files: Vec<AdditionalPrompt> = options
        .into_iter()
        .filter(|(on, _)| *on)
        .map(|(_, (name, purpose, text))| AdditionalPrompt {
            path: format!("{stage}/{name}"),
            purpose,
            text,
        })
        .collect();
    files.extend(tool_wishlist.then(agent_runner::tool_wishlist));
    files
}

/// Writes the files of the stage. Into `inputs/prompts/translate/`: the rules of the
/// translation. With the plan prompt (`plan`) also: the prompt files of the main agent and of the
/// roles; with tab2rust, its prompt files for the sub-agents and the skill
/// (`agent_skills/tab2rust/`); with safe_wrapper, its prompt files for the sub-agents and the
/// framework items (`safe_wrapper_rt/`), from which the designer copies the parts that its design
/// uses. `agent_runner` writes the files of the options for the main agent
/// ([`additional_prompts`]). Into the workspace: `PLAN.md`, the state file
/// of the main agent, and `scout/`, where the scouts and the designer write their reports.
fn write_stage_files(
    case: &CaseDir,
    plan: bool,
    tab2rust: bool,
    safe_wrapper: bool,
) -> io::Result<()> {
    let stage = AgentPhase::Translate.prompt_dir();
    case.write_prompts(stage, &RULE_FILES)?;
    if !plan {
        return Ok(());
    }
    case.write_prompts(stage, &PLAN_FILES)?;
    if tab2rust {
        case.write_skill("tab2rust")?;
        case.write_prompts(stage, &TAB2RUST_FILES)?;
    }
    if safe_wrapper {
        let rt = case.inputs().join("safe_wrapper_rt");
        fs::create_dir_all(&rt)?;
        SAFE_WRAPPER_RT.extract(&rt)?;
        case.write_prompts(stage, &SAFE_WRAPPER_FILES)?;
    }
    fs::write(case.workspace().join("PLAN.md"), PLAN_MD)?;
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
    pub subagent_model: Option<String>,

    /// If true, use the minimal baseline prompt: no plan file, no scouts and no sub-agent
    /// guidance. For controlled experiments that measure the effect of the plan prompt.
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

    /// Experimental: ask the agent to record the analysis tools it wanted in
    /// `tool_wishlist.json`. Off by default; the benchmark flag `--tool-wishlist` turns it on.
    #[serde(default)]
    pub tool_wishlist: bool,

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

    /// Runs `check` on a case directory that holds the files of a run with these options, as the
    /// tool writes them.
    fn with_stage_files(
        agent: AgentKind,
        no_plan: bool,
        tab2rust: bool,
        safe_wrapper: bool,
        check: impl Fn(&CaseDir),
    ) {
        let case = CaseDir::new().unwrap();
        let toolchain = agent_runner::RustToolchainContext::for_version("1.99.0");
        let env = HashMap::new();
        agent_runner::prepare_agent(AgentInvocation {
            phase: AgentPhase::Translate,
            agent,
            case: &case,
            prompt: "",
            timeout_secs: 1,
            model: None,
            subagent_model: None,
            no_plan,
            teamclaude: true,
            extra_env: &env,
            output_log_path: None,
            rust_toolchain: &toolchain,
            additional_prompts: &additional_prompts(tab2rust, safe_wrapper, true),
            stage_facts: &[],
        })
        .unwrap();
        write_stage_files(&case, !no_plan, tab2rust, safe_wrapper).unwrap();
        let prompt = if no_plan {
            PROMPT_TRANSLATE_NO_PLAN.to_owned()
        } else {
            agent_runner::main_prompt(AgentPhase::Translate)
        };
        let name = format!(
            "translate-{agent:?}-plan_{}-tab2rust_{tab2rust}-safe_wrapper_{safe_wrapper}",
            !no_plan
        );
        agent_runner::prompt_check::dump(&case, &name, &prompt, &["PLAN.md"]).unwrap();
        check(&case);
        case.finish();
    }

    /// The files that the reader of `entry` must read, without `entry`.
    fn must_read(files: &agent_runner::prompt_check::PromptFiles, entry: &str) -> Vec<String> {
        agent_runner::prompt_check::must_read(files, entry)
            .into_iter()
            .filter(|file| file != entry)
            .collect()
    }

    /// What every agent reads, and what a main agent and a sub-agent add to it.
    const EVERY_AGENT: [&str; 3] = ["common.md", "run.md", "runtime.md"];
    const MAIN_AGENT: [&str; 2] = ["git_main.md", "main_agent.md"];
    const SUBAGENT: [&str; 2] = ["git_subagent.md", "subagent.md"];

    fn sorted(groups: &[&[&str]]) -> Vec<String> {
        let mut files: Vec<String> = groups.concat().iter().map(|f| f.to_string()).collect();
        files.sort();
        files
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn prompt_files_are_consistent_for_every_option() {
        for agent in [AgentKind::Claude, AgentKind::OpenCode] {
            for (tab2rust, safe_wrapper) in
                [(false, false), (true, false), (false, true), (true, true)]
            {
                with_stage_files(agent, false, tab2rust, safe_wrapper, |case| {
                    let inputs = case.inputs();
                    assert_eq!(
                        agent_runner::prompt_check::problems(&inputs).unwrap(),
                        [""; 0],
                        "tab2rust={tab2rust} safe_wrapper={safe_wrapper}"
                    );
                    let files = agent_runner::prompt_check::load(&inputs).unwrap();
                    // An option that is off leaves no word in the files of the run.
                    for (on, words) in [
                        (tab2rust, &["tab2rust"][..]),
                        (safe_wrapper, &["safe_wrapper", "safe api", "wrapper"][..]),
                    ] {
                        for (name, text) in files.iter().filter(|_| !on) {
                            let text = text.to_ascii_lowercase();
                            for word in words {
                                assert!(!text.contains(word), "{name} reveals {word}");
                            }
                        }
                    }
                    // The options of the run are files of their own, and run.md lists the one
                    // of the main agent.
                    let run = &files["run.md"];
                    for (on, option) in [(tab2rust, "tab2rust"), (safe_wrapper, "safe_wrapper")] {
                        let main = format!("translate/{option}_main.md");
                        assert_eq!(files.contains_key(&main), on, "{main}");
                        assert_eq!(run.contains(&format!("inputs/prompts/{main}")), on);
                        // The file of the main agent names the files of the sub-agents.
                        let named: Vec<&String> = files
                            .keys()
                            .filter(|name| name.starts_with(&format!("translate/{option}")))
                            .filter(|name| **name != main)
                            .collect();
                        assert_eq!(named.len(), if on { 2 } else { 0 }, "{option}");
                        for name in named {
                            assert!(files[&main].contains(&format!("inputs/prompts/{name}")));
                        }
                    }
                });
            }
        }
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn each_reader_must_read_the_files_of_its_rules() {
        with_stage_files(AgentKind::Claude, false, true, true, |case| {
            let files = agent_runner::prompt_check::load(&case.inputs()).unwrap();
            let rules = "translate/translation_rules.md";
            let gate = "translate/completion_gate.md";
            let features = "cargo_features.md";
            for (entry, more) in [
                ("translate/main.md", &[rules, gate, features][..]),
                ("translate/scout.md", &[features][..]),
                ("translate/translator.md", &[rules, features][..]),
                ("translate/reviewer.md", &[rules, gate, features][..]),
                ("translate/safe_wrapper_designer.md", &[][..]),
            ] {
                let role: &[&str] = if entry == "translate/main.md" {
                    &MAIN_AGENT
                } else {
                    &SUBAGENT
                };
                assert_eq!(
                    must_read(&files, entry),
                    sorted(&[&EVERY_AGENT, role, more]),
                    "{entry}"
                );
            }
            // The parts that an option adds to a role name no files of their own to read first.
            for part in [
                "translate/tab2rust_main.md",
                "translate/tab2rust_scout.md",
                "translate/tab2rust.md",
                "translate/safe_wrapper_main.md",
                "translate/safe_wrapper_translator.md",
            ] {
                assert_eq!(must_read(&files, part), [""; 0], "{part}");
            }
            // The main prompt names the role files.
            for role in ["scout.md", "translator.md", "reviewer.md"] {
                assert!(
                    files["translate/main.md"]
                        .contains(&format!("inputs/prompts/translate/{role}"))
                );
            }
        });
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn plan_file_holds_state_and_no_rules() {
        with_stage_files(AgentKind::Claude, false, true, true, |case| {
            let plan = fs::read_to_string(case.workspace().join("PLAN.md")).unwrap();
            for section in [
                "## Self-assessment",
                "## Codebase summary",
                "## Translation subtasks",
                "## Notes for future-me",
            ] {
                assert!(plan.contains(section), "{section}");
            }
            assert!(!plan.contains("Invariants") && !plan.contains("inputs/prompts"));
            assert!(case.workspace().join("scout").is_dir());
        });
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn no_plan_prompt_names_only_files_that_its_run_has() {
        // The workflow hint is the one text that the tool puts into the prompt.
        assert_eq!(
            agent_runner::prompt_check::placeholders(PROMPT_TRANSLATE_NO_PLAN),
            ["{WORKFLOW_HINT}"]
        );
        with_stage_files(AgentKind::Claude, true, false, false, |case| {
            let inputs = case.inputs();
            assert_eq!(
                agent_runner::prompt_check::problems(&inputs).unwrap(),
                [""; 0]
            );
            let files = agent_runner::prompt_check::load(&inputs).unwrap();
            let named = agent_runner::prompt_check::references(PROMPT_TRANSLATE_NO_PLAN);
            assert!(named.contains(&"common.md") && named.contains(&"git_main.md"));
            for file in named {
                assert!(files.contains_key(file), "{file}");
            }
            // No plan file, no scouts, no roles.
            assert!(!files.contains_key("translate/main.md"));
            assert!(!case.workspace().join("PLAN.md").exists());
        });
    }

    #[test]
    fn main_prompt_only_names_the_prompt_file_of_the_stage() {
        let prompt = agent_runner::main_prompt(AgentPhase::Translate);
        assert_eq!(prompt.lines().count(), 2);
        assert!(prompt.contains("inputs/prompts/translate/main.md"));
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
