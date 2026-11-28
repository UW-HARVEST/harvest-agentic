//! The snapshot layout and its stage manifest.
//!
//! Every run writes each program's result as a **self-contained snapshot** —
//! the agentic stages and the direct-LLM translators alike, since this is the
//! framework's one output format: the translated crate at the top level, and everything the
//! framework knows about the run under a single `.harvest/` directory,
//! including the read-only reference inputs. A later stage resumes from the
//! snapshot alone — the bench test case does not have to still be in place, or
//! unchanged.
//!
//! ```text
//! out_x/<prog>/
//!   Cargo.toml  Cargo.lock  src/  tests/  …   the crate; == the frozen CargoPackage,
//!                                             with the agents' reports (PLAN.md, ...)
//!   target/                                   cargo build output
//!   translated_rust/  translated_tools/       cando2 grading output (not part of the crate)
//!   .harvest/
//!     stage.json                              this manifest
//!     c_src/                                  C source (RawSource), rewritten every stage
//!     suite/                                  external tests, rewritten every stage
//!     reference_modified/<stage>/<name>       read-only inputs an agent changed (audit)
//!     prompts/<stage>/                        what the agents of the stage got: PROMPT.md, a
//!                                             copy of the run's inputs/prompts/, given.json
//!                                             (the profile, the command line of the agent,
//!                                             each entry of the run directory with its hash),
//!                                             files/ (a copy of each other given file)
//!     results.err
//!     quality.json  unsafe_ops.tsv            code-quality report (benchmark)
//! ```
//!
//! The layout *is* the protocol: the package is everything at the top level
//! that [`is_reserved_toplevel`] does not claim, so anything dropped into the
//! crate directory is part of the crate, and no list has to be kept in sync
//! with it. That leaves the manifest purely informational — provenance for
//! experiments, plus the one environment reference (the Test-Corpus checkout)
//! that genuinely cannot travel with a snapshot.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::{Profile, Stage};

/// Directory under a snapshot program directory holding everything the
/// framework owns. Excluded from the frozen package.
pub const HARVEST_META_DIR: &str = ".harvest";

/// Cargo's build output directory. Excluded from the frozen package.
pub const BUILD_DIR: &str = "target";

/// The manifest file, inside [`HARVEST_META_DIR`].
pub const STAGE_MANIFEST_FILE: &str = "stage.json";

/// C source directory inside [`HARVEST_META_DIR`].
pub const C_SOURCE_DIR: &str = "c_src";

/// External test suite directory inside [`HARVEST_META_DIR`]. Its own children
/// are the suite directories (`gtest_suite/`, or `runner/` + `test_vectors/`).
pub const SUITE_DIR: &str = "suite";

/// Audit record of the read-only inputs an agent changed: one file per input, under a directory
/// per stage.
pub const REFERENCE_MODIFIED_DIR: &str = "reference_modified";

/// Record of what the agents of each stage were given: under a directory per stage, the prompt of
/// the main agent, a copy of the prompt files of the run, and the list of everything that the
/// agent found in its run directory.
pub const PROMPTS_DIR: &str = "prompts";

/// Every directory name that can constitute an external test suite. The
/// authoritative copy lives under `.harvest/suite/`, but the cando2 grader
/// stages `runner/` and `test_vectors/` next to the crate while it runs, so
/// these names are reserved at the top level: external test material is never
/// part of the crate, whoever put it there. Reserving them is what stops a
/// grading artifact from being picked up as crate content by a later stage —
/// which, for a suite that rounds 1 and 2 hold out, would be a leak.
pub const SUITE_DIR_NAMES: &[&str] = &["gtest_suite", "runner", "test_vectors"];

/// The directories that the cando2 grader writes next to the crate: the compiled library and
/// its own sources. Reserved for the same reason as [`SUITE_DIR_NAMES`].
pub const GRADING_ARTIFACT_NAMES: &[&str] = &["translated_rust", "translated_tools"];

/// Returns the `.harvest/` directory of a snapshot program directory.
pub fn meta_dir(program_dir: &Path) -> PathBuf {
    program_dir.join(HARVEST_META_DIR)
}

/// The file in which a baseline arm keeps the summary of a stage for the manifest: a JSON value
/// in the directory of the arm in the prompt record of the stage (`prompt_record_dir`). The arm
/// writes it when the agent has ended.
pub fn arm_summary_path(prompt_record_dir: &Path, profile: Profile) -> PathBuf {
    prompt_record_dir
        .join(profile.to_string())
        .join("summary.json")
}

