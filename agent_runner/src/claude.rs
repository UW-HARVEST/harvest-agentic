//! The Claude Code backend.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use tracing::info;

use crate::{AgentInvocation, AgentPhase, Arm, BackendEnv, EnvVar, run_agent};

/// The limit of agent turns in one session, the same for every arm. It is far above what a stage
/// needs, so that the time limit of the stage ends a run, not this number: an arm whose main
/// agent does all the work itself takes many more turns than one that sends sub-agents.
const MAX_TURNS: &str = "10000";

pub(crate) fn invoke_claude(
    invocation: &AgentInvocation<'_>,
    prompt: &str,
    log_path: &Path,
    env: &[EnvVar],
) -> Result<ExitStatus, Box<dyn std::error::Error>> {
    let argv = argv(invocation)?;
    info!(
        "Invoking Claude Code {} agent (profile={}, model={}, subagent_model={}, timeout={}s, ccr={}, teamclaude={}, extra_env={} vars)",
        invocation.phase.label(),
        invocation.arm.profile(),
        invocation.model.unwrap_or("(cli default)"),
        invocation.subagent_model.unwrap_or("(main model)"),
        invocation.timeout_secs,
        claude_uses_ccr(invocation.model),
        argv[0] == TEAMCLAUDE,
        invocation.extra_env.len()
    );
    let args: Vec<&str> = argv[1..].iter().map(String::as_str).collect();
    run_agent(
        invocation,
        log_path,
        invocation.timeout_secs,
        &argv[0],
        &args,
        prompt,
        env,
    )
}

/// The environment variables that the backend sets on the agent process. `shell` runs the
/// agent commands (see `agent_env`).
pub(crate) fn backend_env(invocation: &AgentInvocation<'_>, shell: Option<&Path>) -> BackendEnv {
    let mut env = BackendEnv::default();
    // Claude Code gives sandboxed commands CLAUDE_CODE_TMPDIR (default
    // /tmp/claude), not the TMPDIR that the framework sets, and keeps its own
    // per-user directory there too.
    env.defaults.push((
        "CLAUDE_CODE_TMPDIR",
        invocation.case.tmp().display().to_string(),
    ));
    // Claude Code picks a sub-agent model from, in order: the Agent tool's
    // `model` parameter, the agent definition's frontmatter, then
    // CLAUDE_CODE_SUBAGENT_MODEL. With CLAUDE_CODE_SUBAGENT_MODEL_FORCE it
    // drops the first two (and the Workflow tool's per-agent models), so the
    // run's sub-agent model holds like OpenCode's (verified in the 2.1.294
    // bundle).
    if let Some(sub) = invocation.subagent_model {
        env.defaults.extend([
            ("CLAUDE_CODE_SUBAGENT_MODEL", sub.to_string()),
            ("CLAUDE_CODE_SUBAGENT_MODEL_FORCE", "1".to_string()),
        ]);
    }
    // Claude Code runs each Bash command as `<prefix> '<command>'`, with a
    // trailing ` -<flag>` of the prefix kept unquoted between them (verified in
    // the 2.1.294 bundle): `harvest-shell -c <command>`, as with OpenCode.
    if let Some(shell) = shell {
        env.defaults.push((
            "CLAUDE_CODE_SHELL_PREFIX",
            format!("{} -c", shell.display()),
        ));
    }
    // CCR routes the run through its local proxy; a run-config value must
    // not redirect it.
    if claude_uses_ccr(invocation.model) {
        env.overrides
            .push(("ANTHROPIC_BASE_URL", "http://127.0.0.1:3456".to_string()));
    }
    env
}

const TEAMCLAUDE: &str = "teamclaude";

/// The command line of the agent: the program and its arguments. The prompt is not in it: it
/// goes on stdin.
pub(crate) fn argv(invocation: &AgentInvocation<'_>) -> Result<Vec<String>, String> {
    let teamclaude = claude_uses_teamclaude(
        invocation.teamclaude,
        invocation.model,
        invocation.subagent_model,
    )?;
    // `teamclaude run` starts `claude` with the arguments after `--`, routed
    // through its proxy.
    let mut argv = if teamclaude {
        vec![TEAMCLAUDE, "run", "--"]
    } else {
        vec!["claude"]
    };
    argv.push("-p");
    if let Some(model) = invocation.model {
        argv.extend(["--model", model]);
    }
    argv.extend(["--allowed-tools", "Bash(*)", "Write", "Edit"]);
    argv.extend(["--disallowed-tools", "WebFetch", "WebSearch"]);
    argv.push("--dangerously-skip-permissions");
    let system_prompt = system_prompt(invocation);
    if let Some(text) = &system_prompt {
        argv.extend(["--append-system-prompt", text]);
    }
    argv.extend([
        "--max-turns",
        MAX_TURNS,
        "--output-format",
        "stream-json",
        "--verbose",
    ]);
    Ok(argv.into_iter().map(str::to_owned).collect())
}

