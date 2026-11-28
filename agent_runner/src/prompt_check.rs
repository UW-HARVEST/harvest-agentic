//! Checks of what a stage gives its agents, for the tests of the tools. Test code only: the
//! module exists in the tests of `agent_runner` and with its feature `test-support`, which the
//! tools turn on only for their tests.
//!
//! The prompt files in `inputs/prompts/`: every file that a prompt file names exists, the "read
//! first" lists make no cycle, no placeholder is left, and no rule has two copies. A prompt file
//! tells its reader which files to read first in a list after the line [`READ_FIRST`]. Every
//! other path that it names is a file that some reader needs later, for example a role file in
//! the role table of a main agent.
//!
//! A baseline arm: [`check_baseline`] compares everything that the agent receives
//! ([`Received`]) with the allow-list of the arm, and looks for the words of the HARVEST method.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::Path;

use harvest_core::config::{AgentKind, Profile};

use crate::given::{self, GivenKind};
use crate::prompt::{PROMPTS_PATH, READ_FIRST};
use crate::{AgentPhase, PreparedAgent};

const SKILLS_PATH: &str = "inputs/agent_skills/";

/// The smallest text block, in lines without the empty ones, that counts as a second copy.
const DUPLICATE_LINES: usize = 3;

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

/// Everything that the agent of a prepared stage receives from the framework.
pub struct Received {
    /// The prompt of the main agent.
    pub prompt: String,
    /// The command line of the agent, without the prompt.
    pub argv: Vec<String>,
    /// Claude Code: the text that the framework appends to the system prompt.
    pub system_prompt: Option<String>,
    /// OpenCode: the prompt of a resume after a dead provider stream.
    pub resume_prompt: Option<String>,
    /// The environment variables that the framework sets on the agent process: the name and
    /// the value.
    pub env: Vec<(String, String)>,
    /// The path of the run directory. Many texts name it, and it has a random part.
    pub run_dir: String,
    /// Each entry of the run directory at the moment when the agent starts, by its path in the
    /// run directory, with the text of a file. The path of a directory ends with `/`: an empty
    /// one, and the C source and the git record, which are not listed further.
    pub files: BTreeMap<String, String>,
}

impl PreparedAgent<'_> {
    /// Starts the stage as [`PreparedAgent::run`] does, starts no agent, and returns what the
    /// agent receives: everything that the framework and the hook of a baseline arm wrote
    /// before the agent starts. Call it after
    /// [`CaseDir::lock_inputs`](crate::CaseDir::lock_inputs). The scripts of the command
    /// environment are written, but the memory limit of the host is not set.
    pub fn received(&self) -> Result<Received, Box<dyn std::error::Error>> {
        let invocation = &self.invocation;
        let case = invocation.case;
        let shell = crate::agent_env::write_scripts(case.root())?;
        let env = invocation.environment(Some(&shell), &self.limits)?;
        let started =
            crate::start_stage(invocation, &self.routes, Some(&shell), &self.earlier, &env)?;
        let mut files = BTreeMap::new();
        for entry in given::list(case)? {
            let text = match entry.kind {
                GivenKind::File { .. } => {
                    let content = fs::read(case.root().join(&entry.path))?;
                    String::from_utf8_lossy(&content).into_owned()
                }
                GivenKind::Link(_) | GivenKind::Directory => String::new(),
            };
            files.insert(entry.path, text);
        }
        let extras = invocation.runtime_extras()?;
        Ok(Received {
            prompt: started.prompt,
            argv: extras.argv,
            system_prompt: extras.system_prompt,
            resume_prompt: extras.resume_prompt,
            env: env.into_iter().map(|var| (var.name, var.value)).collect(),
            run_dir: case.root().to_string_lossy().into_owned(),
            files,
        })
    }
}

/// The name of the main prompt in a message of [`check_baseline`].
const MAIN_PROMPT: &str = "the main prompt";

impl Received {
    /// Each text with a name for a message: the files, the prompts of the agent runtime, the
    /// command line of the agent and its environment. The path of the run directory is not in
    /// the texts: its random part can make a word by chance.
    fn texts(&self) -> Vec<(&str, String)> {
        let prompts = [
            (MAIN_PROMPT, Some(&self.prompt)),
            ("the system prompt", self.system_prompt.as_ref()),
            ("the resume prompt", self.resume_prompt.as_ref()),
        ];
        let mut texts: Vec<(&str, String)> = self
            .files
            .iter()
            .map(|(path, text)| (&**path, text.clone()))
            .collect();
        for (name, text) in prompts {
            texts.extend(text.map(|text| (name, text.clone())));
        }
        texts.push(("the command line of the agent", self.argv.join(" ")));
        let env: Vec<String> = (self.env.iter())
            .map(|(name, value)| format!("{name}={value}"))
            .collect();
        texts.push(("the environment of the agent", env.join("\n")));
        for (_, text) in &mut texts {
            *text = text.replace(&self.run_dir, "<run directory>");
        }
        texts
    }
}

