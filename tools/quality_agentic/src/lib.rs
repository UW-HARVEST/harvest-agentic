//! Agentic quality tool: the stage after translate (or verify) that refactors a translated crate
//! toward safe and idiomatic Rust without changing its behavior.
//!
//! It consumes a [`CargoPackage`] and the C source, like verify, and produces a new
//! `CargoPackage`. The agent gets the `rust_quality` skill (`skill/` of this crate): `qscan`
//! measures the unsafe operations (as DARPA counts them) and the lints of each function and
//! suggests recipes, `regress` runs the tests that the verify stage left in the crate
//! (`verify_env/`, `tests/`) against a recorded baseline, and `qflow` runs the fixed steps. A crate
//! that never went through verify has no such tests; the agent is told so.
//!
//! The tests are the oracle of this stage, so the agent must not change them. The framework keeps
//! a copy of them, and restores and reports any change before the freeze.

use agent_runner::{AgentInvocation, AgentPhase, CaseDir, StageFact};
use full_source::{CargoPackage, RawSource};
use harvest_core::config::{ToolConfig, unknown_field_warning};
use harvest_core::tools::{RunContext, Tool};
use harvest_core::{Id, Representation};
use include_dir::{Dir, include_dir};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use tracing::{info, warn};

/// The prompt files of the stage, by their name in `inputs/prompts/quality/`: the main agent, the
/// rules for every change, and the sub-agent roles. Each role file names the rules by their path.
const STAGE_FILES: [(&str, &str); 7] = [
    ("main.md", include_str!("../prompts/main.md")),
    ("rules.md", include_str!("../prompts/rules.md")),
    ("runner.md", include_str!("../prompts/runner.md")),
    ("worker.md", include_str!("../prompts/worker.md")),
    ("scout.md", include_str!("../prompts/scout.md")),
    ("implementer.md", include_str!("../prompts/implementer.md")),
    ("integrator.md", include_str!("../prompts/integrator.md")),
];
/// The state file of the main agent, which the framework writes into the workspace as
/// `QUALITY.md` before the agent starts: the sections that the agent fills in. It holds no rules.
const QUALITY_MD: &str = include_str!("quality_md.md");
/// The section of `run.md` that tells which regression tests the crate has (see
/// [`Oracle::describe`]). The prompt of the main agent names it.
const ORACLE_FACT: &str = "Regression tests of this crate";

/// The agent skill, written into `inputs/agent_skills/rust_quality/`.
static SKILL_FILES: Dir = include_dir!("$CARGO_MANIFEST_DIR/skill");
const SKILL: &str = "rust_quality";
/// Directories of the skill that agents do not need: its own tests and Python caches.
const SKIPPED_DIRS: [&str; 2] = ["tests", "__pycache__"];
/// The test directories of the crate that this stage must not change (relative to the crate).
const ORACLE_DIRS: [&str; 2] = ["verify_env", "tests"];

pub struct QualityAgentic;

impl Tool for QualityAgentic {
    fn name(&self) -> &'static str {
        "quality_agentic"
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

        let case = CaseDir::new()?;
        let workspace = case.workspace();
        cargo_package.dir.materialize(&workspace)?;
        raw_source.dir.materialize(case.c_src())?;
        info!("Working directory: {}", workspace.display());

        let oracle = Oracle::capture(&workspace, &case.root().join("quality_oracle"))
            .map_err(|e| format!("saving the regression tests: {e}"))?;
        let prompt = agent_runner::main_prompt(AgentPhase::Quality);
        let facts = [StageFact {
            heading: ORACLE_FACT,
            text: oracle.describe(),
        }];