/// The summaries that the arm `profile` wrote for `stages` in the snapshot `program_dir`, by the
/// name of the stage.
pub fn arm_summaries(
    program_dir: &Path,
    profile: Profile,
    stages: &[Stage],
) -> BTreeMap<String, serde_json::Value> {
    let prompts = meta_dir(program_dir).join(PROMPTS_DIR);
    stages
        .iter()
        .filter_map(|stage| {
            let path = arm_summary_path(&prompts.join(stage.to_string()), profile);
            let summary = serde_json::from_str(&fs::read_to_string(path).ok()?).ok()?;
            Some((stage.to_string(), summary))
        })
        .collect()
}

/// Returns the C source directory carried by a snapshot.
pub fn c_source_dir(program_dir: &Path) -> PathBuf {
    meta_dir(program_dir).join(C_SOURCE_DIR)
}

/// Returns the external test suite directory carried by a snapshot.
pub fn suite_dir(program_dir: &Path) -> PathBuf {
    meta_dir(program_dir).join(SUITE_DIR)
}

/// True when `program_dir` is a pipeline snapshot (it carries a manifest).
pub fn is_snapshot(program_dir: &Path) -> bool {
    meta_dir(program_dir).join(STAGE_MANIFEST_FILE).is_file()
}

/// True when `name` is a top-level entry the frozen package excludes: the
/// framework's own directory, cargo's build output, any external test
/// material (see [`SUITE_DIR_NAMES`]), and what grading leaves behind (see
/// [`GRADING_ARTIFACT_NAMES`]).
pub fn is_reserved_toplevel(name: &std::ffi::OsStr) -> bool {
    [HARVEST_META_DIR, BUILD_DIR]
        .iter()
        .chain(SUITE_DIR_NAMES)
        .chain(GRADING_ARTIFACT_NAMES)
        .any(|reserved| name == std::ffi::OsStr::new(reserved))
}

/// Provenance metadata for a snapshot. Nothing here is required to load a
/// snapshot — the layout carries that — but `test_corpus_root` is used when
/// present, to spare a resumed run from re-deriving the toolchain contract
/// from a C source that no longer lives in the corpus.
///
/// The lineage fields describe where the snapshot's reference material came
/// from and are carried forward. `runs` is the history: one record for each
/// benchmark invocation that produced this snapshot, oldest first. A record
/// stores what that invocation ran with (command line, harvest build,
/// resolved tool configuration), so a new flag or a changed default needs
/// no new field here.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct StageManifest {
    /// Manifest schema version, bumped on incompatible changes.
    pub schema_version: u32,

    /// Name of the bench test case this lineage came from. Provenance only —
    /// a snapshot's own directory name carries it too.
    pub bench_program: String,

    /// Revision of the bench checkout the snapshot's reference material (its
    /// C source and test suite) was taken from. This is the part worth
    /// recording: test suites evolve, so it is what explains two runs of "the
    /// same" case disagreeing. Carried forward with the references, and
    /// updated when `--test-case` swaps in a different checkout's suite.
    #[serde(default)]
    pub bench_revision: Option<String>,

    /// The Test-Corpus / cando2 checkout the Rust toolchain contract was read
    /// from. Detected on the run that starts from the bench directory and
    /// carried forward, because it is the one input that cannot travel inside
    /// a snapshot — and so the only path here that is still resolved.
    #[serde(default)]
    pub test_corpus_root: Option<PathBuf>,

    /// The benchmark invocations that produced this snapshot, oldest first.
    #[serde(default)]
    pub runs: Vec<RunRecord>,
}