/// Words of the HARVEST method, in lower case. Nothing that a baseline agent receives has one,
/// except the texts of its arm, for a word that is also the own method of the arm (see
/// [`check_baseline`]).
pub const METHOD_WORDS: &[&str] = &[
    "plan.md",
    "hypothes",
    "quality.md",
    "conform.md",
    "state file",
    "plan file",
    "scout",
    "worktree",
    "wt/",
    "tab2rust",
    "nm -d",
    "completion gate",
    "smoke test",
    "stub",
    "subtask",
    "delegat",
    "parallel",
    "workflow",
    "role file",
    "task message",
    "reviewer",
    "compaction",
    "invariant",
    "fuzz",
    "verify_env",
    "gtest",
    "googletest",
    "libloading",
    "cargo_features",
    "main_agent",
    "subagent.md",
    "git_main",
    "git_subagent",
    "translation_rules",
    "verify_rules",
    "safe_wrapper",
    "safe api",
    "wishlist",
];

/// The entries that the run directory of every baseline stage has when the agent starts,
/// whatever its arm. For the agent: the C source, the shared files of the study, the
/// empty `shared/`, and in the workspace the git record, `.gitignore`, the links, `scratch/`
/// and the project of OpenCode. Of the framework, readable by the agent: the scripts of the
/// command environment, the empty log of the stage, `tmp/`, and the sandbox settings of Claude
/// Code. Written out here, and not taken from the code that writes them, so that a change of
/// that code shows as a difference.
fn framework_entries(phase: AgentPhase, agent: AgentKind) -> Vec<String> {
    let mut entries = vec![
        "agent_env/harvest-sandbox",
        "agent_env/harvest-shell",
        "agent_env/harvest-shell-inner",
        "inputs/c_src/",
        "inputs/prompts/anti_cheating.md",
        "inputs/prompts/environment.md",
        "inputs/prompts/output_spec.md",
        "shared/",
        "tmp/",
        "workspace/.git/",
        "workspace/.gitignore",
        "workspace/inputs",
        "workspace/scratch/",
        "workspace/shared",
    ];
    entries.push(match phase {
        AgentPhase::Verify => "logs/verify.log",
        _ => "logs/translation.log",
    });
    let mut entries: Vec<String> = entries.into_iter().map(str::to_owned).collect();
    match agent {
        AgentKind::Claude => entries.push(".claude/settings.json".to_owned()),
        AgentKind::OpenCode => entries.extend(
            [
                format!(".opencode/agents/harvest-{}.md", phase.prompt_dir()),
                ".opencode/opencode.json".to_owned(),
                ".opencode/plugin/harvest-resilience.js".to_owned(),
            ]
            .map(|path| format!("workspace/{path}")),
        ),
    }
    entries
}

