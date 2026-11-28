//! The dry-run of a stage. The stage does everything that a run does up to the moment when its
//! agent would start, and stops there. What the agent would receive stays in the output of the
//! run, for a person to read:
//!
//! ```text
//! <output>/
//!   DRY_RUN.md          what was rendered, how a run differs, and what is not shown
//!   PROMPT.md           the prompt of the main agent
//!   ARGV.txt            the command line of the agent
//!   ENV.txt             the environment variables that the framework sets on the agent process
//!   SYSTEM_PROMPT.md    Claude Code: the text that the stage appends to the system prompt, if any
//!   RESUME_PROMPT.md    OpenCode: the prompt of a resume after a dead provider stream
//!   run_dir/            case_dir, as the agent finds it (see `case_dir`)
//! ```
//!
//! A run that keeps a prompt record keeps it in a dry-run too (see `given::record`). The stage
//! has no result: nothing is frozen, and the output is not a snapshot. The output holds no
//! secret: see [`DryRun::keep`] for the environment, and `opencode::config` for the project of
//! OpenCode.

use std::fmt;
use std::fs;
use std::path::Path;

use harvest_core::Representation;
use harvest_core::config::AgentKind;

use crate::case_dir::{DRY_RUN_CASE, INPUTS, WORKSPACE};
use crate::opencode::{MASKED, PROJECT_DIR};
use crate::{AgentInvocation, Arm, EnvVar, claude};

/// The file of a dry-run that says what was rendered and what each entry is.
const SUMMARY_FILE: &str = "DRY_RUN.md";

/// What the tool of a stage returns in place of its result when the run is a dry-run.
pub struct DryRun {
    /// One line: the stage, the profile, the agent runtime and the directory.
    line: String,
}

impl DryRun {
    /// Writes into `dir`, the directory of the dry-run, what the agent of `invocation` gets
    /// besides the files of its run directory, each in a file of its own, and
    /// [`SUMMARY_FILE`]. `prompt` is the prompt of the main agent, and `env` are the variables
    /// of the agent process: a value from the `env` table of the run config is not written,
    /// because it can be the key of a provider. Call it when the stage has started
    /// (`start_stage`).
    pub(crate) fn keep(
        invocation: &AgentInvocation<'_>,
        prompt: &str,
        env: &[EnvVar],
        dir: &Path,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let extras = invocation.runtime_extras()?;
        let argv = extras.argv.join("\n") + "\n";
        let env: String = (env.iter())
            .map(|var| match var.from_config {
                true => format!("{}={MASKED}\n", var.name),
                false => format!("{}={}\n", var.name, var.value),
            })
            .collect();
        let texts = [
            (
                "PROMPT.md",
                "the prompt of the main agent. The agent gets it on its standard input",
                Some(prompt),
            ),
            (
                "ARGV.txt",
                "the command line of the agent, one argument on each line. The prompt is not in it",
                Some(&*argv),
            ),
            (
                "ENV.txt",
                "the environment variables that the framework sets on the agent process, each \
                 with its final value. A value from the `env` table of the run config is masked",
                Some(&*env),
            ),
            (
                "SYSTEM_PROMPT.md",
                "the text that the stage appends to the system prompt of the agent",
                extras.system_prompt.as_deref(),
            ),
            (
                "RESUME_PROMPT.md",
                "the prompt of a resume, which the agent gets when a provider failure cut its \
                 session",
                extras.resume_prompt.as_deref(),
            ),
        ];
        let stage = invocation.phase.prompt_dir();
        let (profile, agent) = (invocation.arm.profile(), invocation.agent);
        let mut summary = format!(
            "# Dry-run of the {stage} stage\n\n\
             No agent ran. This directory holds what the stage gives its agent, as it is at the \
             moment when the agent would start.\n\n\
             - Profile: {profile}\n\
             - Agent runtime: {agent}\n"
        );
        for (file, what, text) in texts {
            match text {
                Some(text) => {
                    fs::write(dir.join(file), text)?;
                    summary.push_str(&format!("- `{file}`: {what}.\n"));
                }
                None => summary.push_str(&format!("- No `{file}` ({what}): the stage has none.\n")),
            }
        }
        summary.push_str(&format!(
            "- `{DRY_RUN_CASE}/`: the run directory. The working directory of the agent is \
             `{DRY_RUN_CASE}/{WORKSPACE}/`.\n"
        ));
        if let Some(record) = invocation.prompt_record_dir {
            let record = std::path::absolute(record)?;
            summary.push_str(&format!(
                "- `{}/`: the record that a run keeps. Its `given.json` lists each file that \
                 the agent gets. Every other file there is a record of the framework, which the \
                 agent does not get.\n",
                record.strip_prefix(dir).unwrap_or(&record).display()
            ));
        }
        summary.push_str(&differences(invocation, dir));
        summary.push_str(&not_shown(agent));
        fs::write(dir.join(SUMMARY_FILE), summary)?;
        Ok(DryRun {
            line: format!(
                "Dry-run of the {stage} stage (profile {profile}, agent {agent}): what the \
                 agent receives is in {}",
                dir.display()
            ),
        })
    }
}