/// What a Claude Code run can take from the configuration of the user, which the framework
/// does not replace. Names and hashes only, no content: for the record of a run, because these
/// can differ between two hosts or two days, and for a person who reads a dry-run.
pub(crate) struct UserConfig {
    /// The configuration directory: `CLAUDE_CONFIG_DIR`, or `.claude` in the home directory.
    dir: PathBuf,
    /// The SHA-256 of the user settings and of the list of installed plugins.
    settings_sha256: Option<String>,
    installed_plugins_sha256: Option<String>,
    /// The user-level skills: the path of each in the `skills` directory.
    skills: Vec<String>,
    /// Each installed plugin: its name, its scope (`user`, `project` or `local`) and, for a
    /// scope that is a project, the directory of the project.
    plugins: Vec<(String, String, Option<String>)>,
    /// The user has a `CLAUDE.md`, which Claude Code reads in every session.
    claude_md: bool,
    /// The names of the environment variables that the user settings give every session.
    settings_env: Vec<String>,
}

impl UserConfig {
    /// Reads the configuration of the user. `None` when no directory can hold it.
    pub(crate) fn read() -> Option<Self> {
        let dir = std::env::var_os("CLAUDE_CONFIG_DIR")
            .map(PathBuf::from)
            .or_else(|| Some(PathBuf::from(std::env::var_os("HOME")?).join(".claude")))?;
        let hash = |file: &str| {
            crate::given::hash_file(&dir.join(file))
                .ok()
                .map(|(_, hash)| hash)
        };
        let json = |file: &str| -> Option<serde_json::Value> {
            serde_json::from_str(&fs::read_to_string(dir.join(file)).ok()?).ok()
        };
        fn skills(root: &Path, dir: &Path, depth: usize, found: &mut Vec<String>) {
            let Ok(entries) = fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                // Not a hidden directory, for example the trash of removed skills.
                if !path.is_dir() || entry.file_name().to_string_lossy().starts_with('.') {
                    continue;
                }
                if path.join("SKILL.md").is_file() {
                    let name = path.strip_prefix(root).unwrap_or(&path);
                    found.push(name.to_string_lossy().into_owned());
                } else if depth > 0 {
                    skills(root, &path, depth - 1, found);
                }
            }
        }
        let mut names = Vec::new();
        skills(&dir.join("skills"), &dir.join("skills"), 2, &mut names);
        names.sort();
        // `{"plugins": {"<name>": [{"scope": "...", "projectPath": "..."}]}}`
        let installed = json("plugins/installed_plugins.json");
        let mut plugins = Vec::new();
        let listed = installed
            .as_ref()
            .and_then(|list| list["plugins"].as_object());
        for (name, installs) in listed.into_iter().flatten() {
            for install in installs.as_array().into_iter().flatten() {
                let scope = install["scope"].as_str().unwrap_or("unknown").to_owned();
                let project = install["projectPath"].as_str().map(str::to_owned);
                plugins.push((name.clone(), scope, project));
            }
        }
        let settings = json("settings.json");
        let settings_env = settings
            .as_ref()
            .and_then(|settings| settings["env"].as_object());
        Some(UserConfig {
            settings_sha256: hash("settings.json"),
            installed_plugins_sha256: hash("plugins/installed_plugins.json"),
            skills: names,
            plugins,
            claude_md: dir.join("CLAUDE.md").is_file(),
            settings_env: settings_env
                .into_iter()
                .flatten()
                .map(|(name, _)| name.clone())
                .collect(),
            dir,
        })
    }

    /// For the record of a run.
    pub(crate) fn json(&self) -> serde_json::Value {
        let plugins: Vec<_> = (self.plugins.iter())
            .map(|(name, scope, project)| {
                serde_json::json!({"name": name, "scope": scope, "project": project})
            })
            .collect();
        serde_json::json!({
            "dir": self.dir,
            "settings_sha256": self.settings_sha256,
            "installed_plugins_sha256": self.installed_plugins_sha256,
            "skills": self.skills,
            "plugins": plugins,
            "claude_md": self.claude_md,
            "settings_env": self.settings_env,
        })
    }

    /// For a person: one line for each kind of entry, with the names that the host has.
    pub(crate) fn lines(&self) -> Vec<String> {
        let list = |names: Vec<String>| match names.is_empty() {
            true => "none".to_owned(),
            false => names.join(", "),
        };
        let quoted = |names: &[String]| names.iter().map(|name| format!("`{name}`")).collect();
        let plugins = (self.plugins.iter())
            .map(|(name, scope, project)| match project {
                Some(project) => format!("`{name}` (scope {scope}, for `{project}`)"),
                None => format!("`{name}` (scope {scope})"),
            })
            .collect();
        // A skill can be in more than one directory: each name once.
        let mut skills: Vec<String> = (self.skills.iter())
            .map(|path| path.rsplit('/').next().unwrap_or(path).to_owned())
            .collect();
        skills.sort();
        skills.dedup();
        vec![
            format!(
                "user-level skills: {} ({} directories with a `SKILL.md`)",
                list(quoted(&skills)),
                self.skills.len()
            ),
            format!("installed plugins: {}", list(plugins)),
            format!(
                "`CLAUDE.md` of the user: {}",
                if self.claude_md { "exists" } else { "none" }
            ),
            format!(
                "names in `env` of the user settings: {}",
                list(quoted(&self.settings_env))
            ),
        ]
    }

    pub(crate) fn dir(&self) -> &Path {
        &self.dir
    }
}

