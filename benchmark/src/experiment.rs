//! Declared experiment sets.
//!
//! An experiment set is a committed TOML file listing the runs that *should*
//! exist. `--experiments FILE` executes them; `--status` reports which are
//! done. This exists because a run's identity otherwise lives only in a
//! hand-typed output directory name, which means nothing can enumerate the
//! intended set: a run that was never started is indistinguishable from one
//! that is missing or that failed, and aggregating results means guessing
//! directory names.
//!
//! Design notes worth knowing before changing this file:
//!
//! * **Every knob is lowered back into argv and re-parsed by clap.** A declared
//!   run becomes a `Vec<String>` handed to [`Args::try_parse_from`], then to
//!   [`Args::validate_stages`]. That is deliberate: clap and `validate_stages`
//!   already encode every `requires` / `conflicts_with` / stage-dependent rule,
//!   so re-parsing inherits all of them for free and a declared run can never
//!   express a combination the command line would reject. Do not reimplement
//!   those rules here.
//!
//! * **Enum-valued knobs are typed `String`, not the enum.** `Stage` and
//!   `AgentKind` only `Deserialize` from their full lowercase names, while the
//!   documented `t`/`v`/`c` and `oc` aliases are `FromStr`-only. Typing these
//!   fields as enums would silently reject `agent = "oc"` and create a second
//!   value vocabulary that desynchronizes from the CLI on every new alias.
//!   Values pass through verbatim and clap owns the vocabulary.
//!
//! * **Completion is decided by this module's own per-program records, never by
//!   the presence of a stage manifest.** `write_stage_manifest` is called after
//!   translation succeeds but *before* build and grading (deliberately, so a
//!   snapshot with a broken build can still be resumed from). Treating a
//!   manifest as "done" would permanently skip any program that crashed during
//!   build, verify, or grading.

use crate::cli::Args;
use crate::error::HarvestResult;
use crate::stats::ProgramEvalStats;
use crate::{ProgramRun, RunOptions};
use clap::Parser;
use harvest_core::utils::get_version;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Schema version of the experiment-set file this build understands.
const SCHEMA_VERSION: u32 = 1;

/// Directory, inside a run's output root, holding everything this module owns.
const SWEEP_META_DIR: &str = ".harvest-sweep";

/// Per-program progress records live in `<run_root>/.harvest-sweep/programs/`.
const PROGRAMS_SUBDIR: &str = "programs";

/// The run's receipt: what was declared, and the exact argv it became.
const RECEIPT_FILE: &str = "receipt.json";

// ── Manifest schema ────────────────────────────────────────────────────

/// A committed experiment set: the runs that should exist.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExperimentSet {
    /// Bumped on incompatible schema changes.
    pub schema_version: u32,

    /// Root of the bench checkout, whose children are test-case directories.
    /// Relative paths resolve against the manifest file, never the process CWD.
    pub bench_root: PathBuf,

    /// Where run outputs go. A run's output root is always
    /// `<results_root>/<id>`; there is no hand-chosen path.
    pub results_root: PathBuf,

    /// Knobs applied to every run unless the run overrides them.
    #[serde(default)]
    pub defaults: Knobs,

    /// The declared runs, in file order.
    #[serde(default, rename = "run")]
    pub runs: Vec<RunSpec>,
}

/// Knobs shared between `[defaults]` and each `[[run]]`. Every field is
/// optional so a run can inherit or override individually.
///
/// `String`-typed enums are intentional; see the module docs.
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Knobs {
    /// Pipeline stages, in pipeline order. Omitted or empty means a
    /// non-agentic run (the `one_shot` / `modular` baselines).
    pub stages: Option<Vec<String>>,
    /// Use modular translation instead of all-at-once. Non-agentic only.
    pub modular: Option<bool>,
    pub agent: Option<String>,
    pub model: Option<String>,
    pub no_plan: Option<bool>,
    pub no_plan_file: Option<bool>,
    pub workflow: Option<bool>,
    pub agent_tools: Option<bool>,
    pub verify_harness: Option<String>,
    pub fuzz: Option<bool>,
    /// Grade against this bench checkout's *current* suite instead of the one
    /// the snapshot carries. Resolved against the manifest file.
    pub test_case: Option<PathBuf>,
    pub test_harness: Option<String>,
    pub timeout: Option<u64>,
    /// Verbatim `-c NAME=VALUE` overrides. `[defaults]` entries come first,
    /// then the run's own, matching how the CLI resolves repeated `-c` flags.
    #[serde(default)]
    pub config: Vec<String>,
    /// Escape hatch for flags this schema does not name yet. Appended verbatim
    /// and still fully validated by clap, so a new `Args` field is usable from
    /// a committed set without changing this file first.
    #[serde(default)]
    pub extra_args: Vec<String>,
}

/// One declared run.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunSpec {
    /// Stable identity. Also the output directory name, so it is restricted to
    /// characters that are safe in a path and must be unique in the set.
    pub id: String,

    /// Free-form coordinates for downstream table generation (issue #6). Kept
    /// as data rather than encoded in `id`, so a table's rows and columns never
    /// depend on parsing an identifier.
    #[serde(default)]
    pub labels: BTreeMap<String, String>,

    /// Programs this run covers. Required for a root run. For a `from` run it
    /// is optional and narrows the parent's set (a subset), which is what makes
    /// "verify only one program of the parent's seven" expressible.
    #[serde(default)]
    pub programs: Vec<String>,

    /// Consume another declared run's output instead of the bench directory.
    /// This is how one frozen translate snapshot feeds several verify runs.
    pub from: Option<String>,

    #[serde(flatten)]
    pub knobs: Knobs,
}

// ── Resolved plan ──────────────────────────────────────────────────────

/// A declared run with inheritance applied and paths resolved.
#[derive(Debug, Clone)]
pub struct ResolvedRun {
    pub id: String,
    pub labels: BTreeMap<String, String>,
    pub programs: Vec<String>,
    pub from: Option<String>,
    /// What the first stage consumes: the bench root, or the parent's root.
    pub input_root: PathBuf,
    /// `<results_root>/<id>`.
    pub output_root: PathBuf,
    /// The exact argv this run lowers to, minus the program filter.
    pub argv: Vec<String>,
    /// Merged `-c` overrides, in resolution order.
    pub config: Vec<String>,
    /// True when the first stage resumes from a snapshot.
    pub resumes: bool,
}

