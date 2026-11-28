//! What the framework gives the agent of a stage: the list of everything that the agent finds in
//! its run directory when it starts, and the record of it in the output of the run.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::Path;

use harvest_core::config::AgentKind;
use harvest_core::stage_manifest::{C_SOURCE_DIR, SUITE_DIR};
use sha2::{Digest, Sha256};

use crate::case_dir::{INPUTS, WORKSPACE};
use crate::{AgentInvocation, CaseDir, EnvVar, PROMPTS_PATH, claude, opencode};

/// The file of a record that holds the prompt of the main agent.
const RECORDED_PROMPT_FILE: &str = "PROMPT.md";
/// The file of a record that holds the profile, what the agent runtime gets and the list of the
/// given entries.
const RECORDED_LIST_FILE: &str = "given.json";
/// The directory of a record that holds the copies of the given files, by their path in the run
/// directory. The prompt files are not here: they are at the top of the record.
const RECORDED_FILES_DIR: &str = "files";

/// One entry that the agent finds in its run directory when it starts.
pub struct Given {
    /// Relative to the run directory, for example `inputs/prompts/run.md`. The path of a
    /// directory ends with `/`.
    pub path: String,
    pub kind: GivenKind,
}

pub enum GivenKind {
    File {
        bytes: u64,
        sha256: String,
    },
    /// A symbolic link, with its target.
    Link(String),
    /// A directory that is empty, or whose content is not listed: the C source and the test
    /// suite of the inputs, which the snapshot keeps, and the git record of the workspace.
    Directory,
}

/// Everything in the run directory of `case`, in the order of the paths. The agent can read all
/// of the run directory, so the list has the files of the framework too (the scripts of the
/// command environment, the settings of the agent runtime, the logs).
pub(crate) fn list(case: &CaseDir) -> io::Result<Vec<Given>> {
    let mut entries = Vec::new();
    walk(case.root(), case.root(), &mut entries)?;
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(entries)
}

fn walk(root: &Path, dir: &Path, entries: &mut Vec<Given>) -> io::Result<()> {
    let relative = |path: &Path| {
        let path = path.strip_prefix(root).expect("walked path is under root");
        path.to_string_lossy().into_owned()
    };
    let not_listed = [
        root.join(INPUTS).join(C_SOURCE_DIR),
        root.join(INPUTS).join(SUITE_DIR),
        root.join(WORKSPACE).join(".git"),
    ];
    let children: Vec<fs::DirEntry> = fs::read_dir(dir)?.collect::<Result<_, _>>()?;
    if dir != root && (children.is_empty() || not_listed.iter().any(|closed| closed == dir)) {
        entries.push(Given {
            path: relative(dir) + "/",
            kind: GivenKind::Directory,
        });
        return Ok(());
    }
    for child in children {
        let path = child.path();
        let file_type = child.file_type()?;
        let kind = if file_type.is_symlink() {
            GivenKind::Link(fs::read_link(&path)?.to_string_lossy().into_owned())
        } else if file_type.is_dir() {
            walk(root, &path, entries)?;
            continue;
        } else {
            let (bytes, sha256) = hash_file(&path)?;
            GivenKind::File { bytes, sha256 }
        };
        entries.push(Given {
            path: relative(&path),
            kind,
        });
    }
    Ok(())
}

/// The size and the SHA-256 of a file.
pub fn hash_file(path: &Path) -> io::Result<(u64, String)> {
    let mut hasher = Sha256::new();
    let bytes = io::copy(&mut fs::File::open(path)?, &mut hasher)?;
    Ok((bytes, format!("{:x}", hasher.finalize())))
}

/// The files of the workspace of `case`, with their SHA-256: the crate that an earlier stage
/// left there, when the tool calls this before the framework writes anything. The record does
/// not copy a file that is still the same when the agent starts: the snapshot of the earlier
/// stage has it.
pub(crate) fn earlier_stage(case: &CaseDir) -> io::Result<BTreeMap<String, String>> {
    let workspace = format!("{WORKSPACE}/");
    Ok(list(case)?
        .into_iter()
        .filter(|entry| entry.path.starts_with(&workspace))
        .filter_map(|entry| match entry.kind {
            GivenKind::File { sha256, .. } => Some((entry.path, sha256)),
            GivenKind::Link(_) | GivenKind::Directory => None,
        })
        .collect())
}