/// One benchmark invocation in a snapshot's history.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RunRecord {
    /// The agentic stages this invocation ran, in pipeline order. Empty for
    /// the non-agentic translators.
    pub stages: Vec<Stage>,

    /// What the agents of these stages were given: the HARVEST pipeline, or a baseline arm.
    #[serde(default)]
    pub profile: Profile,

    /// The command line, as given, and the directory it ran in. For people and
    /// for running it again; the effective settings are in `config`.
    pub argv: Vec<String>,
    pub cwd: PathBuf,

    /// Harvest build: `<sha>[*] (<date>)`, and for a build from a changed
    /// working tree (`*`) also `diff <hash of git diff HEAD>`.
    pub harvest_version: String,

    /// Revision of the Test-Corpus checkout as of this invocation, re-read
    /// every time rather than carried, since the checkout can move.
    #[serde(default)]
    pub test_corpus_revision: Option<String>,

    /// Read-only reference directories an agent modified during this
    /// invocation (see [`REFERENCE_MODIFIED_DIR`]). The change does not reach
    /// the output, but a non-empty list means the results deserve scrutiny — an
    /// agent that edits the C source is editing its own oracle.
    #[serde(default)]
    pub reference_modified: Vec<String>,

    /// What a baseline arm recorded about the result of its stages: for each stage, by its
    /// name, the summary that the arm wrote ([`arm_summary_path`]). Empty when the arm writes
    /// none.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub arm_summary: BTreeMap<String, serde_json::Value>,

    /// Unix timestamp of the invocation.
    pub created_unix: u64,

    /// The settings the invocation ran with: the agent, and the resolved
    /// configuration of each tool that ran (the tool's defaults filled in,
    /// secrets left out). `null` if it could not be resolved.
    pub config: serde_json::Value,
}

impl StageManifest {
    /// The stages of all runs, in run order (a stage run twice appears twice).
    pub fn stages(&self) -> Vec<Stage> {
        self.runs
            .iter()
            .flat_map(|r| r.stages.iter().copied())
            .collect()
    }

    /// The profile of the last run that had the translate stage: what made the crate of the
    /// snapshot. `None` when a non-agentic translator made it.
    pub fn translated_by(&self) -> Option<Profile> {
        self.runs
            .iter()
            .rev()
            .find(|run| run.stages.contains(&Stage::Translate))
            .map(|run| run.profile)
    }

    /// Refuses a stage of a baseline that would resume from this snapshot, unless the same
    /// baseline made all of it: its translate stage and every later run. A baseline arm must not
    /// work on the result of other means.
    pub fn check_resume(&self, profile: Profile) -> Result<(), String> {
        if !profile.is_baseline() {
            return Ok(());
        }
        let origin = match self.runs.iter().find(|run| run.profile != profile) {
            Some(run) if run.stages.is_empty() => "a non-agentic translator".to_owned(),
            Some(run) if run.profile == Profile::Harvest => "the HARVEST pipeline".to_owned(),
            Some(run) => format!("the {} baseline", run.profile),
            None if self.translated_by().is_none() => "no translate stage".to_owned(),
            None => return Ok(()),
        };
        Err(format!(
            "a stage of the {profile} baseline resumes only from a snapshot that only runs with \
             --baseline={profile} made, from the translate stage on; the history of this one \
             has {origin}"
        ))
    }

    /// Reads the manifest from a snapshot program directory.
    pub fn read_from_dir(program_dir: &Path) -> std::io::Result<Self> {
        let path = meta_dir(program_dir).join(STAGE_MANIFEST_FILE);
        let contents = fs::read_to_string(&path)?;
        serde_json::from_str(&contents)
            .map_err(|e| std::io::Error::other(format!("{}: {e}", path.display())))
    }

    /// Writes the manifest into a snapshot program directory.
    pub fn write_to_dir(&self, program_dir: &Path) -> std::io::Result<()> {
        let dir = meta_dir(program_dir);
        fs::create_dir_all(&dir)?;
        let contents =
            serde_json::to_string_pretty(self).map_err(|e| std::io::Error::other(e.to_string()))?;
        fs::write(dir.join(STAGE_MANIFEST_FILE), contents)
    }
}

#[cfg(all(not(miri), test))]
mod tests {
    use super::*;

    fn run(stages: Vec<Stage>) -> RunRecord {
        RunRecord {
            stages,
            profile: Profile::Harvest,
            argv: vec!["benchmark".to_owned()],
            cwd: PathBuf::from("/repo"),
            harvest_version: "test".to_owned(),
            test_corpus_revision: Some("def5678*".to_owned()),
            reference_modified: vec!["c_src".to_owned()],
            arm_summary: BTreeMap::new(),
            created_unix: 0,
            config: serde_json::json!({"agentic_agent": "opencode"}),
        }
    }

    fn manifest() -> StageManifest {
        StageManifest {
            schema_version: 2,
            bench_program: "lz4".to_owned(),
            bench_revision: Some("abc1234".to_owned()),
            test_corpus_root: Some(PathBuf::from("/repo/Test-Corpus")),
            runs: vec![
                run(vec![Stage::Translate]),
                run(vec![Stage::Verify]),
                run(vec![Stage::Verify]),
            ],
        }
    }