/// Terminal state of one program within one run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProgramState {
    /// Invoked but no terminal record written: the process died mid-program.
    Started,
    /// Ran to completion. Says nothing about whether its tests passed.
    Complete,
    /// Ran and reported a translation/build/grading failure of its own.
    Failed,
    /// Could not run because its `from` parent produced no snapshot for it.
    Blocked,
    /// Infrastructure refused to run it (e.g. a populated output directory).
    /// Distinct from `Failed` because it must never count as done.
    Infra,
}

/// One program's progress record, owned by this module.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgramRecord {
    pub program: String,
    pub state: ProgramState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub translation_success: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_success: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_tests: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub passed_tests: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skipped_tests: Option<usize>,
}

impl ProgramRecord {
    /// Whether this record means "do not run this program again".
    ///
    /// `Infra` is deliberately NOT done: its only error was that the harness
    /// refused to write into a populated directory, which is a condition to
    /// clear and retry, not a result. Counting it as done would freeze an
    /// infrastructure hiccup as a permanent failure in the table.
    fn is_done(&self) -> bool {
        matches!(
            self.state,
            ProgramState::Complete | ProgramState::Failed | ProgramState::Blocked
        )
    }
}

/// A run's receipt: everything needed to know what produced its results,
/// without parsing any directory name.
#[derive(Debug, Serialize)]
pub struct Receipt {
    pub schema_version: u32,
    pub id: String,
    pub labels: BTreeMap<String, String>,
    pub programs: Vec<String>,
    pub from: Option<String>,
    /// The exact argv the run was lowered to.
    pub argv: Vec<String>,
    /// Verbatim `-c` overrides in resolution order.
    pub config: Vec<String>,
    pub harvest_version: String,
    pub programs_total: usize,
    pub programs_done: usize,
}

// ── Loading and validation ─────────────────────────────────────────────

fn merge_knobs(defaults: &Knobs, run: &Knobs) -> Knobs {
    // `config` and `extra_args` concatenate (defaults first, so a run's own
    // entry wins a collision exactly as a later `-c` flag does on the CLI).
    // Every other knob is a straight override.
    let mut config = defaults.config.clone();
    config.extend(run.config.iter().cloned());
    let mut extra_args = defaults.extra_args.clone();
    extra_args.extend(run.extra_args.iter().cloned());
    Knobs {
        stages: run.stages.clone().or_else(|| defaults.stages.clone()),
        modular: run.modular.or(defaults.modular),
        agent: run.agent.clone().or_else(|| defaults.agent.clone()),
        model: run.model.clone().or_else(|| defaults.model.clone()),
        no_plan: run.no_plan.or(defaults.no_plan),
        no_plan_file: run.no_plan_file.or(defaults.no_plan_file),
        workflow: run.workflow.or(defaults.workflow),
        agent_tools: run.agent_tools.or(defaults.agent_tools),
        verify_harness: run
            .verify_harness
            .clone()
            .or_else(|| defaults.verify_harness.clone()),
        fuzz: run.fuzz.or(defaults.fuzz),
        test_case: run.test_case.clone().or_else(|| defaults.test_case.clone()),
        test_harness: run
            .test_harness
            .clone()
            .or_else(|| defaults.test_harness.clone()),
        timeout: run.timeout.or(defaults.timeout),
        config,
        extra_args,
    }
}

/// Is this id safe as a single path component and stable as a table key?
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        && id != "."
        && id != ".."
}

/// Lower one resolved run's knobs into argv for [`Args::try_parse_from`].
///
/// Only knobs the stage set actually accepts are emitted. `validate_stages`
/// rejects e.g. `--fuzz` without the verify stage, so a knob inherited from
/// `[defaults]` must not be pushed onto a run whose stages cannot take it —
/// otherwise inheriting one default would make unrelated runs unrunnable.
fn lower_to_argv(
    knobs: &Knobs,
    input_root: &Path,
    output_root: &Path,
    stages: &[String],
    manifest_dir: &Path,
) -> Vec<String> {
    let agentic = !stages.is_empty();
    let has = |s: &str| stages.iter().any(|x| stage_is(x, s));
    let mut argv: Vec<String> = vec![
        "harvest-benchmark".into(),
        input_root.to_string_lossy().into_owned(),
        output_root.to_string_lossy().into_owned(),
    ];

    if agentic {
        argv.push(format!("--agentic={}", stages.join(",")));
    } else if knobs.modular.unwrap_or(false) {
        argv.push("--modular".into());
    }

    // requires = "agentic" on all of these.
    if agentic {
        if let Some(a) = &knobs.agent {
            argv.push(format!("--agent={a}"));
        }
        if let Some(m) = &knobs.model {
            argv.push(format!("--model={m}"));
        }
        if knobs.agent_tools.unwrap_or(false) {
            argv.push("--agent-tools".into());
        }
        // translate-or-verify scoped
        if has("translate") || has("verify") {
            if knobs.no_plan.unwrap_or(false) {
                argv.push("--no-plan".into());
                // requires = "no_plan"
                if knobs.workflow.unwrap_or(false) {
                    argv.push("--workflow".into());
                }
            } else if knobs.no_plan_file.unwrap_or(false) {
                argv.push("--no-plan-file".into());
            }
        }
        // verify-stage scoped
        if has("verify") {
            if let Some(v) = &knobs.verify_harness {
                argv.push(format!("--verify-harness={v}"));
            }
            if knobs.fuzz.unwrap_or(false) {
                argv.push("--fuzz".into());
            }
        }
        // Only meaningful when resuming.
        if has("verify") || has("conform") {
            if let Some(tc) = &knobs.test_case {
                argv.push(format!(
                    "--test-case={}",
                    resolve(manifest_dir, tc).display()
                ));
            }
        }
    }

    if let Some(t) = &knobs.test_harness {
        argv.push(format!("--test-harness={t}"));
    }
    if let Some(t) = knobs.timeout {
        argv.push(format!("--timeout={t}"));
    }
    for c in &knobs.config {
        // Equals form: values may contain spaces, commas, or a second '='.
        // There is no shell here, so they survive byte-identical.
        argv.push(format!("--config={c}"));
    }
    argv.extend(knobs.extra_args.iter().cloned());
    argv
}

