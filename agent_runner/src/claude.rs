//! The Claude Code backend.

use std::fs;
use std::path::Path;
use std::process::ExitStatus;
use tracing::info;

use crate::{AgentInvocation, AgentPhase, BackendEnv, run_agent};

pub(crate) fn invoke_claude(
    invocation: &AgentInvocation<'_>,
    log_path: &Path,
) -> Result<ExitStatus, Box<dyn std::error::Error>> {
    let teamclaude = claude_uses_teamclaude(
        invocation.teamclaude,
        invocation.model,
        invocation.subagent_model,
    )?;
    let use_ccr = claude_uses_ccr(invocation.model);
    info!(
        "Invoking Claude Code {} agent (model={}, subagent_model={}, no_plan={}, timeout={}s, ccr={}, teamclaude={}, extra_env={} vars)",
        invocation.phase.label(),
        invocation.model.unwrap_or("(cli default)"),
        invocation.subagent_model.unwrap_or("(main model)"),
        invocation.no_plan,
        invocation.timeout_secs,
        use_ccr,
        teamclaude,
        invocation.extra_env.len()
    );

    let hint = compaction_hint(invocation.phase);
    // `teamclaude run` starts `claude` with the arguments after `--`, routed
    // through its proxy.
    let (program, mut args) = if teamclaude {
        ("teamclaude", vec!["run", "--"])
    } else {
        ("claude", Vec::new())
    };
    args.extend(["-p", invocation.prompt]);
    if let Some(model) = invocation.model {
        args.extend(["--model", model]);
    }
    args.extend(["--allowed-tools", "Bash(*)", "Write", "Edit"]);
    args.extend(["--disallowed-tools", "WebFetch", "WebSearch"]);
    args.push("--dangerously-skip-permissions");
    if invocation.plan_files_enabled() {
        args.extend(["--append-system-prompt", &hint]);
    }
    args.extend([
        "--max-turns",
        "1000",
        "--output-format",
        "stream-json",
        "--verbose",
    ]);
    let mut env = BackendEnv::default();
    // Claude Code gives sandboxed commands CLAUDE_CODE_TMPDIR (default
    // /tmp/claude), not the TMPDIR that `run_agent` sets, and keeps its own
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
    // CCR routes the run through its local proxy; a run-config value must
    // not redirect it.
    if use_ccr {
        env.overrides
            .push(("ANTHROPIC_BASE_URL", "http://127.0.0.1:3456".to_string()));
    }
    run_agent(
        invocation,
        log_path,
        invocation.timeout_secs,
        program,
        &args,
        &env,
    )
}

/// Compaction-recovery hint, injected per turn. Claude-only: the OpenCode
/// backend replaces this with the compaction-recovery plugin (see
/// `OPENCODE_COMPACTION_PLUGIN`), which fires exactly once per compaction
/// instead of diluting every turn's system prompt.
fn compaction_hint(phase: AgentPhase) -> String {
    format!(
        "After any context compaction, you MUST first read {}.",
        phase.recovery_files().join(" and ")
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
        assert_eq!(
            compaction_hint(AgentPhase::Verify),
            "After any context compaction, you MUST first read PLAN.md and HYPOTHESES.md."
        );
        assert_eq!(
            compaction_hint(AgentPhase::Conform),
            "After any context compaction, you MUST first read CONFORM.md."
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