/// The problems of what the agent of a baseline stage receives, one line each:
/// - an entry that is not on the allow-list, or an entry of the allow-list that is missing. The
///   allow-list is [`framework_entries`] and `arm_entries`: the files of the arm (paths in the
///   run directory, for example `inputs/prompts/actor/translate.md`), what its hook before the
///   agent writes and, for a stage that resumes, the crate of the earlier stage,
/// - a word of [`METHOD_WORDS`] in a file, in a prompt or in the command line of the agent.
///   `own_words` are the words that are the own method of the arm. They can be only in the
///   texts of the arm: the main prompt and the `arm_entries`. A text of the framework (a shared
///   file, a text of the agent runtime, the command line) has none,
/// - a main prompt that does not start with the list of the shared files, or that names a
///   prompt file that the run does not have,
/// - a problem of the prompt files ([`problems`]).
pub fn check_baseline(
    received: &Received,
    profile: Profile,
    phase: AgentPhase,
    agent: AgentKind,
    arm_entries: &[&str],
    own_words: &[&str],
) -> Vec<String> {
    let mut found = Vec::new();
    let framework = framework_entries(phase, agent);
    let allowed: BTreeSet<&str> = framework
        .iter()
        .map(|path| &**path)
        .chain(arm_entries.iter().copied())
        .collect();
    let given: BTreeSet<&str> = received.files.keys().map(|path| &**path).collect();
    for path in given.difference(&allowed) {
        found.push(format!(
            "{path}: not on the allow-list of the {profile} arm"
        ));
    }
    for path in allowed.difference(&given) {
        found.push(format!("{path}: on the allow-list, but not given"));
    }
    for (name, text) in received.texts() {
        let text = text.to_lowercase();
        let of_the_arm = name == MAIN_PROMPT || arm_entries.contains(&name);
        for word in METHOD_WORDS {
            if text.contains(word) && !(of_the_arm && own_words.contains(word)) {
                found.push(format!("{name}: has the HARVEST method word {word:?}"));
            }
        }
    }
    if !received.prompt.starts_with(&crate::shared_reading_list()) {
        found.push("the main prompt does not start with the list of the shared files".to_owned());
    }
    let named = references(&received.prompt);
    let prompts_dir = format!("{PROMPTS_PATH}/");
    for file in named {
        if !received.files.contains_key(&format!("{prompts_dir}{file}")) {
            found.push(format!("the main prompt names the missing file {file}"));
        }
    }
    let prompt_files: PromptFiles = received
        .files
        .iter()
        .filter_map(|(path, text)| {
            Some((path.strip_prefix(&prompts_dir)?.to_owned(), text.clone()))
        })
        .collect();
    let skills: BTreeSet<&str> = received
        .files
        .keys()
        .filter_map(|path| path.strip_prefix(SKILLS_PATH)?.split('/').next())
        .collect();
    found.extend(file_problems(&prompt_files, &|skill| {
        skills.contains(skill)
    }));
    found
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
    Ok(file_problems(&files, &|skill| {
        inputs.join("agent_skills").join(skill).is_dir()
    }))
}