/// Reject a knob the run set *explicitly* on itself that its own stages cannot
/// accept.
///
/// This is the counterpart to the stage-scoping in [`lower_to_argv`]. Scoping is
/// what lets `[defaults]` carry a verify-stage knob without making every
/// translate-only run in the set unrunnable — but applying it to a knob written
/// on the run itself would silently ignore it, and the run would then record an
/// identity (fuzz, gtest, no_plan…) that does not describe what actually ran.
/// So: inherited knobs are scoped, explicit knobs are an error.
fn reject_inapplicable_explicit_knobs(
    id: &str,
    own: &Knobs,
    merged: &Knobs,
    stages: &[String],
) -> Result<(), String> {
    let agentic = !stages.is_empty();
    let has = |s: &str| has_stage(stages, s);
    let stages_str = || {
        if stages.is_empty() {
            "none (non-agentic)".to_owned()
        } else {
            stages.join(",")
        }
    };
    let mut bad: Vec<&str> = Vec::new();

    let agentic_only: [(bool, &str); 8] = [
        (own.agent.is_some(), "agent"),
        (own.model.is_some(), "model"),
        (own.agent_tools.unwrap_or(false), "agent_tools"),
        (own.no_plan.unwrap_or(false), "no_plan"),
        (own.no_plan_file.unwrap_or(false), "no_plan_file"),
        (own.workflow.unwrap_or(false), "workflow"),
        (own.fuzz.unwrap_or(false), "fuzz"),
        (own.verify_harness.is_some(), "verify_harness"),
    ];
    if !agentic {
        for (set, name) in agentic_only {
            if set {
                bad.push(name);
            }
        }
        if !bad.is_empty() {
            return Err(format!(
                "run {id:?} sets {} but declares no agentic stages; those knobs only apply \
                 to an agentic run",
                bad.join(", ")
            ));
        }
        if own.test_case.is_some() {
            return Err(format!(
                "run {id:?} sets test_case but declares no agentic stages"
            ));
        }
        return Ok(());
    }

    if own.modular.unwrap_or(false) {
        return Err(format!(
            "run {id:?} sets modular = true together with stages = [{}]; modular is the \
             non-agentic translator, so declare it with no stages",
            stages_str()
        ));
    }
    for (set, name) in [
        (own.fuzz.unwrap_or(false), "fuzz"),
        (own.verify_harness.is_some(), "verify_harness"),
    ] {
        if set && !has("verify") {
            bad.push(name);
        }
    }
    if !bad.is_empty() {
        return Err(format!(
            "run {id:?} sets {} but its stages are [{}]; those knobs require the verify stage",
            bad.join(", "),
            stages_str()
        ));
    }
    for (set, name) in [
        (own.no_plan.unwrap_or(false), "no_plan"),
        (own.no_plan_file.unwrap_or(false), "no_plan_file"),
        (own.workflow.unwrap_or(false), "workflow"),
    ] {
        if set && !(has("translate") || has("verify")) {
            bad.push(name);
        }
    }
    if !bad.is_empty() {
        return Err(format!(
            "run {id:?} sets {} but its stages are [{}]; those knobs require the translate \
             or verify stage",
            bad.join(", "),
            stages_str()
        ));
    }
    if own.test_case.is_some() && !(has("verify") || has("conform")) {
        return Err(format!(
            "run {id:?} sets test_case but its stages are [{}]; a suite override only applies \
             when resuming (verify or conform)",
            stages_str()
        ));
    }
    // Mirrors clap's `requires = "no_plan"` on --workflow. Checked here rather
    // than left to clap because --workflow is only emitted alongside --no-plan,
    // so an explicit `workflow = true` without it would otherwise be dropped
    // before clap ever saw it.
    if own.workflow.unwrap_or(false) && !merged.no_plan.unwrap_or(false) {
        return Err(format!(
            "run {id:?} sets workflow = true without no_plan = true; the workflow hint is \
             only wired up in no-plan mode"
        ));
    }
    Ok(())
}

/// Accepts the same stage spellings clap does (full name or first-letter alias).
fn stage_is(value: &str, full: &str) -> bool {
    let v = value.trim().to_ascii_lowercase();
    v == full || (v.len() == 1 && full.starts_with(&v))
}

/// Resolve a manifest-relative path against the manifest's own directory.
fn resolve(manifest_dir: &Path, p: &Path) -> PathBuf {
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        manifest_dir.join(p)
    }
}

