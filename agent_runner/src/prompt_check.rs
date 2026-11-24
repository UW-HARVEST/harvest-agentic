//! Checks of the prompt files that a stage writes into `inputs/prompts/`, for the tests of the
//! tools: every file that a prompt file names exists, the "read first" lists make no cycle, no
//! placeholder is left, and no rule has two copies. Test code only: the module exists in the
//! tests of `agent_runner` and with its feature `test-support`, which the tools turn on only for
//! their tests.
//!
//! A prompt file tells its reader which files to read first in a list after the line
//! [`READ_FIRST`]. Every other path that it names is a file that some reader needs later, for
//! example a role file in the role table of a main agent.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::Path;

use crate::CaseDir;
use crate::prompt::PROMPTS_PATH;

/// The line that starts the list of the files that the reader of a prompt file reads first.
pub const READ_FIRST: &str = "Read these files first:";

const SKILLS_PATH: &str = "inputs/agent_skills/";

/// The smallest text block, in lines without the empty ones, that counts as a second copy.
const DUPLICATE_LINES: usize = 3;

/// The environment variable that names a directory for [`dump`].
pub const DUMP_VAR: &str = "HARVEST_PROMPT_DUMP";

/// The placeholders (`{UPPER_CASE}`) that `text` has. The tests use it to make sure that a file
/// for the agents has none.
pub fn placeholders(text: &str) -> Vec<&str> {
    let mut found = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find('{') {
        rest = &rest[start..];
        let name_len = rest[1..]
            .find(|c: char| !(c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_'))
            .unwrap_or(rest.len() - 1);
        if name_len > 0 && rest[1 + name_len..].starts_with('}') {
            found.push(&rest[..name_len + 2]);
        }
        rest = &rest[1..];
    }
    found
}

/// The prompt files of a run: their text by their path in `inputs/prompts/`.
pub type PromptFiles = BTreeMap<String, String>;

/// For a person who wants to read what the agents of a run get: when [`DUMP_VAR`] names a
/// directory, writes the files of `case` into `<directory>/<name>/`: the prompt of the main agent
/// (`PROMPT.md`), `inputs/prompts/`, and the state files `state` of the workspace. The tests of
/// the tools call it for each kind of run that they prepare.
pub fn dump(case: &CaseDir, name: &str, prompt: &str, state: &[&str]) -> io::Result<()> {
    let Some(root) = std::env::var_os(DUMP_VAR) else {
        return Ok(());
    };
    let dir = Path::new(&root).join(name);
    fs::create_dir_all(&dir)?;
    fs::write(dir.join("PROMPT.md"), prompt)?;
    for (file, text) in load(&case.inputs())? {
        let path = dir.join(PROMPTS_PATH).join(file);
        fs::create_dir_all(path.parent().expect("a file has a parent"))?;
        fs::write(path, text)?;
    }
    for file in state {
        let from = case.workspace().join(file);
        if from.exists() {
            fs::copy(from, dir.join(file))?;
        }
    }
    Ok(())
}

/// Reads all files of `<inputs>/prompts/`.
pub fn load(inputs: &Path) -> io::Result<PromptFiles> {
    fn walk(root: &Path, dir: &Path, files: &mut PromptFiles) -> io::Result<()> {
        for entry in fs::read_dir(dir)? {
            let path = entry?.path();
            if path.is_dir() {
                walk(root, &path, files)?;
            } else {
                let name = path.strip_prefix(root).expect("file outside its root");
                files.insert(
                    name.to_string_lossy().into_owned(),
                    fs::read_to_string(&path)?,
                );
            }
        }
        Ok(())
    }
    let mut files = PromptFiles::new();
    let root = inputs.join("prompts");
    walk(&root, &root, &mut files)?;
    Ok(files)
}

/// The paths after `prefix` that `text` names: the longest run of path characters, without a
/// final period.
fn paths_after<'a>(text: &'a str, prefix: &str) -> Vec<&'a str> {
    text.match_indices(prefix)
        .map(|(start, _)| {
            let rest = &text[start + prefix.len()..];
            let end = rest
                .find(|c: char| !(c.is_ascii_alphanumeric() || "_-/.".contains(c)))
                .unwrap_or(rest.len());
            rest[..end].trim_end_matches('.')
        })
        .collect()
}