/// What the agent runtime gets from the framework besides the prompt on stdin and the files.
pub(crate) struct RuntimeExtras {
    /// The command line of the agent, without the prompt.
    pub(crate) argv: Vec<String>,
    /// Claude Code: the text that `--append-system-prompt` adds.
    pub(crate) system_prompt: Option<String>,
    /// OpenCode: the prompt of a resume after a dead provider stream.
    pub(crate) resume_prompt: Option<String>,
}

impl AgentInvocation<'_> {
    pub(crate) fn runtime_extras(&self) -> Result<RuntimeExtras, String> {
        Ok(match self.agent {
            AgentKind::Claude => RuntimeExtras {
                argv: claude::argv(self)?,
                system_prompt: claude::system_prompt(self),
                resume_prompt: None,
            },
            AgentKind::OpenCode => RuntimeExtras {
                argv: opencode::argv(self),
                system_prompt: None,
                resume_prompt: Some(opencode::resume_prompt(self)),
            },
        })
    }
}

/// Makes `record`, a directory of the output, empty for the record of this stage run. A record
/// of an earlier run of the stage goes.
pub(crate) fn start_record(record: &Path) -> io::Result<()> {
    if record.exists() {
        fs::remove_dir_all(record)?;
    }
    fs::create_dir_all(record)
}