        let rust_toolchain_context = agent_runner::detect_rust_toolchain_context(
            &context.config.input,
            config.test_corpus_root.as_deref(),
        )?;
        let prepared = agent_runner::prepare_agent(AgentInvocation {
            phase: AgentPhase::Quality,
            agent,
            case: &case,
            prompt: &prompt,
            timeout_secs: config.timeout_secs,
            model: config.model.as_deref(),
            subagent_model: config.subagent_model.as_deref(),
            no_plan: false,
            teamclaude: context.config.teamclaude,
            extra_env: &config.env,
            output_log_path: config.output_log_path.as_deref(),
            rust_toolchain: &rust_toolchain_context,
            additional_prompts: &[],
            stage_facts: &facts,
        })?;
        write_inputs(&case).map_err(|e| format!("writing the inputs: {e}"))?;
        fs::write(workspace.join("QUALITY.md"), QUALITY_MD)
            .map_err(|e| format!("writing QUALITY.md: {e}"))?;
        let reference_guard = case.lock_inputs()?;
        prepared.run()?;
        info!("Quality stage complete");
        for report in ["QUALITY.md", "QUALITY_REPORT.md"] {
            if !workspace.join(report).exists() {
                warn!("Agent did not produce {report}, although the prompt asks for it");
            }
        }
        let restored = oracle
            .restore(&workspace)
            .map_err(|e| format!("restoring the regression tests: {e}"))?;
        for path in restored {
            warn!(
                "The quality agent changed the regression tests ({path}); the input version is restored"
            );
        }
        remove_harness_builds(&workspace)
            .map_err(|e| format!("removing verify_env/build-*: {e}"))?;
        let dir = case.freeze(&reference_guard, config.reference_audit_dir.as_deref())?;
        Ok(Box::new(CargoPackage { dir }))
    }
}

/// Writes the inputs of the stage: the skill with the verify stage's run scripts in its `runner/`,
/// and the prompt files of the stage in `prompts/quality/`.
fn write_inputs(case: &CaseDir) -> io::Result<()> {
    let skill = case.inputs().join("agent_skills").join(SKILL);
    write_dir(&SKILL_FILES, &skill)?;
    let runner = skill.join("runner");
    fs::create_dir_all(&runner)?;
    for (name, text) in verify_fix_agentic::RUNNER_SCRIPTS {
        fs::write(runner.join(name), text)?;
    }
    case.write_prompts(AgentPhase::Quality.prompt_dir(), &STAGE_FILES)
}

/// Writes the files of `dir` below `dest`, without the skipped directories.
fn write_dir(dir: &Dir, dest: &Path) -> io::Result<()> {
    fs::create_dir_all(dest)?;
    for file in dir.files() {
        let out = dest.join(file.path().file_name().expect("a file has a name"));
        fs::write(out, file.contents())?;
    }
    for sub in dir.dirs() {
        let name = sub.path().file_name().expect("a directory has a name");
        if !SKIPPED_DIRS.iter().any(|s| name == *s) {
            write_dir(sub, &dest.join(name))?;
        }
    }
    Ok(())
}

/// The regression tests of the crate, as the stage received them.
struct Oracle {
    backup: PathBuf,
    /// Relative path → contents, of every protected file.
    files: BTreeMap<PathBuf, Vec<u8>>,
    has_harness: bool,
    has_fuzz: bool,
    has_cargo_tests: bool,
}

impl Oracle {
    fn capture(workspace: &Path, backup: &Path) -> io::Result<Self> {
        let files = protected_files(workspace)?;
        for (rel, data) in &files {
            let out = backup.join(rel);
            if let Some(parent) = out.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(out, data)?;
        }
        let cmake = workspace.join("verify_env/CMakeLists.txt");
        let has_harness = cmake.is_file();
        let has_fuzz = has_harness
            && fs::read_to_string(&cmake)?
                .to_ascii_lowercase()
                .contains("fuzztest");
        Ok(Self {
            backup: backup.to_path_buf(),
            files,
            has_harness,
            has_fuzz,
            has_cargo_tests: workspace.join("tests").is_dir(),
        })
    }

