//! The baseline arms of the verify stage. Each arm that has this stage has a module with one
//! function, `stage`, which returns everything that the stage gives the agent of the arm (see
//! [`agent_runner::Baseline`]).

pub(crate) mod actor;

#[cfg(test)]
pub(crate) mod tests {
    use agent_runner::AgentPhase;
    use agent_runner::prompt_check::check_baseline;
    use harvest_core::config::{AgentKind, Profile};

    use crate::tests::{CRATE, prepared_stage};

    /// The test of the allow-list of an arm. Prepares the verify stage of `profile` with the
    /// code of the tool, for each agent runtime, and checks everything that the agent receives
    /// ([`check_baseline`]):
    /// - `arm_entries`: the files of the arm, by their path in the run directory. The agent
    ///   gets exactly these, the crate of the earlier stage, and what the framework gives every
    ///   baseline.
    /// - `own_words`: the words of the HARVEST method that are also the own method of the arm.
    pub(crate) fn check_arm(profile: Profile, arm_entries: &[&str], own_words: &[&str]) {
        let config = serde_json::from_value(serde_json::json!({})).unwrap();
        let entries = [&CRATE[..], arm_entries].concat();
        for agent in [AgentKind::Claude, AgentKind::OpenCode] {
            let received = prepared_stage(profile, agent, &config, |_| ());
            let problems = check_baseline(
                &received,
                profile,
                AgentPhase::Verify,
                agent,
                &entries,
                own_words,
            );
            assert_eq!(problems, [""; 0], "{profile} {agent}");
        }
    }
}