/// The problems of the prompt files `files`. `has_skill` tells whether the run has a skill.
fn file_problems(files: &PromptFiles, has_skill: &dyn Fn(&str) -> bool) -> Vec<String> {
    let mut problems = Vec::new();
    for (name, text) in files {
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
            if !skill.is_empty() && !has_skill(skill) {
                problems.push(format!("{name}: names the missing skill {skill}"));
            }
        }
        if let Some(cycle) = cycle(files, name, &mut Vec::new()) {
            problems.push(format!("read-first cycle: {cycle}"));
        }
    }
    let mut blocks: BTreeMap<Vec<&str>, &str> = BTreeMap::new();
    for (name, text) in files {
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
    problems
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AgentInvocation, Arm, Baseline, CaseDir, RustToolchainContext, no_hook, prepare_agent,
        shared_reading_list,
    };
    use std::collections::HashMap;

    /// What the agent of a stage receives when `arm` (made for the stage) is its arm. The C
    /// source is one file.
    fn received(
        phase: AgentPhase,
        agent: AgentKind,
        arm: impl Fn(AgentPhase, AgentKind) -> Option<Baseline>,
    ) -> Received {
        let case = CaseDir::new().unwrap();
        let toolchain = RustToolchainContext::for_version("1.99.0");
        let env = HashMap::new();
        let baseline = arm(phase, agent);
        let prepared = prepare_agent(AgentInvocation {
            phase,
            agent,
            case: &case,
            arm: match &baseline {
                Some(baseline) => Arm::Baseline(baseline),
                None => Arm::Harvest {
                    additional_prompts: &[],
                    stage_facts: &[],
                },
            },
            timeout_secs: 1,
            model: None,
            subagent_model: None,
            teamclaude: true,
            extra_env: &env,
            output_log_path: None,
            prompt_record_dir: None,
            rust_toolchain: &toolchain,
        })
        .unwrap();
        fs::create_dir_all(case.c_src()).unwrap();
        fs::write(case.c_src().join("a.c"), "int a;").unwrap();
        let guard = case.lock_inputs().unwrap();
        let received = prepared.received().unwrap();
        drop((prepared, guard));
        case.finish();
        received
    }

    /// A baseline arm with one file of each kind.
    fn arm(profile: Profile) -> impl Fn(AgentPhase, AgentKind) -> Option<Baseline> {
        move |_, _| {
            Some(Baseline {
                profile,
                prompt: format!("{}\nDo the task.\n", shared_reading_list()),
                prompt_files: vec![("arm/method.md".to_owned(), "# Method\n".to_owned())],
                skills: vec!["tab2rust"],
                workspace_files: vec![("notes/NOTES.md".to_owned(), "notes\n".to_owned())],
                before_agent: no_hook,
                after_agent: no_hook,
            })
        }
    }

    /// The entries of [`arm`]: its prompt file, its workspace file and the files of its skill.
    fn arm_entries(received: &Received) -> Vec<&str> {
        let skill: Vec<&str> = received
            .files
            .keys()
            .map(|path| &**path)
            .filter(|path| path.starts_with("inputs/agent_skills/tab2rust/"))
            .collect();
        assert!(skill.contains(&"inputs/agent_skills/tab2rust/SKILL.md"));
        [
            &["inputs/prompts/arm/method.md", "workspace/notes/NOTES.md"][..],
            &skill,
        ]
        .concat()
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn a_baseline_agent_receives_its_allow_list_and_no_method_word() {
        for profile in Profile::BASELINES {
            for phase in [AgentPhase::Translate, AgentPhase::Verify] {
                for agent in [AgentKind::Claude, AgentKind::OpenCode] {
                    let received = received(phase, agent, arm(profile));
                    // The skill of the test arm is a HARVEST skill.
                    let found = check_baseline(
                        &received,
                        profile,
                        phase,
                        agent,
                        &arm_entries(&received),
                        &["tab2rust", "stub", "invariant", "workflow"],
                    );
                    assert_eq!(found, [""; 0], "{profile} {phase:?} {agent:?}");
                    assert_eq!(received.system_prompt, None);
                    assert_eq!(
                        received.resume_prompt.is_some(),
                        agent == AgentKind::OpenCode
                    );
                    // The variables that the framework sets on the agent process.
                    let names: Vec<&str> = received.env.iter().map(|(name, _)| &**name).collect();
                    let of_runtime: &[&str] = match agent {
                        AgentKind::Claude => &["CLAUDE_CODE_TMPDIR", "CLAUDE_CODE_SHELL_PREFIX"],
                        AgentKind::OpenCode => &["XDG_CONFIG_HOME"],
                    };
                    let every_runtime = ["PWD", "RUSTUP_TOOLCHAIN", "TMPDIR"];
                    assert_eq!(names, [&every_runtime[..], of_runtime].concat());
                }
            }
        }
    }

    /// The check finds each thing that only a HARVEST stage gets, and a file that an arm does
    /// not declare.
    #[test]
    #[cfg_attr(miri, ignore)]
    fn the_check_finds_what_is_not_on_the_allow_list() {
        let check = |received: &Received, agent| {
            let found = check_baseline(
                received,
                Profile::Vanilla,
                AgentPhase::Translate,
                agent,
                &[],
                &[],
            );
            found.join("\n")
        };
        let harvest = |agent| received(AgentPhase::Translate, agent, |_, _| None);
        let found = check(&harvest(AgentKind::Claude), AgentKind::Claude);
        for problem in [
            "inputs/prompts/common.md: not on the allow-list of the vanilla arm",
            "inputs/prompts/cargo_features.md: not on the allow-list",
            "inputs/agent_skills/cargo_features/SKILL.md: not on the allow-list",
            "inputs/prompts/run.md: not on the allow-list",
            "inputs/prompts/runtime.md: not on the allow-list",
            "inputs/prompts/git_main.md: has the HARVEST method word \"worktree\"",
            "inputs/prompts/runtime.md: has the HARVEST method word \"delegat\"",
            "workspace/.gitignore: has the HARVEST method word \"wt/\"",
            "the system prompt: has the HARVEST method word \"compaction\"",
            "the main prompt does not start with the list of the shared files",
            "the main prompt names the missing file translate/main.md",
        ] {
            assert!(found.contains(problem), "{problem} not in:\n{found}");
        }
        let found = check(&harvest(AgentKind::OpenCode), AgentKind::OpenCode);
        for problem in [
            "workspace/.opencode/plugin/compaction-recovery.js: not on the allow-list",
            "workspace/.opencode/plugin/harvest-resilience.js: has the HARVEST method word \"subtask\"",
            "the resume prompt: has the HARVEST method word \"plan file\"",
        ] {
            assert!(found.contains(problem), "{problem} not in:\n{found}");
        }
        // What a hook writes before the agent starts, in the workspace or elsewhere in the run
        // directory, is given too.
        fn hook(context: &crate::HookContext<'_>) -> Result<(), Box<dyn std::error::Error>> {
            fs::write(context.case.workspace().join("from_hook.txt"), "")?;
            fs::create_dir(context.case.root().join("extra"))?;
            Ok(fs::write(context.case.root().join("extra/tool"), "")?)
        }
        let with_hook = received(AgentPhase::Translate, AgentKind::Claude, |_, _| {
            Some(Baseline {
                profile: Profile::Vanilla,
                prompt: shared_reading_list(),
                prompt_files: Vec::new(),
                skills: Vec::new(),
                workspace_files: Vec::new(),
                before_agent: hook,
                after_agent: no_hook,
            })
        });
        assert_eq!(
            check(&with_hook, AgentKind::Claude),
            "extra/tool: not on the allow-list of the vanilla arm\n\
             workspace/from_hook.txt: not on the allow-list of the vanilla arm"
        );
        // An arm that gives more than it declares.
        let undeclared = received(
            AgentPhase::Translate,
            AgentKind::Claude,
            arm(Profile::Vanilla),
        );
        let found = check(&undeclared, AgentKind::Claude);
        for problem in [
            "inputs/prompts/arm/method.md: not on the allow-list",
            "workspace/notes/NOTES.md: not on the allow-list",
            "inputs/agent_skills/tab2rust/SKILL.md: has the HARVEST method word \"tab2rust\"",
        ] {
            assert!(found.contains(problem), "{problem} not in:\n{found}");
        }
    }

    /// A word of the own method of an arm passes in the main prompt and in a file of the arm.
    /// It does not pass in a text that the framework gives every arm.
    #[test]
    #[cfg_attr(miri, ignore)]
    fn an_own_word_passes_only_in_the_texts_of_the_arm() {
        let (phase, agent) = (AgentPhase::Verify, AgentKind::OpenCode);
        let mut received = received(phase, agent, arm(Profile::Actor));
        let entries: Vec<String> = arm_entries(&received)
            .into_iter()
            .map(str::to_owned)
            .collect();
        let entries: Vec<&str> = entries.iter().map(|path| &**path).collect();
        let own_words = ["tab2rust", "stub", "invariant", "workflow", "subtask"];
        let check = |received: &Received| {
            check_baseline(received, Profile::Actor, phase, agent, &entries, &own_words)
        };
        let own = " Work through the subtasks.";
        received.prompt.push_str(own);
        received
            .files
            .get_mut("inputs/prompts/arm/method.md")
            .unwrap()
            .push_str(own);
        assert_eq!(check(&received), [""; 0]);
        let plugin = "workspace/.opencode/plugin/harvest-resilience.js";
        for file in ["inputs/prompts/output_spec.md", plugin] {
            received.files.get_mut(file).unwrap().push_str(own);
        }
        received.resume_prompt.as_mut().unwrap().push_str(own);
        received.argv.push("--subtask".to_owned());
        received.env.push(("SUBTASK".to_owned(), "1".to_owned()));
        assert_eq!(
            check(&received),
            [
                "inputs/prompts/output_spec.md: has the HARVEST method word \"subtask\"",
                "workspace/.opencode/plugin/harvest-resilience.js: has the HARVEST method word \"subtask\"",
                "the resume prompt: has the HARVEST method word \"subtask\"",
                "the command line of the agent: has the HARVEST method word \"subtask\"",
                "the environment of the agent: has the HARVEST method word \"subtask\"",
            ]
        );
    }

    /// An arm cannot give its own text under the name of a shared file.
    #[test]
    #[cfg_attr(miri, ignore)]
    fn an_arm_cannot_replace_a_shared_file() {
        for (name, _) in crate::shared::FILES {
            let case = CaseDir::new().unwrap();
            let toolchain = RustToolchainContext::for_version("1.99.0");
            let env = HashMap::new();
            let baseline = Baseline {
                prompt_files: vec![(name.to_owned(), "# Other rules\n".to_owned())],
                ..arm(Profile::Actor)(AgentPhase::Translate, AgentKind::Claude).unwrap()
            };
            let err = prepare_agent(AgentInvocation {
                phase: AgentPhase::Translate,
                agent: AgentKind::Claude,
                case: &case,
                arm: Arm::Baseline(&baseline),
                timeout_secs: 1,
                model: None,
                subagent_model: None,
                teamclaude: true,
                extra_env: &env,
                output_log_path: None,
                prompt_record_dir: None,
                rust_toolchain: &toolchain,
            })
            .err()
            .expect("refused")
            .to_string();
            assert_eq!(
                err,
                format!("the actor arm gives the prompt file {name}, which is a shared file")
            );
            case.finish();
        }
    }

    /// No shared text has a word of the HARVEST method.
    #[test]
    fn the_shared_texts_have_no_method_word() {
        for (name, text) in crate::shared::FILES {
            let text = text.to_lowercase();
            for word in METHOD_WORDS {
                assert!(!text.contains(word), "{name} has {word:?}");
            }
            assert_eq!(placeholders(&text), [""; 0], "{name}");
        }
    }

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
