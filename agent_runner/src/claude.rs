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
    let use_ccr = claude_uses_ccr(invocation.model);
    info!(
        "Invoking Claude Code {} agent (model={}, subagent_model={}, no_plan={}, timeout={}s, ccr={}, extra_env={} vars)",
        invocation.phase.label(),
        invocation.model.unwrap_or("(cli default)"),
        invocation.subagent_model.unwrap_or("(main model)"),
        invocation.no_plan,
        invocation.timeout_secs,
        use_ccr,
        invocation.extra_env.len()
    );

    let hint = compaction_hint(invocation.phase);
    let mut args = vec!["-p", invocation.prompt];
    if let Some(model) = invocation.model {
        args.extend(["--model", model]);
    }
    args.extend(["--allowed-tools", "Bash(*)", "Write", "Edit"]);
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
    run_agent(invocation, log_path, invocation.timeout_secs, &args, &env)
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
}
