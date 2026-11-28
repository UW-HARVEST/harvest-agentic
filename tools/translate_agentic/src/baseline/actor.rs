//! The actor arm: the translation prompts of the ACTOR system. The main prompt of the stage is
//! one file of `prompts/baseline/actor/`, as it is: the list of the shared files of the study,
//! then the text of the ACTOR prompt file for the shape of the program, with a few changes.
//! `prompts/baseline/actor/verbatim/` has the files of ACTOR without a change, and its
//! `README.md` names their source and the changes. The arm gives the agent no file, no skill
//! and no hook of its own.

use agent_runner::{Baseline, no_hook};
use build_project_spec::ProjectKind;
use harvest_core::config::Profile;
use serde::Deserialize;

use super::Program;

/// The prompts of the arm. For a library.
const LIBRARY: &str = include_str!("../../prompts/baseline/actor/translate-library.md");
/// For an executable.
const EXECUTABLE: &str = include_str!("../../prompts/baseline/actor/translate-executable.md");
/// For a library and an executable, with build options.
const SHARED: &str = include_str!("../../prompts/baseline/actor/translate-shared.md");
/// For a library and an executable, without build options.
const SHARED_NO_OPTIONS: &str =
    include_str!("../../prompts/baseline/actor/translate-no-features-shared.md");

/// What the translate stage gives the agent of the arm, for `program`.
pub(crate) fn stage(program: &Program) -> Result<Baseline, String> {
    Ok(Baseline {
        profile: Profile::Actor,
        prompt: prompt(program)?.to_owned(),
        prompt_files: Vec::new(),
        skills: Vec::new(),
        workspace_files: Vec::new(),
        before_agent: no_hook,
        after_agent: no_hook,
    })
}

/// The prompt for `program`. The artifacts of the program decide. For a library with an
/// executable, the build options that the test case declares decide between the two prompts.
fn prompt(program: &Program) -> Result<&'static str, String> {
    let (library, executable) = match program.kind {
        ProjectKind::Library => (true, false),
        ProjectKind::Executable => (false, true),
        // The Test-Corpus kind of a program with build configurations, whatever its test case
        // declares.
        ProjectKind::Configurable => return Ok(SHARED),
        ProjectKind::HarvestBench(spec) => (spec.lib.is_some(), spec.bin.is_some()),
    };
    match (library, executable) {
        (true, false) => Ok(LIBRARY),
        (false, true) => Ok(EXECUTABLE),
        (true, true) if declares_build_options(program.configuration)? => Ok(SHARED),
        (true, true) => Ok(SHARED_NO_OPTIONS),
        (false, false) => Err(
            "the actor baseline has no prompt for a program that has no library and no \
             executable"
                .to_owned(),
        ),
    }
}