/// The prompt files that `text` names, by their path in `inputs/prompts/`. A path with a part to
/// fill in (`inputs/prompts/<stage>/main.md`) is not a name.
pub fn references(text: &str) -> Vec<&str> {
    paths_after(text, &format!("{PROMPTS_PATH}/"))
        .into_iter()
        .filter(|path| path.ends_with(".md"))
        .collect()
}

/// The lines of the "read first" list of `text`.
fn read_first_lines(text: &str) -> Vec<&str> {
    text.lines()
        .skip_while(|line| line.trim() != READ_FIRST)
        .skip(1)
        .skip_while(|line| line.trim().is_empty())
        .take_while(|line| line.starts_with("- "))
        .collect()
}

/// The prompt files that the reader of `text` reads first.
pub fn read_first(text: &str) -> Vec<&str> {
    read_first_lines(text)
        .into_iter()
        .flat_map(references)
        .collect()
}

/// `entry` and every file that its reader must read: the files of its "read first" list, their
/// lists, and so on.
pub fn must_read(files: &PromptFiles, entry: &str) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    let mut open = vec![entry.to_owned()];
    while let Some(file) = open.pop() {
        if let Some(text) = files.get(&file)
            && found.insert(file)
        {
            open.extend(read_first(text).into_iter().map(str::to_owned));
        }
    }
    found
}

/// The files that the reader of `entry` must read and that the "read first" list of `entry` does
/// not name itself. The file that a task message names for a sub-agent (its role file, or
/// `subagent.md`) must leave none: sub-agents read the files of that list, but they do not
/// reliably go on to the list of a file that they read.
pub fn not_listed(files: &PromptFiles, entry: &str) -> Vec<String> {
    let listed = files.get(entry).map(|text| read_first(text));
    must_read(files, entry)
        .into_iter()
        .filter(|file| file != entry && !listed.iter().flatten().any(|named| named == file))
        .collect()
}

/// A cycle of "read first" lists that starts at `file`, if there is one.
fn cycle(files: &PromptFiles, file: &str, path: &mut Vec<String>) -> Option<String> {
    if path.iter().any(|seen| seen == file) {
        return Some(format!("{} -> {file}", path.join(" -> ")));
    }
    path.push(file.to_owned());
    let found = files.get(file).and_then(|text| {
        read_first(text)
            .into_iter()
            .find_map(|next| cycle(files, next, path))
    });
    path.pop();
    found
}

/// The text of a prompt file as the lines that can be a second copy of a rule: without the empty
/// lines, the "read first" list, table rules and code fences.
fn rule_lines(text: &str) -> Vec<&str> {
    let list = read_first_lines(text);
    text.lines()
        .filter(|line| !list.contains(line))
        .map(str::trim)
        .filter(|line| {
            !line.is_empty()
                && *line != READ_FIRST
                && !line.starts_with("```")
                && !line.starts_with("|--")
        })
        .collect()
}