/// Keeps what the agents of the stage are given in `record` (see [`start_record`]). The case
/// directory does not outlive the stage. Call it immediately before the agent starts, so that
/// the record has everything that the framework and the hook of an arm wrote. The record holds:
/// - the prompt of the main agent ([`RECORDED_PROMPT_FILE`]),
/// - a copy of `inputs/prompts/`, at the top of the record,
/// - a copy of each other file of [`list`], by its path in the run directory, in
///   [`RECORDED_FILES_DIR`]. Not copied: a file of the crate of the earlier stage (`earlier`,
///   see [`earlier_stage`]), and the project config of OpenCode, which can hold the key of a
///   provider,
/// - [`RECORDED_LIST_FILE`]: the profile, the command line of the agent, the names of the
///   environment variables that the framework sets on the agent process (`env`), the texts that
///   the agent runtime gets, for Claude Code what the run can take from the configuration of
///   the user, and each entry of [`list`] with its size, its SHA-256 and the path of its copy.
pub(crate) fn record(
    invocation: &AgentInvocation<'_>,
    prompt: &str,
    earlier: &BTreeMap<String, String>,
    env: &[EnvVar],
    record: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let case = invocation.case;
    fs::write(record.join(RECORDED_PROMPT_FILE), prompt)?;
    let prompts = format!("{PROMPTS_PATH}/");
    let project_config = format!("{WORKSPACE}/{}/opencode.json", opencode::PROJECT_DIR);
    let mut files = Vec::new();
    for entry in list(case)? {
        files.push(match entry.kind {
            GivenKind::File { bytes, sha256 } => {
                let mut file =
                    serde_json::json!({"path": entry.path, "bytes": bytes, "sha256": sha256});
                if earlier.get(&entry.path) == Some(&sha256) {
                    file["earlier_stage"] = true.into();
                } else if entry.path != project_config {
                    let copy = match entry.path.strip_prefix(&prompts) {
                        Some(prompt_file) => prompt_file.to_owned(),
                        None => format!("{RECORDED_FILES_DIR}/{}", entry.path),
                    };
                    let to = record.join(&copy);
                    fs::create_dir_all(to.parent().expect("a file has a parent"))?;
                    // Not `fs::copy`: the inputs are read-only, and the record must not be.
                    fs::write(to, fs::read(case.root().join(&entry.path))?)?;
                    file["record"] = copy.into();
                }
                file
            }
            GivenKind::Link(target) => serde_json::json!({"path": entry.path, "link": target}),
            GivenKind::Directory => serde_json::json!({"path": entry.path}),
        });
    }
    let extras = invocation.runtime_extras()?;
    let list = serde_json::json!({
        "profile": invocation.arm.profile(),
        "stage": invocation.phase.prompt_dir(),
        "agent": invocation.agent,
        "agent_argv": extras.argv,
        "agent_env": env.iter().map(|var| &var.name).collect::<Vec<_>>(),
        "main_prompt": RECORDED_PROMPT_FILE,
        "claude_append_system_prompt": extras.system_prompt,
        "opencode_resume_prompt": extras.resume_prompt,
        "claude_user_config": match invocation.agent {
            AgentKind::Claude => claude::UserConfig::read().map(|config| config.json()),
            AgentKind::OpenCode => None,
        },
        "files": files,
    });
    let text = serde_json::to_string_pretty(&list)?;
    Ok(fs::write(record.join(RECORDED_LIST_FILE), text + "\n")?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prompt::RegistryLimits;
    use crate::prompt::tests::with_invocation;

    #[test]
    #[cfg_attr(miri, ignore)]
    fn record_holds_what_the_agent_is_given() {
        with_invocation(AgentKind::Claude, None, true, |invocation| {
            let case = invocation.case;
            // The crate of the earlier stage: one file stays, one file the framework replaces.
            let workspace = case.workspace();
            fs::create_dir_all(workspace.join("src")).unwrap();
            fs::write(workspace.join("src/lib.rs"), "// crate\n").unwrap();
            fs::write(workspace.join("PLAN.md"), "# Old plan\n").unwrap();
            let earlier = earlier_stage(case).unwrap();
            invocation
                .write_files(&RegistryLimits::default(), true)
                .unwrap();
            fs::create_dir_all(case.c_src()).unwrap();
            fs::write(case.c_src().join("a.c"), "int a;").unwrap();
            fs::write(workspace.join("PLAN.md"), "# Plan\n").unwrap();
            let inputs = case.lock_inputs().unwrap();
            crate::set_up_workspace(invocation, &[], None).unwrap();
            // The project of OpenCode, as its backend writes it.
            let project = workspace.join(opencode::PROJECT_DIR);
            fs::create_dir_all(project.join("plugin")).unwrap();
            fs::write(project.join("plugin/p.js"), "// plugin\n").unwrap();
            fs::write(project.join("opencode.json"), "{\"key\": \"secret\"}").unwrap();

            let out = tempfile::tempdir().unwrap();
            let record_dir = out.path().join("prompts/verify");
            // A record of an earlier run of the stage goes.
            fs::create_dir_all(&record_dir).unwrap();
            fs::write(record_dir.join("stale.md"), "old").unwrap();
            start_record(&record_dir).unwrap();
            let env = invocation
                .environment(None, &RegistryLimits::default())
                .unwrap();
            record(invocation, "the prompt\n", &earlier, &env, &record_dir).unwrap();
            assert!(!record_dir.join("stale.md").exists());
            let recorded = |file: &str| fs::read_to_string(record_dir.join(file)).unwrap();
            assert_eq!(recorded(RECORDED_PROMPT_FILE), "the prompt\n");
            // The prompt files are at the top of the record.
            for file in ["run.md", "common.md", "verify/method.md"] {
                assert_eq!(
                    recorded(file),
                    fs::read_to_string(case.prompts().join(file)).unwrap(),
                    "{file}"
                );
            }
            // The inputs are read-only; the record is an ordinary file.
            let writable = |path: &Path| !fs::metadata(path).unwrap().permissions().readonly();
            assert!(!writable(&case.prompts().join("run.md")));
            assert!(writable(&record_dir.join("run.md")));
            // Every other given file is under files/: the skill, the files of the framework in
            // the workspace and in the run directory. Not the project config of OpenCode, and
            // not the file of the earlier stage that is still the same.
            assert_eq!(recorded("files/workspace/PLAN.md"), "# Plan\n");
            assert_eq!(
                recorded("files/workspace/.opencode/plugin/p.js"),
                "// plugin\n"
            );
            assert!(recorded("files/workspace/.gitignore").contains("target/"));
            assert!(recorded("files/.claude/settings.json").contains("sandbox"));
            assert!(
                record_dir
                    .join("files/inputs/agent_skills/cargo_features/SKILL.md")
                    .is_file()
            );
            assert!(
                !record_dir
                    .join("files/workspace/.opencode/opencode.json")
                    .exists()
            );
            assert!(!record_dir.join("files/workspace/src").exists());
            assert!(!record_dir.join("files/inputs/prompts").exists());

            let list: serde_json::Value = serde_json::from_str(&recorded("given.json")).unwrap();
            assert_eq!(list["profile"], "harvest");
            assert_eq!(list["stage"], "verify");
            assert_eq!(list["agent"], "claude");
            let argv: Vec<&str> = list["agent_argv"]
                .as_array()
                .unwrap()
                .iter()
                .map(|arg| arg.as_str().unwrap())
                .collect();
            assert!(argv[0] == "claude" || argv[0] == "teamclaude", "{argv:?}");
            assert!(
                argv.windows(2)
                    .any(|pair| pair == ["--model", "provider/main"])
            );
            assert!(argv.windows(2).any(|pair| pair == ["--max-turns", "10000"]));
            assert!(!argv.contains(&"the prompt\n"));
            let hint = list["claude_append_system_prompt"].as_str().unwrap();
            assert!(hint.contains("After any context compaction"), "{hint}");
            assert!(argv.contains(&hint));
            assert!(list["opencode_resume_prompt"].is_null());
            // The variables of the agent process: names only.
            assert_eq!(
                list["agent_env"],
                serde_json::json!(["PWD", "RUSTUP_TOOLCHAIN", "TMPDIR", "CLAUDE_CODE_TMPDIR"])
            );
            // What the run can take from the configuration of the user: hashes and names only.
            let user = list["claude_user_config"].as_object().unwrap();
            assert!(user["skills"].is_array() && user["plugins"].is_array());
            assert!(user["settings_env"].is_array() && user["claude_md"].is_boolean());
            assert!(user.contains_key("settings_sha256"));
            let files = list["files"].as_array().unwrap();
            let entry = |path: &str| {
                files
                    .iter()
                    .find(|file| file["path"] == path)
                    .unwrap_or_else(|| panic!("{path} not in {files:?}"))
            };
            // sha256 of "# Plan\n".
            assert_eq!(
                entry("workspace/PLAN.md"),
                &serde_json::json!({
                    "path": "workspace/PLAN.md",
                    "bytes": 7,
                    "sha256": "c3964bb3b70a957ec9b233c7dd3653f6ba17701ab00facf88ae1393dc6155577",
                    "record": "files/workspace/PLAN.md"
                })
            );
            assert_eq!(entry("inputs/prompts/run.md")["record"], "run.md");
            assert_eq!(entry("workspace/src/lib.rs")["earlier_stage"], true);
            assert!(entry("workspace/src/lib.rs").get("record").is_none());
            let config = entry("workspace/.opencode/opencode.json");
            assert!(config["sha256"].is_string() && config.get("record").is_none());
            assert_eq!(
                entry("workspace/inputs")["link"],
                case.inputs().to_string_lossy().as_ref()
            );
            // Not listed further: the C source, the git record. Listed: an empty directory.
            for dir in [
                "inputs/c_src/",
                "workspace/.git/",
                "workspace/scratch/",
                "shared/",
                "tmp/",
            ] {
                assert_eq!(entry(dir), &serde_json::json!({ "path": dir }));
            }
            assert!(!files.iter().any(|file| file["path"] == "inputs/c_src/a.c"));
            drop(inputs);
        });
    }
}