/// Whether the test case declares build options: its `configuration.json` has an entry in
/// `configurable_variables`.
fn declares_build_options(configuration: Option<&[u8]>) -> Result<bool, String> {
    #[derive(Deserialize)]
    struct Configuration {
        configurable_variables: serde_json::Map<String, serde_json::Value>,
    }
    let Some(configuration) = configuration else {
        return Ok(false);
    };
    let configuration: Configuration = serde_json::from_slice(configuration)
        .map_err(|e| format!("configuration.json of the test case: {e}"))?;
    Ok(!configuration.configurable_variables.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::baseline::tests::check_arm;
    use crate::tests::{LIB_AND_BIN, LIB_ONLY, harvest_bench};

    const BIN_ONLY: &str = r#"{"bin": {"name": "toy"}}"#;
    /// The configuration file of a test case that declares one build option, and of one that
    /// declares none.
    const BUILD_OPTIONS: &str = r#"{"configurable_variables": {"WITH_X": [true, false]}}"#;
    const NO_OPTIONS: &str = r#"{"configurable_variables": {}}"#;

    /// The words of the HARVEST method that are also the method of ACTOR, in its prompts for a
    /// library with an executable: "no stubs, no placeholders, no empty functions", and
    /// "breaking the translation into subtasks".
    const SHARED_WORDS: [&str; 2] = ["stub", "subtask"];

    #[test]
    #[cfg_attr(miri, ignore)]
    fn the_agent_receives_only_the_prompt_and_the_shared_files() {
        for (spec, configuration, own_words) in [
            (LIB_ONLY, None, &[][..]),
            (BIN_ONLY, None, &[][..]),
            (LIB_AND_BIN, None, &SHARED_WORDS[..]),
            (LIB_AND_BIN, Some(BUILD_OPTIONS), &SHARED_WORDS[..]),
        ] {
            let program = Program {
                kind: &harvest_bench(spec),
                configuration: configuration.map(str::as_bytes),
            };
            check_arm(Profile::Actor, &program, &[], own_words);
            // No word is allowed that the prompt does not have.
            let prompt = prompt(&program).unwrap().to_lowercase();
            for word in own_words {
                assert!(prompt.contains(word), "{spec}: {word}");
            }
        }
    }

    #[test]
    fn the_artifacts_and_the_build_options_of_the_program_select_the_prompt() {
        let chosen = |kind: &ProjectKind, configuration: Option<&str>| {
            prompt(&Program {
                kind,
                configuration: configuration.map(str::as_bytes),
            })
        };
        for (program, configuration, expected) in [
            (LIB_ONLY, None, LIBRARY),
            (LIB_ONLY, Some(BUILD_OPTIONS), LIBRARY),
            (BIN_ONLY, None, EXECUTABLE),
            (BIN_ONLY, Some(BUILD_OPTIONS), EXECUTABLE),
            (LIB_AND_BIN, None, SHARED_NO_OPTIONS),
            (LIB_AND_BIN, Some(NO_OPTIONS), SHARED_NO_OPTIONS),
            (LIB_AND_BIN, Some(BUILD_OPTIONS), SHARED),
        ] {
            assert_eq!(
                chosen(&harvest_bench(program), configuration),
                Ok(expected),
                "{program} {configuration:?}"
            );
        }
        assert_eq!(
            chosen(&ProjectKind::Library, Some(BUILD_OPTIONS)),
            Ok(LIBRARY)
        );
        assert_eq!(chosen(&ProjectKind::Executable, None), Ok(EXECUTABLE));
        assert_eq!(chosen(&ProjectKind::Configurable, None), Ok(SHARED));
        // No silent choice for a program or a declaration that the arm cannot read.
        let err = chosen(&harvest_bench("{}"), None).unwrap_err();
        assert!(err.contains("no library and no executable"), "{err}");
        for malformed in ["{", "{}", r#"{"configurable_variables": []}"#] {
            let err = chosen(&harvest_bench(LIB_AND_BIN), Some(malformed)).unwrap_err();
            assert!(
                err.starts_with("configuration.json of the test case: "),
                "{err}"
            );
        }
    }

    /// The copies in `verbatim/` are the files of ACTOR, byte for byte: nobody corrects, formats
    /// or shortens them. In the ACTOR repository,
    /// `git show IEEE-Privacy-and-Security:prompts/claude/<file> | sha256sum` prints each value.
    #[test]
    fn the_verbatim_copies_are_the_files_of_actor() {
        use sha2::{Digest, Sha256};
        macro_rules! verbatim {
            ($file:literal, $sha256:literal) => {
                (
                    $file,
                    include_str!(concat!("../../prompts/baseline/actor/verbatim/", $file)),
                    $sha256,
                )
            };
        }
        for (file, text, sha256) in [
            verbatim!(
                "translate-library.md",
                "1590f58e445880f4f902366ce6712803300f731ee9e08d33d2d9647f789d90ff"
            ),
            verbatim!(
                "translate-executable.md",
                "7c1c643e4aeb3be5bb6a45149eb0b7f770949de7beea711c6b6dd9cafbc8e2ef"
            ),
            verbatim!(
                "translate-shared.md",
                "f3b86c254ab51f2f633403190973ae3fed2e974a0a9bbf8aa6ff64e7a31e0a4a"
            ),
            verbatim!(
                "translate-no-features-shared.md",
                "9391ddc0b1e137a8dfab5d2d7bd5e212af4fb724285db20f8592ae118bd740f7"
            ),
        ] {
            let found = format!("{:x}", Sha256::digest(text));
            assert_eq!(found, sha256, "verbatim/{file} is not the file of ACTOR");
        }
    }
}