/// Load, validate, and resolve an experiment set.
///
/// Everything that can be wrong with the file is reported here rather than
/// hours into a sweep: unknown keys (serde), bad ids, duplicate ids, unknown
/// or cyclic `from` targets, programs that are not subsets of their parent,
/// programs missing from the bench root, and any knob combination clap or
/// `validate_stages` rejects.
pub fn load(manifest_path: &Path) -> HarvestResult<(ExperimentSet, Vec<ResolvedRun>)> {
    let text = std::fs::read_to_string(manifest_path)
        .map_err(|e| format!("cannot read {}: {e}", manifest_path.display()))?;
    let set: ExperimentSet = toml::from_str(&text).map_err(|e| {
        format!(
            "{} is not a valid experiment set:\n{e}",
            manifest_path.display()
        )
    })?;

    if set.schema_version != SCHEMA_VERSION {
        return Err(format!(
            "{}: schema_version is {}, this build understands {}",
            manifest_path.display(),
            set.schema_version,
            SCHEMA_VERSION
        )
        .into());
    }
    if set.runs.is_empty() {
        return Err(format!("{}: declares no [[run]] entries", manifest_path.display()).into());
    }

    let manifest_dir = manifest_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
        .to_path_buf();
    let bench_root = resolve(&manifest_dir, &set.bench_root);
    let results_root = resolve(&manifest_dir, &set.results_root);

    // Duplicate ids would silently collapse two declared experiments onto one
    // output root, and the second would then look already-complete.
    let mut seen: HashSet<&str> = HashSet::new();
    for r in &set.runs {
        if !valid_id(&r.id) {
            return Err(format!(
                "run id {:?} is not usable as a directory name; use only \
                 letters, digits, '.', '_' and '-'",
                r.id
            )
            .into());
        }
        if !seen.insert(r.id.as_str()) {
            return Err(format!("duplicate run id {:?}", r.id).into());
        }
    }

    // Resolve in declaration order so a `from` can only refer to a run
    // declared before it. That also makes cycles impossible by construction.
    let mut resolved: Vec<ResolvedRun> = Vec::new();
    let mut by_id: HashMap<String, usize> = HashMap::new();

    for spec in &set.runs {
        let knobs = merge_knobs(&set.defaults, &spec.knobs);
        let stages: Vec<String> = knobs.stages.clone().unwrap_or_default();
        let resumes = stages
            .first()
            .map(|s| stage_is(s, "verify") || stage_is(s, "conform"))
            .unwrap_or(false);

        let (input_root, programs) = match &spec.from {
            Some(parent_id) => {
                let idx = *by_id.get(parent_id.as_str()).ok_or_else(|| {
                    format!(
                        "run {:?}: from = {:?} names a run that is not declared earlier in the file",
                        spec.id, parent_id
                    )
                })?;
                let parent: &ResolvedRun = &resolved[idx];
                let programs = if spec.programs.is_empty() {
                    parent.programs.clone()
                } else {
                    for p in &spec.programs {
                        if !parent.programs.contains(p) {
                            return Err(format!(
                                "run {:?}: program {:?} is not in the program set of its \
                                 parent {:?} ({:?})",
                                spec.id, p, parent_id, parent.programs
                            )
                            .into());
                        }
                    }
                    spec.programs.clone()
                };
                (parent.output_root.clone(), programs)
            }
            None => {
                if spec.programs.is_empty() {
                    return Err(format!(
                        "run {:?}: a run with no `from` must list its `programs`",
                        spec.id
                    )
                    .into());
                }
                (bench_root.clone(), spec.programs.clone())
            }
        };

        if spec.from.is_some() && !resumes {
            return Err(format!(
                "run {:?}: from = {:?} consumes a snapshot, so its first stage must be \
                 verify or conform (got stages = {:?})",
                spec.id, spec.from, stages
            )
            .into());
        }
        if spec.from.is_none() && resumes {
            return Err(format!(
                "run {:?}: stages start with {:?}, which resumes from a snapshot, so the run \
                 needs `from = \"<run id>\"`",
                spec.id,
                stages.first().map(String::as_str).unwrap_or("")
            )
            .into());
        }

        // Explicit knobs are checked against this run's own stages before the
        // lowering scopes anything away.
        reject_inapplicable_explicit_knobs(&spec.id, &spec.knobs, &knobs, &stages)?;

        let output_root = results_root.join(&spec.id);
        let argv = lower_to_argv(&knobs, &input_root, &output_root, &stages, &manifest_dir);

        // The decisive check: hand the lowered argv to clap and to
        // validate_stages, so a declared run is exactly as constrained as the
        // equivalent command line. Any `requires`/`conflicts_with`/stage rule
        // is enforced here without being restated.
        let parsed = Args::try_parse_from(&argv).map_err(|e| {
            format!(
                "run {:?} does not lower to a valid invocation:\n  {}\n{e}",
                spec.id,
                argv.join(" ")
            )
        })?;
        parsed
            .validate_stages(&parsed.stages())
            .map_err(|e| format!("run {:?}: {e}\n  {}", spec.id, argv.join(" ")))?;

        // Contradictions clap cannot see, each of which would otherwise record
        // a run identity that does not match what actually ran.
        if has_stage(&stages, "verify")
            && (knobs.no_plan.unwrap_or(false) || knobs.no_plan_file.unwrap_or(false))
            && (knobs.fuzz.unwrap_or(false) || knobs.verify_harness.is_some())
        {
            return Err(format!(
                "run {:?}: no_plan/no_plan_file cannot be combined with fuzz or \
                 verify_harness. The gtest verify harness is only active when plan files \
                 are enabled, so the run would record gtest/fuzz in its identity while \
                 actually verifying via libloading (see verify_fix_agentic gtest_harness_active).",
                spec.id
            )
            .into());
        }
        if knobs.workflow.unwrap_or(false)
            && knobs
                .agent
                .as_deref()
                .map(|a| a != "claude")
                .unwrap_or(false)
        {
            return Err(format!(
                "run {:?}: workflow only affects the claude agent; with agent = {:?} the \
                 hint is silently dropped",
                spec.id,
                knobs.agent.as_deref().unwrap_or("")
            )
            .into());
        }

        // Root runs must name programs that exist; `deny_unknown_fields`
        // catches misspelled keys but never misspelled values, and a filter
        // that matches nothing yields an empty, header-only result set.
        if spec.from.is_none() {
            let mut missing = Vec::new();
            for p in &programs {
                if !bench_root.join(p).is_dir() {
                    missing.push(p.clone());
                }
            }
            if !missing.is_empty() {
                return Err(format!(
                    "run {:?}: programs not found under {}: {}",
                    spec.id,
                    bench_root.display(),
                    missing.join(", ")
                )
                .into());
            }
        }

        by_id.insert(spec.id.clone(), resolved.len());
        resolved.push(ResolvedRun {
            id: spec.id.clone(),
            labels: spec.labels.clone(),
            programs,
            from: spec.from.clone(),
            input_root,
            output_root,
            argv,
            config: knobs.config.clone(),
            resumes,
        });
    }

    Ok((set, resolved))
}

fn has_stage(stages: &[String], full: &str) -> bool {
    stages.iter().any(|s| stage_is(s, full))
}

// ── Progress records ───────────────────────────────────────────────────

fn programs_dir(run_root: &Path) -> PathBuf {
    run_root.join(SWEEP_META_DIR).join(PROGRAMS_SUBDIR)
}

fn record_path(run_root: &Path, program: &str) -> PathBuf {
    programs_dir(run_root).join(format!("{program}.json"))
}

/// Read one program's record, if any. A malformed record is treated as absent
/// so a corrupted file causes a re-run rather than a permanent skip.
pub fn read_record(run_root: &Path, program: &str) -> Option<ProgramRecord> {
    let text = std::fs::read_to_string(record_path(run_root, program)).ok()?;
    serde_json::from_str(&text).ok()
}

/// Write a program's record. Temp-then-rename so a crash cannot leave a
/// half-written record that later parses as something else.
fn write_record(run_root: &Path, rec: &ProgramRecord) -> HarvestResult<()> {
    let dir = programs_dir(run_root);
    std::fs::create_dir_all(&dir)?;
    let final_path = record_path(run_root, &rec.program);
    let tmp = dir.join(format!(".{}.json.tmp", rec.program));
    std::fs::write(&tmp, serde_json::to_string_pretty(rec)?)?;
    std::fs::rename(&tmp, &final_path)?;
    Ok(())
}

/// Classify a finished program's stats into a terminal state.
///
/// The `ProgramEvalStats` returned by a program run has no "did not run"
/// channel — every early return produces a stats row — so an infrastructure
/// refusal has to be recognized from its message and kept out of the done set.
fn classify(stats: &ProgramEvalStats) -> ProgramState {
    if let Some(err) = &stats.error_message {
        if err.contains("is not empty") {
            return ProgramState::Infra;
        }
    }
    if !stats.translation_success || !stats.rust_build_success {
        return ProgramState::Failed;
    }
    ProgramState::Complete
}

