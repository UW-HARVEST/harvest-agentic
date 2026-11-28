//! The texts that are the same for every arm of the study, the HARVEST pipeline included: the
//! output specification, the facts of the environment, and the anti-cheating rules. Each
//! sentence has one source file (`prompts/shared/`), and every stage of every arm gets the
//! three files. A text that tells an agent how to work does not belong here: it is the method
//! of an arm.

use crate::prompt::{PROMPTS_PATH, READ_FIRST};

/// The shared files, by their name in `inputs/prompts/`, in the order of
/// [`shared_reading_list`]: what the result must be, the facts of the environment, and what
/// makes a result something other than a translation by the arm.
pub(crate) const FILES: [(&str, &str); 3] = [
    (
        "output_spec.md",
        include_str!("../prompts/shared/output_spec.md"),
    ),
    (
        "environment.md",
        include_str!("../prompts/shared/environment.md"),
    ),
    (
        "anti_cheating.md",
        include_str!("../prompts/shared/anti_cheating.md"),
    ),
];

/// The text that points an agent at the shared files: the line "Read these files first:" and
/// one line for each file. The main prompt of a baseline arm starts with it.
pub fn shared_reading_list() -> String {
    let mut list = format!("{READ_FIRST}\n\n");
    for (name, _) in FILES {
        list.push_str(&format!("- `{PROMPTS_PATH}/{name}`\n"));
    }
    list
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reading_list_names_each_shared_file() {
        assert_eq!(
            shared_reading_list(),
            "Read these files first:\n\n\
             - `inputs/prompts/output_spec.md`\n\
             - `inputs/prompts/environment.md`\n\
             - `inputs/prompts/anti_cheating.md`\n"
        );
    }
}
