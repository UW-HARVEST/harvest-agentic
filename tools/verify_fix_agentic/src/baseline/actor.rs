//! The actor arm: the validation prompt of the ACTOR system. The main prompt of the stage is the
//! file `prompts/baseline/actor/verify.md`, as it is: the list of the shared files of the study,
//! then the text of the ACTOR prompt file, with a few changes. ACTOR has one validation prompt
//! for every shape of program. `prompts/baseline/actor/verbatim/` has the file of ACTOR without
//! a change, and its `README.md` names its source and the changes. The arm gives the agent no
//! file, no skill and no hook of its own: the session starts from the crate of the translate
//! stage and the C source.

use agent_runner::{Baseline, no_hook};
use harvest_core::config::Profile;

/// The prompt of the arm.
const PROMPT: &str = include_str!("../../prompts/baseline/actor/verify.md");

/// What the verify stage gives the agent of the arm.
pub(crate) fn stage() -> Baseline {
    Baseline {
        profile: Profile::Actor,
        prompt: PROMPT.to_owned(),
        prompt_files: Vec::new(),
        skills: Vec::new(),
        workspace_files: Vec::new(),
        before_agent: no_hook,
        after_agent: no_hook,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::baseline::tests::check_arm;

    /// The words of the HARVEST method that are also the method of ACTOR, in its validation
    /// prompt: "use `libloading` to load BOTH the C .so AND the Rust .so", "Compare `nm -D` on
    /// the C .so and the Rust .so", and "break the work into focused subtasks".
    const OWN_WORDS: [&str; 3] = ["libloading", "nm -d", "subtask"];

    /// The agent gets the crate of the translate stage, the prompt and the shared files: no
    /// test environment, no state file and no method file of the HARVEST verify stage.
    #[test]
    #[cfg_attr(miri, ignore)]
    fn the_agent_receives_only_the_crate_the_prompt_and_the_shared_files() {
        check_arm(Profile::Actor, &[], &OWN_WORDS);
        // No word is allowed that the prompt does not have.
        let prompt = PROMPT.to_lowercase();
        for word in OWN_WORDS {
            assert!(prompt.contains(word), "{word}");
        }
    }

    /// The copy in `verbatim/` is the file of ACTOR, byte for byte: nobody corrects, formats or
    /// shortens it. In the ACTOR repository,
    /// `git show IEEE-Privacy-and-Security:prompts/claude/verify.md | sha256sum` prints the value.
    #[test]
    fn the_verbatim_copy_is_the_file_of_actor() {
        use sha2::{Digest, Sha256};
        let verbatim = include_str!("../../prompts/baseline/actor/verbatim/verify.md");
        assert_eq!(
            format!("{:x}", Sha256::digest(verbatim)),
            "2c2bd1700f8ca5b58a7418b25e45884d94e2c393f1a1b1d503eb3c6a591fea0d",
            "verbatim/verify.md is not the file of ACTOR"
        );
    }
}
