//! The vanilla arm: the agent with its native ability. Its whole method is one task sentence.
//! The arm gives the agent no file, no skill and no hook of its own.

use agent_runner::{Baseline, no_hook, shared_reading_list};
use harvest_core::config::Profile;

use super::Program;

/// The task sentence.
const TASK: &str = include_str!("../../prompts/baseline/vanilla.md");

/// The main prompt is the list of the shared files of the study and the task sentence.
pub(crate) fn stage(_program: &Program) -> Result<Baseline, String> {
    let shared = shared_reading_list();
    Ok(Baseline {
        profile: Profile::Vanilla,
        prompt: format!("{shared}\n{TASK}"),
        prompt_files: Vec::new(),
        skills: Vec::new(),
        workspace_files: Vec::new(),
        before_agent: no_hook,
        after_agent: no_hook,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::baseline::tests::{check_arm, kinds};

    #[test]
    #[cfg_attr(miri, ignore)]
    fn the_agent_receives_only_the_task_and_the_shared_files() {
        for kind in kinds() {
            let program = Program {
                kind: &kind,
                configuration: None,
            };
            check_arm(Profile::Vanilla, &program, &[], &[]);
        }
    }

    #[test]
    fn the_main_prompt_is_the_reading_list_and_the_task_sentence() {
        let kind = kinds().into_iter().next().unwrap();
        let program = Program {
            kind: &kind,
            configuration: None,
        };
        assert_eq!(
            stage(&program).unwrap().prompt,
            "Read these files first:\n\
             \n\
             - `inputs/prompts/output_spec.md`\n\
             - `inputs/prompts/environment.md`\n\
             - `inputs/prompts/anti_cheating.md`\n\
             \n\
             Translate the C project in `inputs/c_src` to Rust.\n"
        );
    }
}