/// The text that the stage appends to the system prompt of Claude Code: the compaction-recovery
/// hint of a HARVEST stage. A baseline arm adds no text.
pub(crate) fn system_prompt(invocation: &AgentInvocation<'_>) -> Option<String> {
    match invocation.arm {
        Arm::Harvest { .. } => Some(compaction_hint(invocation.phase)),
        Arm::Baseline(_) => None,
    }
}

/// Compaction-recovery hint, injected per turn. Claude-only: the OpenCode
/// backend replaces this with the compaction-recovery plugin (see
/// `OPENCODE_COMPACTION_PLUGIN`), which fires exactly once per compaction
/// instead of diluting every turn's system prompt. It tells the main agent
/// and a sub-agent apart, in case a sub-agent gets it too.
fn compaction_hint(phase: AgentPhase) -> String {
    let files = phase.recovery_files();
    let (last, first) = files.split_last().expect("a phase has a state file");
    let list = if first.is_empty() {
        last.clone()
    } else {
        format!("{} and {last}", first.join(", "))
    };
    format!(
        "After any context compaction, the main agent of the stage MUST first read {list}. \
         A sub-agent MUST first read again the prompt file that its task message names, and \
         inputs/prompts/subagent.md, and then continue its own task."
    )
}

pub(crate) fn claude_uses_ccr(model: Option<&str>) -> bool {
    model.is_some_and(|m| m.contains(','))
}

/// Whether Claude Code runs through TeamClaude: it does when `allowed` and
/// the `teamclaude` command is on PATH. Errors when it does and a model uses
/// CCR (`provider,model`), because TeamClaude sends every request of Claude
/// Code to its own proxy.
pub fn claude_uses_teamclaude(
    allowed: bool,
    model: Option<&str>,
    subagent_model: Option<&str>,
) -> Result<bool, String> {
    teamclaude_route(allowed && on_path("teamclaude"), model, subagent_model)
}

fn teamclaude_route(
    teamclaude: bool,
    model: Option<&str>,
    subagent_model: Option<&str>,
) -> Result<bool, String> {
    if teamclaude
        && let Some(ccr) = [model, subagent_model]
            .into_iter()
            .find(|m| claude_uses_ccr(*m))
            .flatten()
    {
        return Err(format!(
            "the CCR model {ccr} cannot run through TeamClaude, which Claude Code uses when \
             `teamclaude` is on PATH: give --no-teamclaude (config: teamclaude = false)"
        ));
    }
    Ok(teamclaude)
}

/// Whether an executable file `command` is in a directory of PATH.
fn on_path(command: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|dir| {
            fs::metadata(dir.join(command))
                .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        })
    })
}

pub(crate) fn write_claude_sandbox(case_dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let claude_dir = case_dir.join(".claude");
    fs::create_dir_all(&claude_dir)?;
    fs::write(
        claude_dir.join("settings.json"),
        serde_json::json!({
            "sandbox": {
                "enabled": true,
                "allowUnsandboxedCommands": false,
                "filesystem": {
                    "allowRead": [case_dir.to_string_lossy()],
                    "allowWrite": [case_dir.to_string_lossy()]
                }
            }
        })
        .to_string(),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compaction_hint_names_the_recovery_files() {
        let subagent = " A sub-agent MUST first read again the prompt file that its task message \
                        names, and inputs/prompts/subagent.md, and then continue its own task.";
        assert_eq!(
            compaction_hint(AgentPhase::Verify),
            format!(
                "After any context compaction, the main agent of the stage MUST first read \
                 inputs/prompts/main_agent.md, inputs/prompts/verify/main.md, PLAN.md and \
                 HYPOTHESES.md.{subagent}"
            )
        );
        assert_eq!(
            compaction_hint(AgentPhase::Quality),
            format!(
                "After any context compaction, the main agent of the stage MUST first read \
                 inputs/prompts/main_agent.md, inputs/prompts/quality/main.md and \
                 QUALITY.md.{subagent}"
            )
        );
        assert_eq!(
            compaction_hint(AgentPhase::Conform),
            format!(
                "After any context compaction, the main agent of the stage MUST first read \
                 inputs/prompts/main_agent.md, inputs/prompts/conform/main.md and \
                 CONFORM.md.{subagent}"
            )
        );
    }

    #[test]
    fn teamclaude_rejects_ccr_models() {
        assert_eq!(
            teamclaude_route(true, Some("sonnet"), Some("haiku")),
            Ok(true)
        );
        assert_eq!(teamclaude_route(true, None, None), Ok(true));
        for (model, sub) in [
            (Some("openrouter,a/b"), None),
            (None, Some("openrouter,c/d")),
        ] {
            let err = teamclaude_route(true, model, sub).unwrap_err();
            assert!(err.contains("openrouter,"), "{err}");
            assert_eq!(teamclaude_route(false, model, sub), Ok(false));
        }
    }
}
