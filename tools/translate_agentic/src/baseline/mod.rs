//! The baseline arms of the translate stage. Each arm has a module with one function, `stage`,
//! which returns everything that the stage gives the agent of the arm (see
//! [`agent_runner::Baseline`]): `stage(program)`, for the [`Program`] of the run. An error of
//! `stage` stops the stage before the run directory exists.

use build_project_spec::ProjectKind;
use harvest_core::fs::RawDir;

pub(crate) mod actor;
pub(crate) mod c2rust;
pub(crate) mod vanilla;

/// The file of a test case that declares its build options, in the C source.
pub(crate) const CONFIGURATION_FILE: &str = "configuration.json";

/// What the framework knows about the program of a run.
pub(crate) struct Program<'a> {
    /// What the translation must produce.
    pub(crate) kind: &'a ProjectKind,
    /// The text of [`CONFIGURATION_FILE`], when the test case has one.
    pub(crate) configuration: Option<&'a [u8]>,
}

impl<'a> Program<'a> {
    /// The program of `kind` whose C source is `source`.
    pub(crate) fn new(kind: &'a ProjectKind, source: &'a RawDir) -> Self {
        Program {
            kind,
            configuration: source.get_file(CONFIGURATION_FILE).ok(),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::Program;
    use agent_runner::AgentPhase;
    use agent_runner::prompt_check::{Received, check_baseline};
    use build_project_spec::ProjectKind;
    use harvest_core::config::{AgentKind, Profile};

    use crate::tests::{LIB_AND_BIN, LIB_ONLY, harvest_bench, prepared_stage};

    /// The kinds of the bench programs: a library, and a library with an executable.
    pub(crate) fn kinds() -> [ProjectKind; 2] {
        [harvest_bench(LIB_ONLY), harvest_bench(LIB_AND_BIN)]
    }

    /// The test of the allow-list of an arm. Prepares the translate stage of `profile` for
    /// `program` with the code of the tool, for each agent runtime, and checks everything that
    /// the agent receives ([`check_baseline`]):
    /// - `arm_entries`: the files of the arm, by their path in the run directory (for example
    ///   `inputs/agent_skills/c2rust/SKILL.md`). The agent gets exactly these and what the
    ///   framework gives every baseline.
    /// - `own_words`: the words of the HARVEST method that are also the own method of the arm.
    pub(crate) fn check_arm(
        profile: Profile,
        program: &Program,
        arm_entries: &[&str],
        own_words: &[&str],
    ) {
        let stage = |agent| prepared_stage(profile, agent, program, serde_json::json!({}), |_| ());
        check_received(profile, program, stage, arm_entries, own_words);
    }

    /// [`check_arm`] on what `received` gives for each agent runtime: for an arm whose `stage`
    /// needs a program of the host, with the stage that the arm builds without it.
    pub(crate) fn check_received(
        profile: Profile,
        program: &Program,
        received: impl Fn(AgentKind) -> Received,
        arm_entries: &[&str],
        own_words: &[&str],
    ) {
        let kind = program.kind;
        for agent in [AgentKind::Claude, AgentKind::OpenCode] {
            let received = received(agent);
            let problems = check_baseline(
                &received,
                profile,
                AgentPhase::Translate,
                agent,
                arm_entries,
                own_words,
            );
            assert_eq!(problems, [""; 0], "{profile} {agent} {kind}");
        }
    }
}