fn record_from_stats(stats: &ProgramEvalStats) -> ProgramRecord {
    ProgramRecord {
        program: stats.program_name.clone(),
        state: classify(stats),
        error: stats.error_message.clone(),
        translation_success: Some(stats.translation_success),
        build_success: Some(stats.rust_build_success),
        total_tests: Some(stats.total_tests),
        passed_tests: Some(stats.passed_tests),
        skipped_tests: Some(stats.skipped_tests),
    }
}

fn write_receipt(run: &ResolvedRun, done: usize) -> HarvestResult<()> {
    let dir = run.output_root.join(SWEEP_META_DIR);
    std::fs::create_dir_all(&dir)?;
    let receipt = Receipt {
        schema_version: SCHEMA_VERSION,
        id: run.id.clone(),
        labels: run.labels.clone(),
        programs: run.programs.clone(),
        from: run.from.clone(),
        argv: run.argv.clone(),
        config: run.config.clone(),
        harvest_version: get_version().to_owned(),
        programs_total: run.programs.len(),
        programs_done: done,
    };
    let final_path = dir.join(RECEIPT_FILE);
    let tmp = dir.join(".receipt.json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(&receipt)?)?;
    std::fs::rename(&tmp, &final_path)?;
    Ok(())
}

// ── Status ─────────────────────────────────────────────────────────────

/// What `--status` reports for one run.
#[derive(Debug)]
pub struct RunStatus {
    pub id: String,
    pub total: usize,
    pub done: usize,
    /// Programs invoked but with no terminal record: the process died.
    pub interrupted: Vec<String>,
    /// Programs the harness refused to run; clear and retry.
    pub infra: Vec<String>,
    pub failed: Vec<String>,
    pub blocked: Vec<String>,
    pub missing: Vec<String>,
}

impl RunStatus {
    pub fn is_complete(&self) -> bool {
        self.done == self.total && self.total > 0
    }
}

/// Inspect the filesystem and report each declared run's progress.
///
/// Deliberately read-only: it opens no log file and creates no directory, so
/// checking progress from a second shell during a multi-hour sweep cannot
/// disturb the sweep.
pub fn status(runs: &[ResolvedRun]) -> Vec<RunStatus> {
    runs.iter()
        .map(|run| {
            let mut st = RunStatus {
                id: run.id.clone(),
                total: run.programs.len(),
                done: 0,
                interrupted: Vec::new(),
                infra: Vec::new(),
                failed: Vec::new(),
                blocked: Vec::new(),
                missing: Vec::new(),
            };
            for p in &run.programs {
                match read_record(&run.output_root, p) {
                    None => st.missing.push(p.clone()),
                    Some(rec) => {
                        if rec.is_done() {
                            st.done += 1;
                        }
                        match rec.state {
                            ProgramState::Started => st.interrupted.push(p.clone()),
                            ProgramState::Infra => st.infra.push(p.clone()),
                            ProgramState::Failed => st.failed.push(p.clone()),
                            ProgramState::Blocked => st.blocked.push(p.clone()),
                            ProgramState::Complete => {}
                        }
                    }
                }
            }
            st
        })
        .collect()
}

/// Render the status table and return true when every declared run is complete.
pub fn print_status(runs: &[ResolvedRun]) -> bool {
    let all = status(runs);
    println!(
        "{:<34} {:>9}  OUTSTANDING (interrupted/infra/failed/blocked/missing)",
        "RUN", "PROGRESS"
    );
    let mut complete = true;
    for st in &all {
        if !st.is_complete() {
            complete = false;
        }
        let mut notes: Vec<String> = Vec::new();
        let mut note = |label: &str, xs: &Vec<String>| {
            if !xs.is_empty() {
                notes.push(format!("{label}: {}", xs.join(",")));
            }
        };
        note("interrupted", &st.interrupted);
        note("infra", &st.infra);
        note("failed", &st.failed);
        note("blocked", &st.blocked);
        note("missing", &st.missing);
        println!(
            "{:<34} {:>4}/{:<4}  {}",
            st.id,
            st.done,
            st.total,
            if notes.is_empty() {
                "-".to_owned()
            } else {
                notes.join("  ")
            }
        );
    }
    complete
}

/// Print the resolved plan without running anything.
pub fn print_plan(runs: &[ResolvedRun]) {
    let programs: usize = runs.iter().map(|r| r.programs.len()).sum();
    println!(
        "{} declared run(s), {} program invocation(s) total\n",
        runs.len(),
        programs
    );
    for r in runs {
        println!("[{}]", r.id);
        println!("  programs : {}", r.programs.join(", "));
        if let Some(f) = &r.from {
            println!("  from     : {f}");
        }
        println!("  input    : {}", r.input_root.display());
        println!("  output   : {}", r.output_root.display());
        println!("  argv     : {}", r.argv.join(" "));
        if !r.labels.is_empty() {
            let labels: Vec<String> = r.labels.iter().map(|(k, v)| format!("{k}={v}")).collect();
            println!("  labels   : {}", labels.join(" "));
        }
        println!();
    }
}

// ── Execution ──────────────────────────────────────────────────────────

/// Build the `ProgramRun` for one program of one run.
///
/// Mirrors what the positional path does, but for a single named program, so
/// the sweep never has to discover directories by globbing.
fn program_run_for(run: &ResolvedRun, program: &str) -> HarvestResult<Option<ProgramRun>> {
    let dir = run.input_root.join(program);
    if run.resumes {
        if !harvest_core::stage_manifest::is_snapshot(&dir) {
            // The parent produced nothing for this program (e.g. its
            // translation failed, so no manifest was ever written). That is
            // `blocked`, not `missing`: there is nothing to retry until the
            // parent is re-run.
            return Ok(None);
        }
        Ok(Some(crate::resume_from(&dir, program.to_owned(), None)?))
    } else {
        Ok(Some(crate::start_from_bench(&dir, program.to_owned())?))
    }
}

/// Execute a declared set, skipping programs that already finished.
///
/// One program at a time, with its record written immediately after it
/// returns. That granularity is the whole point: the aggregate CSV is only
/// written after a whole run finishes, so a sweep interrupted midway would
/// otherwise leave no trace of the programs that did complete, and re-running
/// would redo them — or worse, hit the populated-directory guard and record
/// completed work as failures.
pub fn execute(runs: &[ResolvedRun], only: &[String], force: bool) -> HarvestResult<()> {
    let selected: Vec<&ResolvedRun> = runs
        .iter()
        .filter(|r| only.is_empty() || only.contains(&r.id))
        .collect();
    if selected.is_empty() {
        return Err(format!(
            "--only matched no declared run; available ids: {}",
            runs.iter()
                .map(|r| r.id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )
        .into());
    }

    for run in selected {
        log::info!("\n{}", "=".repeat(80));
        log::info!("Run [{}] -> {}", run.id, run.output_root.display());
        log::info!("{}", "=".repeat(80));

        // Re-parse the lowered argv to get this run's options. It already
        // passed clap and validate_stages at load time.
        let args = Args::try_parse_from(&run.argv)
            .map_err(|e| format!("run {:?}: argv no longer parses: {e}", run.id))?;
        let stages = args.stages();

        std::fs::create_dir_all(&run.output_root)?;

        let mut done = 0usize;
        for program in &run.programs {
            let prior = read_record(&run.output_root, program);
            if let Some(rec) = &prior {
                if rec.is_done() {
                    done += 1;
                    log::info!("  ⏭️  {program}: already {:?}, skipping", rec.state);
                    continue;
                }
            }

            let program_run = match program_run_for(run, program)? {
                Some(pr) => pr,
                None => {
                    log::warn!(
                        "  🚧 {program}: parent run produced no snapshot; recording blocked"
                    );
                    write_record(
                        &run.output_root,
                        &ProgramRecord {
                            program: program.clone(),
                            state: ProgramState::Blocked,
                            error: Some(format!(
                                "no snapshot at {}",
                                run.input_root.join(program).display()
                            )),
                            translation_success: None,
                            build_success: None,
                            total_tests: None,
                            passed_tests: None,
                            skipped_tests: None,
                        },
                    )?;
                    done += 1;
                    continue;
                }
            };

            // A program with a non-terminal prior record left a partially
            // written output directory behind. Clear just that program so the
            // populated-directory guard does not turn a crash into a permanent
            // failure. Scoped to this program: never a blanket force.
            let retry = prior.is_some();
            if retry {
                log::warn!("  ♻️  {program}: previous attempt did not finish; retrying");
            }

            write_record(
                &run.output_root,
                &ProgramRecord {
                    program: program.clone(),
                    state: ProgramState::Started,
                    error: None,
                    translation_success: None,
                    build_success: None,
                    total_tests: None,
                    passed_tests: None,
                    skipped_tests: None,
                },
            )?;

            let opts = RunOptions {
                config_overrides: args.config.clone(),
                timeout: args.timeout,
                modular: args.modular,
                stages: stages.clone(),
                agent: args.agent,
                agent_tools: args.agent_tools,
                model: args.model.clone(),
                no_plan: args.no_plan,
                no_plan_file: args.no_plan_file,
                workflow: args.workflow,
                test_harness: args.test_harness,
                verify_harness: args.verify_harness.unwrap_or_default(),
                fuzz: args.fuzz,
                force: force || retry,
            };

            let results = crate::run_all_benchmarks(
                std::slice::from_ref(&program_run),
                &run.output_root,
                &opts,
            )?;
            for stats in &results {
                let rec = record_from_stats(stats);
                if rec.is_done() {
                    done += 1;
                }
                write_record(&run.output_root, &rec)?;
            }
            write_receipt(run, done)?;
        }

        write_receipt(run, done)?;
        log::info!(
            "Run [{}]: {}/{} programs done",
            run.id,
            done,
            run.programs.len()
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write_set(dir: &Path, body: &str) -> PathBuf {
        fs::create_dir_all(dir.join("bench/lz4/test_case/src")).unwrap();
        fs::create_dir_all(dir.join("bench/libpng/test_case/src")).unwrap();
        let p = dir.join("set.toml");
        fs::write(&p, body).unwrap();
        p
    }

    const HEAD: &str = "schema_version = 1\nbench_root = \"bench\"\nresults_root = \"out\"\n";

    #[test]
    fn loads_a_chained_translate_verify_set() {
        let tmp = tempfile::tempdir().unwrap();
        let p = write_set(
            tmp.path(),
            &format!(
                "{HEAD}
[defaults]
agent = \"claude\"

[[run]]
id = \"t\"
programs = [\"lz4\", \"libpng\"]
stages = [\"translate\"]
model = \"sonnet\"

[[run]]
id = \"t.v_sonnet\"
from = \"t\"
stages = [\"verify\"]
model = \"sonnet\"

[[run]]
id = \"t.v_opus\"
from = \"t\"
programs = [\"lz4\"]
stages = [\"verify\"]
model = \"opus\"
"
            ),
        );
        let (_set, runs) = load(&p).unwrap();
        assert_eq!(runs.len(), 3);
        // One frozen translate feeds both verify runs.
        assert_eq!(runs[1].input_root, runs[0].output_root);
        assert_eq!(runs[2].input_root, runs[0].output_root);
        // A `from` run inherits the parent's programs unless it narrows them.
        assert_eq!(runs[1].programs, vec!["lz4", "libpng"]);
        assert_eq!(runs[2].programs, vec!["lz4"]);
        // Output roots are derived from the id, never hand-typed.
        assert!(runs[0].output_root.ends_with("out/t"));
        assert!(runs[2].output_root.ends_with("out/t.v_opus"));
        // Inherited default reached the argv.
        assert!(runs[0].argv.iter().any(|a| a == "--agent=claude"));
    }

    #[test]
    fn unknown_key_is_rejected_with_the_expected_field_list() {
        let tmp = tempfile::tempdir().unwrap();
        // A silently-ignored typo would burn a whole run and then report it
        // complete, so the loader is strict rather than warn-and-continue.
        let p = write_set(
            tmp.path(),
            &format!("{HEAD}\n[[run]]\nid = \"a\"\nprograms = [\"lz4\"]\nmodle = \"sonnet\"\n"),
        );
        let err = load(&p).unwrap_err().to_string();
        assert!(err.contains("unknown field"), "{err}");
        assert!(err.contains("modle"), "{err}");
    }

    #[test]
    fn duplicate_ids_are_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        // Both would resolve to the same output root, so the second would look
        // already-complete and silently never run.
        let p = write_set(
            tmp.path(),
            &format!(
                "{HEAD}\n[[run]]\nid = \"a\"\nprograms = [\"lz4\"]\n\n\
                 [[run]]\nid = \"a\"\nprograms = [\"libpng\"]\n"
            ),
        );
        let err = load(&p).unwrap_err().to_string();
        assert!(err.contains("duplicate run id"), "{err}");
    }

    #[test]
    fn ids_must_be_safe_path_components() {
        let tmp = tempfile::tempdir().unwrap();
        for bad in ["../escape", "a/b", ""] {
            let p = write_set(
                tmp.path(),
                &format!("{HEAD}\n[[run]]\nid = \"{bad}\"\nprograms = [\"lz4\"]\n"),
            );
            assert!(load(&p).is_err(), "id {bad:?} should be rejected");
        }
    }

    #[test]
    fn misspelled_program_is_rejected_at_load() {
        let tmp = tempfile::tempdir().unwrap();
        // deny_unknown_fields catches bad KEYS but never bad VALUES; a filter
        // matching nothing would otherwise yield an empty, header-only result.
        let p = write_set(
            tmp.path(),
            &format!("{HEAD}\n[[run]]\nid = \"a\"\nprograms = [\"1z4\"]\n"),
        );
        let err = load(&p).unwrap_err().to_string();
        assert!(err.contains("programs not found"), "{err}");
        assert!(err.contains("1z4"), "{err}");
    }

    #[test]
    fn from_must_name_an_earlier_run_and_narrow_its_programs() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = write_set(
            tmp.path(),
            &format!("{HEAD}\n[[run]]\nid = \"v\"\nfrom = \"nope\"\nstages = [\"verify\"]\n"),
        );
        assert!(load(&missing)
            .unwrap_err()
            .to_string()
            .contains("not declared earlier"));

        let widening = write_set(
            tmp.path(),
            &format!(
                "{HEAD}
[[run]]
id = \"t\"
programs = [\"lz4\"]
stages = [\"translate\"]

[[run]]
id = \"v\"
from = \"t\"
programs = [\"lz4\", \"libpng\"]
stages = [\"verify\"]
"
            ),
        );
        let err = load(&widening).unwrap_err().to_string();
        assert!(
            err.contains("not in the program set of its parent"),
            "{err}"
        );
    }

    #[test]
    fn stage_and_from_must_agree() {
        let tmp = tempfile::tempdir().unwrap();
        // verify without a parent snapshot, and a parent with translate stages.
        let orphan_verify = write_set(
            tmp.path(),
            &format!("{HEAD}\n[[run]]\nid = \"v\"\nprograms = [\"lz4\"]\nstages = [\"verify\"]\n"),
        );
        assert!(load(&orphan_verify)
            .unwrap_err()
            .to_string()
            .contains("needs `from"));

        let from_translate = write_set(
            tmp.path(),
            &format!(
                "{HEAD}
[[run]]
id = \"t\"
programs = [\"lz4\"]
stages = [\"translate\"]

[[run]]
id = \"t2\"
from = \"t\"
stages = [\"translate\"]
"
            ),
        );
        let err = load(&from_translate).unwrap_err().to_string();
        assert!(err.contains("verify or conform"), "{err}");
    }

    #[test]
    fn an_explicit_knob_its_stages_cannot_take_is_rejected_not_dropped() {
        let tmp = tempfile::tempdir().unwrap();
        // fuzz requires the verify stage. Silently scoping it away here would
        // leave the run recording an identity that does not match what ran, so
        // an explicitly-set knob must be an error rather than ignored.
        let p = write_set(
            tmp.path(),
            &format!(
                "{HEAD}\n[[run]]\nid = \"a\"\nprograms = [\"lz4\"]\n\
                 stages = [\"translate\"]\nfuzz = true\n"
            ),
        );
        let err = load(&p).unwrap_err().to_string();
        assert!(err.contains("fuzz"), "{err}");
        assert!(err.contains("verify stage"), "{err}");

        // And a knob that needs an agentic run at all.
        let non_agentic = write_set(
            tmp.path(),
            &format!("{HEAD}\n[[run]]\nid = \"a\"\nprograms = [\"lz4\"]\nmodel = \"sonnet\"\n"),
        );
        let err = load(&non_agentic).unwrap_err().to_string();
        assert!(err.contains("no agentic stages"), "{err}");
    }

    #[test]
    fn workflow_without_no_plan_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        // --workflow is only emitted alongside --no-plan, so an explicit
        // workflow = true without it would be dropped before clap saw it.
        let p = write_set(
            tmp.path(),
            &format!(
                "{HEAD}\n[[run]]\nid = \"a\"\nprograms = [\"lz4\"]\n\
                 stages = [\"translate\"]\nworkflow = true\nagent = \"claude\"\n"
            ),
        );
        let err = load(&p).unwrap_err().to_string();
        assert!(err.contains("no_plan"), "{err}");
    }

    #[test]
    fn clap_itself_rejects_a_bad_enum_value() {
        let tmp = tempfile::tempdir().unwrap();
        // Enum-valued knobs are Strings here on purpose, so clap owns the
        // vocabulary and its own enumerated error surfaces.
        let p = write_set(
            tmp.path(),
            &format!(
                "{HEAD}\n[[run]]\nid = \"a\"\nprograms = [\"lz4\"]\ntest_harness = \"gtst\"\n"
            ),
        );
        let err = load(&p).unwrap_err().to_string();
        assert!(err.contains("gtst"), "{err}");

        // ...while the documented CLI aliases keep working.
        let alias = write_set(
            tmp.path(),
            &format!(
                "{HEAD}\n[[run]]\nid = \"a\"\nprograms = [\"lz4\"]\n\
                 stages = [\"t\"]\nagent = \"oc\"\nmodel = \"opencode-go/x\"\n"
            ),
        );
        let (_s, runs) = load(&alias).expect("stage alias 't' and agent alias 'oc' must parse");
        assert!(runs[0].argv.iter().any(|a| a == "--agent=oc"));
    }

    #[test]
    fn defaults_do_not_make_unrelated_runs_unrunnable() {
        let tmp = tempfile::tempdir().unwrap();
        // fuzz is a verify-stage knob. Inheriting it from [defaults] must not
        // be pushed onto a translate-only run, or validate_stages would reject
        // a third of the set with no key to point at.
        let p = write_set(
            tmp.path(),
            &format!(
                "{HEAD}
[defaults]
fuzz = true
verify_harness = \"gtest\"

[[run]]
id = \"t\"
programs = [\"lz4\"]
stages = [\"translate\"]

[[run]]
id = \"t.v\"
from = \"t\"
stages = [\"verify\"]
"
            ),
        );
        let (_s, runs) = load(&p).unwrap();
        assert!(!runs[0].argv.iter().any(|a| a == "--fuzz"));
        assert!(runs[1].argv.iter().any(|a| a == "--fuzz"));
    }

    #[test]
    fn no_plan_with_fuzz_is_rejected_as_a_mislabelling_hazard() {
        let tmp = tempfile::tempdir().unwrap();
        // The gtest verify harness is only active when plan files are enabled,
        // so this combination records gtest/fuzz while running libloading.
        let p = write_set(
            tmp.path(),
            &format!(
                "{HEAD}
[[run]]
id = \"t\"
programs = [\"lz4\"]
stages = [\"translate\"]

[[run]]
id = \"t.v\"
from = \"t\"
stages = [\"verify\"]
no_plan = true
fuzz = true
"
            ),
        );
        let err = load(&p).unwrap_err().to_string();
        assert!(err.contains("no_plan"), "{err}");
    }

    #[test]
    fn non_agentic_baselines_are_expressible() {
        let tmp = tempfile::tempdir().unwrap();
        // Without these the ablation table cannot hold its own control row.
        let p = write_set(
            tmp.path(),
            &format!(
                "{HEAD}
[[run]]
id = \"one_shot\"
programs = [\"lz4\"]

[[run]]
id = \"modular\"
programs = [\"lz4\"]
modular = true
"
            ),
        );
        let (_s, runs) = load(&p).unwrap();
        assert!(!runs[0].argv.iter().any(|a| a.starts_with("--agentic")));
        assert!(runs[1].argv.iter().any(|a| a == "--modular"));
    }

    #[test]
    fn config_overrides_survive_verbatim_and_defaults_come_first() {
        let tmp = tempfile::tempdir().unwrap();
        let p = write_set(
            tmp.path(),
            &format!(
                "{HEAD}
[defaults]
config = [\"tools.a.x=1\"]

[[run]]
id = \"a\"
programs = [\"lz4\"]
config = [\"tools.b.prompt=use --no-plan, then a=b=c\"]
"
            ),
        );
        let (_s, runs) = load(&p).unwrap();
        assert_eq!(
            runs[0].config,
            vec![
                "tools.a.x=1".to_owned(),
                "tools.b.prompt=use --no-plan, then a=b=c".to_owned()
            ]
        );
        // Equals form, so a value with spaces/commas/'=' needs no quoting.
        assert!(runs[0]
            .argv
            .iter()
            .any(|a| a == "--config=tools.b.prompt=use --no-plan, then a=b=c"));
    }

    #[test]
    fn extra_args_reach_clap_and_are_still_validated() {
        let tmp = tempfile::tempdir().unwrap();
        let ok = write_set(
            tmp.path(),
            &format!(
                "{HEAD}\n[[run]]\nid = \"a\"\nprograms = [\"lz4\"]\nextra_args = [\"--force\"]\n"
            ),
        );
        assert!(load(&ok).is_ok());

        let bogus = write_set(
            tmp.path(),
            &format!(
                "{HEAD}\n[[run]]\nid = \"a\"\nprograms = [\"lz4\"]\nextra_args = [\"--nope\"]\n"
            ),
        );
        assert!(
            load(&bogus).is_err(),
            "clap must still reject unknown flags"
        );
    }

    #[test]
    fn infra_refusal_is_not_counted_as_done() {
        // A populated output directory is a condition to clear and retry, not
        // a result; counting it as done would freeze completed work as a
        // permanent failure in the reported numbers.
        let mut stats = ProgramEvalStats::new("lz4");
        stats.error_message =
            Some("output program directory /x/lz4 is not empty; pass --force".to_owned());
        assert_eq!(classify(&stats), ProgramState::Infra);
        assert!(!record_from_stats(&stats).is_done());

        let mut failed = ProgramEvalStats::new("lz4");
        failed.error_message = Some("Failed to translate C project".to_owned());
        assert_eq!(classify(&failed), ProgramState::Failed);
        assert!(record_from_stats(&failed).is_done());
    }

    #[test]
    fn started_without_a_terminal_record_reads_as_interrupted() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("run");
        let started = ProgramRecord {
            program: "lz4".to_owned(),
            state: ProgramState::Started,
            error: None,
            translation_success: None,
            build_success: None,
            total_tests: None,
            passed_tests: None,
            skipped_tests: None,
        };
        write_record(&root, &started).unwrap();
        let back = read_record(&root, "lz4").unwrap();
        assert_eq!(back.state, ProgramState::Started);
        // Not done: it must be retried, not skipped forever.
        assert!(!back.is_done());
    }

    #[test]
    fn a_complete_program_is_skipped_and_a_missing_one_is_reported() {
        let tmp = tempfile::tempdir().unwrap();
        let p = write_set(
            tmp.path(),
            &format!("{HEAD}\n[[run]]\nid = \"a\"\nprograms = [\"lz4\", \"libpng\"]\n"),
        );
        let (_s, runs) = load(&p).unwrap();
        let mut complete = ProgramEvalStats::new("lz4");
        complete.translation_success = true;
        complete.rust_build_success = true;
        write_record(&runs[0].output_root, &record_from_stats(&complete)).unwrap();

        let st = &status(&runs)[0];
        assert_eq!(st.done, 1);
        assert_eq!(st.total, 2);
        assert_eq!(st.missing, vec!["libpng"]);
        assert!(!st.is_complete());
    }

    #[test]
    fn malformed_record_is_treated_as_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("run");
        fs::create_dir_all(programs_dir(&root)).unwrap();
        fs::write(record_path(&root, "lz4"), "{ not json").unwrap();
        // Re-run rather than permanently skip.
        assert!(read_record(&root, "lz4").is_none());
    }

    #[test]
    fn schema_version_mismatch_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let p = write_set(
            tmp.path(),
            "schema_version = 99\nbench_root = \"bench\"\nresults_root = \"out\"\n\
             [[run]]\nid = \"a\"\nprograms = [\"lz4\"]\n",
        );
        let err = load(&p).unwrap_err().to_string();
        assert!(err.contains("schema_version"), "{err}");
    }

    #[test]
    fn paths_resolve_against_the_manifest_not_the_cwd() {
        let tmp = tempfile::tempdir().unwrap();
        let p = write_set(
            tmp.path(),
            &format!("{HEAD}\n[[run]]\nid = \"a\"\nprograms = [\"lz4\"]\n"),
        );
        let (_s, runs) = load(&p).unwrap();
        assert!(runs[0].input_root.starts_with(tmp.path()));
        assert!(runs[0].output_root.starts_with(tmp.path()));
    }
}