/// The problems of the prompt files in `<inputs>/prompts/`, one line each:
/// - a placeholder that is not filled in,
/// - a prompt file or a skill that a file names and that the run does not have,
/// - a cycle of "read first" lists,
/// - a block of [`DUPLICATE_LINES`] lines that two files have.
pub fn problems(inputs: &Path) -> io::Result<Vec<String>> {
    let files = load(inputs)?;
    let mut problems = Vec::new();
    for (name, text) in &files {
        for placeholder in placeholders(text) {
            problems.push(format!("{name}: placeholder {placeholder}"));
        }
        for path in references(text) {
            if !files.contains_key(path) {
                problems.push(format!("{name}: names the missing file {path}"));
            }
        }
        for path in paths_after(text, SKILLS_PATH) {
            let skill = path.split('/').next().unwrap_or_default();
            if !skill.is_empty() && !inputs.join("agent_skills").join(skill).is_dir() {
                problems.push(format!("{name}: names the missing skill {skill}"));
            }
        }
        if let Some(cycle) = cycle(&files, name, &mut Vec::new()) {
            problems.push(format!("read-first cycle: {cycle}"));
        }
    }
    let mut blocks: BTreeMap<Vec<&str>, &str> = BTreeMap::new();
    for (name, text) in &files {
        let lines = rule_lines(text);
        let mut reported_to = 0;
        for (start, block) in lines.windows(DUPLICATE_LINES).enumerate() {
            match blocks.get(block) {
                Some(other) if *other != name => {
                    if start >= reported_to {
                        problems.push(format!(
                            "{name} and {other} have the same text: {:?}",
                            block[0]
                        ));
                    }
                    // One report for a long block.
                    reported_to = start + DUPLICATE_LINES;
                }
                Some(_) => {}
                None => {
                    blocks.insert(block.to_vec(), name);
                }
            }
        }
    }
    Ok(problems)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (name, text) in files {
            let path = dir.path().join("prompts").join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }
        dir
    }

    #[test]
    fn references_are_the_named_prompt_files() {
        let text = "Read `inputs/prompts/a/b_c.md`. See inputs/prompts/d.md. Not \
                    `inputs/prompts/<stage>/main.md`, not inputs/prompts/ alone.";
        assert_eq!(references(text), ["a/b_c.md", "d.md"]);
    }

    #[test]
    fn read_first_is_only_the_list_after_its_line() {
        let text = "# T\n\nRead these files first:\n\n- `inputs/prompts/a.md`\n\
                    - `inputs/prompts/s/b.md`\n\nLater: `inputs/prompts/c.md`.\n\
                    - `inputs/prompts/d.md`\n";
        assert_eq!(read_first(text), ["a.md", "s/b.md"]);
        assert!(read_first("See `inputs/prompts/c.md`.").is_empty());
    }

    #[test]
    fn must_read_follows_the_lists() {
        let dir = inputs(&[
            (
                "s/main.md",
                "Read these files first:\n- `inputs/prompts/m.md`\n\nRole: `inputs/prompts/s/r.md`",
            ),
            ("m.md", "Read these files first:\n- `inputs/prompts/c.md`\n"),
            ("c.md", "text"),
            ("s/r.md", "text"),
        ]);
        let files = load(dir.path()).unwrap();
        let read: Vec<String> = must_read(&files, "s/main.md").into_iter().collect();
        assert_eq!(read, ["c.md", "m.md", "s/main.md"]);
        // main.md leaves c.md to the list of m.md; m.md names all that its reader must read.
        assert_eq!(not_listed(&files, "s/main.md"), ["c.md"]);
        assert_eq!(not_listed(&files, "m.md"), [""; 0]);
        assert_eq!(problems(dir.path()).unwrap(), [""; 0]);
    }

    #[test]
    fn problems_are_reported() {
        let block = "one rule line\nsecond rule line\nthird rule line\n";
        let dir = inputs(&[
            (
                "a.md",
                &format!(
                    "Read these files first:\n- `inputs/prompts/b.md`\n\n{block}\n{{LEFT}} \
                     `inputs/prompts/none.md` `inputs/agent_skills/no_skill/SKILL.md`"
                ),
            ),
            (
                "b.md",
                &format!("Read these files first:\n- `inputs/prompts/a.md`\n\n{block}"),
            ),
        ]);
        let found = problems(dir.path()).unwrap().join("\n");
        for problem in [
            "a.md: placeholder {LEFT}",
            "a.md: names the missing file none.md",
            "a.md: names the missing skill no_skill",
            "read-first cycle: a.md -> b.md -> a.md",
            "b.md and a.md have the same text: \"one rule line\"",
        ] {
            assert!(found.contains(problem), "{problem} not in:\n{found}");
        }
    }

    #[test]
    fn placeholders_are_upper_case_names_in_braces() {
        assert_eq!(
            placeholders("a {X} b {GIT_DISCIPLINE}{} {x} {A1}c { B } `{`"),
            ["{X}", "{GIT_DISCIPLINE}", "{A1}"]
        );
        assert!(placeholders("fn f() { EXPECT_EQ(RunC(kMode, {}), 1); }").is_empty());
    }
}