/// The section of the summary on what a run has in another way than the dry-run in `dir`.
fn differences(invocation: &AgentInvocation<'_>, dir: &Path) -> String {
    let log = invocation.phase.log_file_name();
    let prompt_copy = Path::new(log).with_extension("prompt");
    let project_config = format!("{DRY_RUN_CASE}/{WORKSPACE}/{PROJECT_DIR}/opencode.json");
    let (settings, shell) = match invocation.agent {
        AgentKind::Claude => (
            format!("{DRY_RUN_CASE}/.claude/settings.json"),
            "`CLAUDE_CODE_SHELL_PREFIX` in `ENV.txt`".to_owned(),
        ),
        AgentKind::OpenCode => (
            project_config.clone(),
            format!("`shell` in `{project_config}`"),
        ),
    };
    // Only a HARVEST stage has the file of the facts of the run.
    let run_file = match invocation.arm {
        Arm::Harvest { .. } => format!("`{DRY_RUN_CASE}/{INPUTS}/prompts/run.md`, "),
        Arm::Baseline(_) => String::new(),
    };
    let mut text = format!(
        "\n## How a run differs\n\n\
         - The run directory of a run is a new temporary directory. Here it is `{}`, and each \
         text that names the run directory has this path: {run_file}\
         `{settings}`, the links `{INPUTS}` and `shared` of the workspace, and `ENV.txt`. A \
         search for a word in this directory can find the word in that path.\n\
         - A run makes `{DRY_RUN_CASE}/{INPUTS}/` read-only before the agent starts. Here it is \
         writable again, so that the directory can be removed.\n\
         - When the agent command starts, a run writes `{DRY_RUN_CASE}/logs/{}` (the text of \
         `PROMPT.md`), and the output of the agent goes to `{DRY_RUN_CASE}/logs/{log}`.\n\
         - A run sets the shared memory limit of the agent commands on the host. A dry-run only \
         asks whether the host can set it. When the host can, and the setting then fails, a run \
         gives its agent no shell for its commands ({shell}), and a dry-run shows one.\n",
        dir.join(DRY_RUN_CASE).display(),
        prompt_copy.display(),
    );
    if invocation.agent == AgentKind::OpenCode {
        text.push_str(&format!(
            "- When `{project_config}` has a provider definition from the configuration of the \
             user, its values that can be secrets are replaced with `{MASKED}`: the values of \
             keys with a name like key, token, secret, password or auth, and of `headers`. A \
             run writes the real values.\n"
        ));
    }
    text
}

/// The section of the summary on what the agent gets from outside the framework. A dry-run does
/// not render it.
fn not_shown(agent: AgentKind) -> String {
    let mut text = "\n## Not shown here\n\n\
        - The environment that the agent process inherits from the process of the framework. \
        `ENV.txt` has only the variables that the framework sets.\n"
        .to_owned();
    match agent {
        AgentKind::Claude => match claude::UserConfig::read() {
            Some(config) => {
                text.push_str(&format!(
                    "- The configuration of the user for Claude Code (`{}`), which the \
                     framework does not replace. On this host now:\n",
                    config.dir().display()
                ));
                for line in config.lines() {
                    text.push_str(&format!("  - {line}\n"));
                }
            }
            None => text.push_str(
                "- The configuration of the user for Claude Code, which the framework does not \
                 replace. This host has no directory for it.\n",
            ),
        },
        AgentKind::OpenCode => text.push_str(
            "- The configuration of the user for OpenCode. A run hides the global configuration \
             (config files, plugins, instructions): `XDG_CONFIG_HOME` in `ENV.txt` names a \
             directory of the run that has none. The stored logins of the user and the model cache stay \
             in use.\n",
        ),
    }
    text
}

impl fmt::Display for DryRun {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.line)
    }
}

impl Representation for DryRun {
    fn name(&self) -> &'static str {
        "dry_run"
    }
}