    /// The fact for `run.md`: which regression tests this crate has. `prompts/main.md` tells
    /// what the main agent does when there are none.
    fn describe(&self) -> String {
        let mut lines = Vec::new();
        if self.has_harness {
            lines.push(
                "- `verify_env/`: the GoogleTest harness of the verify stage. It compares the C \
                 reference with the Rust library, test by test."
                    .to_owned(),
            );
        }
        if self.has_fuzz {
            lines.push(
                "- FuzzTest properties in `verify_env/`, with the inputs that the verify stage's \
                 campaigns saved (`verify_env/fuzz-artifacts/`). `regress.py` replays them and runs \
                 short new campaigns."
                    .to_owned(),
            );
        }
        if self.has_cargo_tests {
            lines.push("- `tests/`: Rust integration tests (`cargo test`).".to_owned());
        }
        if lines.is_empty() {
            "- NONE. This crate did not go through the verify stage and has no regression tests."
                .to_owned()
        } else {
            lines.join("\n")
        }
    }

    /// Restores the protected files that the agent changed, added or removed. Returns their paths.
    fn restore(&self, workspace: &Path) -> io::Result<Vec<String>> {
        let now = protected_files(workspace)?;
        let mut changed = Vec::new();
        for (rel, data) in &now {
            if self.files.get(rel) != Some(data) {
                changed.push(rel.clone());
            }
        }
        for rel in self.files.keys() {
            if !now.contains_key(rel) {
                changed.push(rel.clone());
            }
        }
        for rel in &changed {
            let path = workspace.join(rel);
            match self.files.get(rel) {
                Some(_) => {
                    if let Some(parent) = path.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    fs::copy(self.backup.join(rel), &path)?;
                }
                None => fs::remove_file(&path)?,
            }
        }
        changed.sort();
        Ok(changed.iter().map(|p| p.display().to_string()).collect())
    }
}

/// Removes `verify_env/build-*`: an agent that ran the verify stage's build scripts directly left
/// the fetched GoogleTest/FuzzTest sources and the binaries there. The snapshot keeps the sources.
fn remove_harness_builds(workspace: &Path) -> io::Result<()> {
    let dir = workspace.join("verify_env");
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(&dir)? {
        let entry = entry?;
        let name = entry.file_name();
        if !name.to_string_lossy().starts_with("build-") {
            continue;
        }
        if entry.file_type()?.is_dir() {
            fs::remove_dir_all(entry.path())?;
        } else {
            fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

/// Every file under the oracle directories, except build output and fuzz campaign data.
fn protected_files(workspace: &Path) -> io::Result<BTreeMap<PathBuf, Vec<u8>>> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) -> io::Result<()> {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            let name = entry.file_name().to_string_lossy().into_owned();
            // Build output and fuzz campaign data are not tests. A FuzzTest build installs its
            // ANTLR runtime into `verify_env/dist`.
            let skipped = kind.is_dir()
                && (name.starts_with("build-")
                    || name == "fuzz-artifacts"
                    || (name == "dist" && dir == root.join("verify_env")));
            if kind.is_symlink() || skipped {
                continue;
            }
            let path = entry.path();
            if kind.is_dir() {
                walk(root, &path, out)?;
            } else {
                let rel = path.strip_prefix(root).expect("walk stays below root");
                out.insert(rel.to_path_buf(), fs::read(&path)?);
            }
        }
        Ok(())
    }
    let mut out = BTreeMap::new();
    for dir in ORACLE_DIRS {
        let path = workspace.join(dir);
        if path.is_dir() {
            walk(workspace, &path, &mut out)?;
        }
    }
    Ok(out)
}

/// Tool-specific configuration, read from `[tools.quality_agentic]` in the HARVEST config.
#[derive(Debug, Deserialize, Serialize)]
pub struct Config {
    /// Agent timeout in seconds.
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,

    /// Agent model to use. If absent, no --model flag is passed and the CLI uses its default.
    pub model: Option<String>,

    /// Default model of the sub-agents that the main agent dispatches, in the
    /// same format as `model`. If absent, the sub-agents use the main model.
    pub subagent_model: Option<String>,

