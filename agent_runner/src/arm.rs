//! The arm of the study that a stage runs as. It decides everything that the framework gives the
//! agent besides the C source: the main prompt, the files in `inputs/` and in the workspace, and
//! the extras of the agent runtime.

use std::path::Path;

use harvest_core::config::Profile;

use crate::CaseDir;
use crate::prompt::{AdditionalPrompt, StageFact};

/// What a stage gives its agent.
#[derive(Clone, Copy)]
pub enum Arm<'a> {
    /// The HARVEST pipeline: the shared files of the study, the prompt files of every HARVEST
    /// stage, the main prompt that names the prompt file of the stage, and the compaction
    /// recovery of the agent runtime. The tool of the stage adds the files of its stage.
    Harvest {
        /// The prompt files that the options of the run add for the main agent. `run.md` lists
        /// them.
        additional_prompts: &'a [AdditionalPrompt],
        /// The facts of the stage that `run.md` gives after the common ones.
        stage_facts: &'a [StageFact<'a>],
    },
    /// A baseline arm: only what the [`Baseline`] lists, and the shared files of the study
    /// (see `shared`).
    Baseline(&'a Baseline),
}

impl<'a> Arm<'a> {
    pub fn profile(&self) -> Profile {
        match self {
            Arm::Harvest { .. } => Profile::Harvest,
            Arm::Baseline(baseline) => baseline.profile,
        }
    }

    /// The baseline arm, when the stage runs as one.
    pub(crate) fn baseline(&self) -> Option<&'a Baseline> {
        match self {
            Arm::Harvest { .. } => None,
            Arm::Baseline(baseline) => Some(baseline),
        }
    }
}

/// One stage of a baseline arm: everything that the framework gives its agent besides the C
/// source and the shared files of the study. The framework writes nothing else for the
/// agent.
pub struct Baseline {
    pub profile: Profile,
    /// The whole prompt of the main agent.
    pub prompt: String,
    /// The prompt files of the arm: the path in `inputs/prompts/` and the text.
    pub prompt_files: Vec<(String, String)>,
    /// The skills of the arm: directories of `agent_skills/`, written to
    /// `inputs/agent_skills/<name>/`.
    pub skills: Vec<&'static str>,
    /// The files that the framework creates in the workspace: the path and the text.
    pub workspace_files: Vec<(String, String)>,
    /// Runs when the workspace is ready (git repository, links, prompt record), immediately
    /// before the agent starts. An error fails the stage.
    pub before_agent: Hook,
    /// Runs when the agent has ended, whatever its exit status, before the tool changes or
    /// freezes the workspace. An error fails the stage.
    pub after_agent: Hook,
}

/// A step of a baseline arm around the agent run.
pub type Hook = fn(&HookContext<'_>) -> Result<(), Box<dyn std::error::Error>>;

pub struct HookContext<'a> {
    pub case: &'a CaseDir,
    /// The Rust toolchain of the run: the `RUSTUP_TOOLCHAIN` of the agent.
    pub rust_toolchain: &'a str,
    /// The prompt record of the stage in the output (`.harvest/prompts/<stage>/`), when the run
    /// keeps one. A hook can add its own record files.
    pub record_dir: Option<&'a Path>,
}

/// The [`Hook`] of an arm that has nothing to do at that point.
pub fn no_hook(_: &HookContext<'_>) -> Result<(), Box<dyn std::error::Error>> {
    Ok(())
}