    #[test]
    fn roundtrip_through_the_meta_dir() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!is_snapshot(dir.path()));
        manifest().write_to_dir(dir.path()).unwrap();
        assert!(is_snapshot(dir.path()));
        let read = StageManifest::read_from_dir(dir.path()).unwrap();
        assert_eq!(
            read.stages(),
            [Stage::Translate, Stage::Verify, Stage::Verify]
        );
        assert_eq!(read.test_corpus_root, manifest().test_corpus_root);
        assert_eq!(read.bench_program, "lz4");
        assert_eq!(read.bench_revision.as_deref(), Some("abc1234"));
        assert_eq!(read.runs[0].reference_modified, ["c_src"]);
    }

    #[test]
    fn the_summary_of_an_arm_is_read_for_the_stages_that_have_one() {
        let dir = tempfile::tempdir().unwrap();
        let stages = [Stage::Translate, Stage::Verify];
        assert!(arm_summaries(dir.path(), Profile::C2rust, &stages).is_empty());
        let record = meta_dir(dir.path()).join(PROMPTS_DIR).join("translate");
        let path = arm_summary_path(&record, Profile::C2rust);
        assert!(path.ends_with(".harvest/prompts/translate/c2rust/summary.json"));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, r#"{"comparison": "made"}"#).unwrap();
        let summaries = arm_summaries(dir.path(), Profile::C2rust, &stages);
        assert_eq!(
            serde_json::json!(summaries),
            serde_json::json!({"translate": {"comparison": "made"}})
        );
        assert!(arm_summaries(dir.path(), Profile::Actor, &stages).is_empty());
    }

    #[test]
    fn a_baseline_resumes_only_from_a_snapshot_that_it_made_alone() {
        let of = |profile, stages| RunRecord {
            profile,
            ..run(stages)
        };
        let mut manifest = manifest();
        assert_eq!(manifest.translated_by(), Some(Profile::Harvest));
        manifest.check_resume(Profile::Harvest).unwrap();
        let err = manifest.check_resume(Profile::Actor).unwrap_err();
        assert!(err.ends_with("has the HARVEST pipeline"), "{err}");
        manifest.runs = vec![
            of(Profile::Actor, vec![Stage::Translate]),
            of(Profile::Actor, vec![Stage::Verify]),
        ];
        manifest.check_resume(Profile::Actor).unwrap();
        // A HARVEST stage can take any snapshot. After it, the snapshot is not the work of the
        // baseline alone.
        manifest.check_resume(Profile::Harvest).unwrap();
        manifest
            .runs
            .push(of(Profile::Harvest, vec![Stage::Verify]));
        assert_eq!(manifest.translated_by(), Some(Profile::Actor));
        let err = manifest.check_resume(Profile::Actor).unwrap_err();
        assert!(err.ends_with("has the HARVEST pipeline"), "{err}");
        manifest.runs = vec![of(Profile::Vanilla, vec![Stage::Translate])];
        let err = manifest.check_resume(Profile::Actor).unwrap_err();
        assert!(err.ends_with("has the vanilla baseline"), "{err}");
        // A snapshot of a non-agentic translator has no translate stage.
        manifest.runs = vec![run(Vec::new())];
        assert_eq!(manifest.translated_by(), None);
        let err = manifest.check_resume(Profile::Actor).unwrap_err();
        assert!(err.ends_with("has a non-agentic translator"), "{err}");
        manifest.runs = Vec::new();
        let err = manifest.check_resume(Profile::Actor).unwrap_err();
        assert!(err.ends_with("has no translate stage"), "{err}");
    }

    #[test]
    fn a_manifest_without_profile_is_a_harvest_run() {
        let mut record = serde_json::to_value(run(vec![Stage::Translate])).unwrap();
        record.as_object_mut().unwrap().remove("profile");
        let record: RunRecord = serde_json::from_value(record).unwrap();
        assert_eq!(record.profile, Profile::Harvest);
    }

    #[test]
    fn reserved_toplevel_entries() {
        for reserved in [
            ".harvest",
            "target",
            "gtest_suite",
            "runner",
            "test_vectors",
            "translated_rust",
            "translated_tools",
        ] {
            assert!(
                is_reserved_toplevel(std::ffi::OsStr::new(reserved)),
                "{reserved}"
            );
        }
        for owned in ["src", "Cargo.lock", "Cargo.toml", "build.rs", "benches"] {
            assert!(
                !is_reserved_toplevel(std::ffi::OsStr::new(owned)),
                "{owned}"
            );
        }
    }
}