    /// Extra environment variables to inject into the agent process.
    #[serde(default, skip_serializing)]
    pub env: HashMap<String, String>,

    /// Destination path for the benchmark's output.log file. Injected by the benchmark.
    pub output_log_path: Option<PathBuf>,

    /// Directory that records the read-only inputs the agent changed, one file each
    /// (`.harvest/reference_modified/<stage>/`). Injected by the benchmark.
    pub reference_audit_dir: Option<PathBuf>,

    /// The Test-Corpus / cando2 checkout defining the Rust toolchain contract. Injected from the
    /// stage manifest when resuming from a snapshot.
    pub test_corpus_root: Option<PathBuf>,

    #[serde(flatten)]
    unknown: HashMap<String, serde_json::Value>,
}

fn default_timeout_secs() -> u64 {
    agent_runner::DEFAULT_AGENT_TIMEOUT_SECS
}

impl ToolConfig for Config {
    const KEY: &'static str = "quality_agentic";
}

impl Config {
    fn validate(&self) {
        unknown_field_warning(&format!("tools.{}", Self::KEY), &self.unknown);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, rel: &str, text: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn oracle_restores_changed_added_and_removed_tests() {
        let ws = tempfile::tempdir().unwrap();
        let backup = tempfile::tempdir().unwrap();
        let root = ws.path();
        write(root, "verify_env/CMakeLists.txt", "find_package(fuzztest)");
        write(root, "verify_env/verification_tests.cc", "TEST(A, B) {}");
        write(root, "verify_env/build-test/junk.o", "o");
        write(root, "verify_env/fuzz-artifacts/A.B.x/run.log", "log");
        write(root, "verify_env/dist/libantlr4-runtime.a", "a");
        write(root, "tests/it.rs", "#[test] fn t() {}");
        write(root, "src/lib.rs", "fn f() {}");
        let oracle = Oracle::capture(root, &backup.path().join("b")).unwrap();
        assert!(oracle.has_harness && oracle.has_fuzz && oracle.has_cargo_tests);

        write(
            root,
            "verify_env/verification_tests.cc",
            "TEST(A, B) { GTEST_SKIP(); }",
        );
        write(root, "tests/new.rs", "");
        fs::remove_file(root.join("tests/it.rs")).unwrap();
        write(root, "verify_env/build-test/other.o", "o");
        write(root, "verify_env/dist/libantlr4-runtime.a", "rebuilt");
        write(root, "src/lib.rs", "fn g() {}");
        let changed = oracle.restore(root).unwrap();
        assert_eq!(
            changed,
            [
                "tests/it.rs",
                "tests/new.rs",
                "verify_env/verification_tests.cc"
            ]
        );
        assert_eq!(
            fs::read_to_string(root.join("verify_env/verification_tests.cc")).unwrap(),
            "TEST(A, B) {}"
        );
        assert!(root.join("tests/it.rs").is_file() && !root.join("tests/new.rs").exists());
        // Build output and the crate itself are not the oracle's business.
        assert!(root.join("verify_env/build-test/other.o").is_file());
        assert_eq!(
            fs::read_to_string(root.join("src/lib.rs")).unwrap(),
            "fn g() {}"
        );
        assert!(oracle.restore(root).unwrap().is_empty());
        remove_harness_builds(root).unwrap();
        assert!(!root.join("verify_env/build-test").exists());
        assert!(
            root.join("verify_env/fuzz-artifacts/A.B.x/run.log")
                .is_file()
        );
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn crate_without_tests_is_described_as_unguarded() {
        let ws = tempfile::tempdir().unwrap();
        let backup = tempfile::tempdir().unwrap();
        write(ws.path(), "src/lib.rs", "");
        let oracle = Oracle::capture(ws.path(), backup.path()).unwrap();
        assert!(oracle.describe().starts_with("- NONE."));
    }

    /// Runs `check` on a case directory that holds the inputs of a run, as the tool writes them.
    fn with_inputs(check: impl Fn(&CaseDir)) {
        let case = CaseDir::new().unwrap();
        let toolchain = agent_runner::RustToolchainContext::for_version("1.99.0");
        let env = HashMap::new();
        let facts = [StageFact {
            heading: ORACLE_FACT,
            text: "- NONE.".to_owned(),
        }];
        agent_runner::prepare_agent(AgentInvocation {
            phase: AgentPhase::Quality,
            agent: harvest_core::config::AgentKind::Claude,
            case: &case,
            prompt: "",
            timeout_secs: 1,
            model: None,
            subagent_model: None,
            no_plan: false,
            teamclaude: true,
            extra_env: &env,
            output_log_path: None,
            rust_toolchain: &toolchain,
            additional_prompts: &[],
            stage_facts: &facts,
        })
        .unwrap();
        write_inputs(&case).unwrap();
        fs::write(case.workspace().join("QUALITY.md"), QUALITY_MD).unwrap();
        let prompt = agent_runner::main_prompt(AgentPhase::Quality);
        agent_runner::prompt_check::dump(&case, "quality", &prompt, &["QUALITY.md"]).unwrap();
        check(&case);
        case.finish();
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn inputs_hold_the_skill_and_its_runner() {
        with_inputs(|case| {
            let skill = case.inputs().join("agent_skills/rust_quality");
            for file in [
                "SKILL.md",
                "RECIPES.md",
                "GLOBAL.md",
                "CURSORS.md",
                "scripts/qscan.py",
                "scripts/qscan_lib/report.py",
                "scripts/regress.py",
                "scripts/qflow.py",
                "runner/run_tests.sh",
                "runner/run_fuzz.sh",
                "runner/memory_scope.sh",
            ] {
                assert!(skill.join(file).is_file(), "{file} missing");
            }
            assert!(!skill.join("tests").exists());
        });
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn prompt_files_are_consistent() {
        with_inputs(|case| {
            let inputs = case.inputs();
            assert_eq!(
                agent_runner::prompt_check::problems(&inputs).unwrap(),
                [""; 0]
            );
            let files = agent_runner::prompt_check::load(&inputs).unwrap();
            let must_read = |entry: &str| -> Vec<String> {
                agent_runner::prompt_check::must_read(&files, entry)
                    .into_iter()
                    .filter(|file| file != entry)
                    .collect()
            };
            assert_eq!(
                must_read("quality/main.md"),
                [
                    "common.md",
                    "git_main.md",
                    "main_agent.md",
                    "quality/rules.md",
                    "run.md",
                    "runtime.md"
                ]
            );
            let main = &files["quality/main.md"];
            for (name, _) in STAGE_FILES {
                if matches!(name, "main.md" | "rules.md") {
                    continue;
                }
                // Every role reads the rules of a sub-agent and the rules for every change.
                assert_eq!(
                    must_read(&format!("quality/{name}")),
                    [
                        "common.md",
                        "git_subagent.md",
                        "quality/rules.md",
                        "run.md",
                        "runtime.md",
                        "subagent.md"
                    ],
                    "{name}"
                );
                // The main prompt names every role file.
                assert!(
                    main.contains(&format!("inputs/prompts/quality/{name}")),
                    "{name} not in the main prompt"
                );
            }
            // The main prompt names the section of run.md that the framework writes.
            assert!(main.contains(ORACLE_FACT));
            assert!(files["run.md"].contains(&format!("## {ORACLE_FACT}\n\n- NONE.")));
        });
    }

    #[test]
    fn plan_file_holds_state_and_no_rules() {
        for section in ["## Baseline", "## Local phase", "## Global phase", "## Log"] {
            assert!(QUALITY_MD.contains(section), "{section}");
        }
        assert!(!QUALITY_MD.contains("Invariants") && !QUALITY_MD.contains("inputs/prompts"));
        let prompt = agent_runner::main_prompt(AgentPhase::Quality);
        assert_eq!(prompt.lines().count(), 2);
        assert!(prompt.contains("inputs/prompts/quality/main.md"));
    }
}
