//! The OpenCode backend.

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::ExitStatus;
use std::time::{Duration, Instant};
use tracing::{info, warn};

use crate::prompt::RegistryLimits;
use crate::{
    AgentInvocation, AgentPhase, BackendEnv, TIMEOUT_EXIT_CODE, append_output_log_line, run_agent,
};

mod config;
mod export;
mod registry;

pub(crate) use config::write_project;
pub(crate) use export::export_opencode_sessions;
pub(crate) use registry::load_registry_limits;

impl AgentPhase {
    fn opencode_agent_name(self) -> &'static str {
        match self {
            AgentPhase::Translate => "harvest-translate",
            AgentPhase::Verify => "harvest-verify",
            AgentPhase::Conform => "harvest-conform",
        }
    }

    fn opencode_description(self) -> &'static str {
        match self {
            AgentPhase::Translate => "Harvest agentic translation backend",
            AgentPhase::Verify => "Harvest agentic verification backend",
            AgentPhase::Conform => "Harvest agentic conformance backend",
        }
    }
}

pub(crate) fn invoke_opencode(
    invocation: &AgentInvocation<'_>,
    log_path: &Path,
    limits: &RegistryLimits,
) -> Result<ExitStatus, Box<dyn std::error::Error>> {
    info!(
        "Invoking OpenCode {} agent (model={}, subagent_model={}, timeout={}s, extra_env={} vars)",
        invocation.phase.label(),
        invocation.model.unwrap_or("(cli default)"),
        invocation.subagent_model.unwrap_or("(main model)"),
        invocation.timeout_secs,
        invocation.extra_env.len()
    );

    let mut env = BackendEnv::default();
    // Hide the user-global OpenCode config from the run. OpenCode resolves
    // its global config dir via xdg-basedir (XDG_CONFIG_HOME/opencode), so a
    // run-private empty directory makes global plugins, global
    // opencode.json{,c}, and global AGENTS.md instructions unreachable. Auth
    // (XDG data dir) and the models cache (XDG cache dir) are unaffected.
    // This replaces `--pure`, whose single effect (verified in the opencode
    // source) is to clear the external-plugin list: that would also disable
    // the project-local compaction-recovery plugin written by
    // `write_opencode_agent`, while project-local `.opencode/` still loads
    // here.
    let xdg_config = invocation.case.root().join("xdg-config");
    env.defaults
        .push(("XDG_CONFIG_HOME", xdg_config.display().to_string()));
    // Temporary workaround for an OpenCode bug (upstream issue #29363).
    match registry::output_cap(limits) {
        Some(cap) => env
            .defaults
            .push(("OPENCODE_EXPERIMENTAL_OUTPUT_TOKEN_MAX", cap.to_string())),
        None if invocation.model.is_some() => warn!(
            "A model of this run has no registry output limit, so every model keeps \
             OpenCode's 32k output cap (opencode#29363)"
        ),
        None => {}
    }

    // One deadline covers the first run and every resume, so resumes cannot
    // stretch the stage past `timeout_secs`.
    let deadline = Instant::now() + Duration::from_secs(invocation.timeout_secs);
    let mut status = run_agent(
        invocation,
        log_path,
        invocation.timeout_secs,
        &opencode_run_args(invocation, None, invocation.prompt),
        &env,
    )?;

    // OpenCode ends the process with success when a provider stream dies
    // mid-response: it treats the truncated turn as a finished one. Half a
    // translation is then graded as if the agent had chosen to stop.
    // Resume the session so the run continues from the work it already did
    // instead of being scored on it.
    let mut resumes = 0;
    loop {
        // The stage used up its time budget, so it is not resumed.
        if status.code() == Some(TIMEOUT_EXIT_CODE) {
            warn!(
                "OpenCode {} agent timed out after {}s.",
                invocation.phase.label(),
                invocation.timeout_secs
            );
            append_output_log_line(
                invocation.output_log_path,
                &format!(
                    "WARNING: {} agent timed out after {}s",
                    invocation.phase.label(),
                    invocation.timeout_secs
                ),
            );
            break;
        }
        match assess_opencode_run(log_path) {
            OpenCodeOutcome::Healthy => break,
            OpenCodeOutcome::Fatal(error) => {
                // Retrying cannot fix this, and grading the partial output
                // would put an environment failure into the results as if it
                // were the model's score.
                return Err(format!(
                    "OpenCode {} agent hit an unrecoverable provider error: {error}",
                    invocation.phase.label()
                )
                .into());
            }
            OpenCodeOutcome::Resumable { session_id, reason } => {
                // Assessment runs after every resume, so a run that stays
                // broken is reported rather than passed off as recovered.
                if resumes >= OPENCODE_MAX_RESUMES {
                    warn!(
                        "OpenCode {} session {session_id} still ending abnormally (reason: \
                         {reason}) after {OPENCODE_MAX_RESUMES} resumes; giving up. The output \
                         is incomplete — grade it as such.",
                        invocation.phase.label()
                    );
                    append_output_log_line(
                        invocation.output_log_path,
                        &format!(
                            "WARNING: {} session {session_id} still abnormal (reason: {reason}) \
                             after {OPENCODE_MAX_RESUMES} resumes; output is incomplete",
                            invocation.phase.label()
                        ),
                    );
                    break;
                }
                let remaining_secs = deadline.saturating_duration_since(Instant::now()).as_secs();
                if remaining_secs == 0 {
                    warn!(
                        "OpenCode {} session {session_id} ended abnormally (reason: {reason}) \
                         with no time left of the {}s budget; not resuming.",
                        invocation.phase.label(),
                        invocation.timeout_secs
                    );
                    append_output_log_line(
                        invocation.output_log_path,
                        &format!(
                            "WARNING: {} session {session_id} ended abnormally (reason: {reason}) \
                             with no time left; not resumed",
                            invocation.phase.label()
                        ),
                    );
                    break;
                }
                resumes += 1;
                warn!(
                    "OpenCode {} session {session_id} ended abnormally (last step reason: {reason}); \
                     resuming (attempt {resumes}/{OPENCODE_MAX_RESUMES}, {remaining_secs}s left)",
                    invocation.phase.label()
                );
                append_output_log_line(
                    invocation.output_log_path,
                    &format!(
                        "WARNING: {} session {session_id} ended abnormally (reason: {reason}); \
                         resuming (attempt {resumes}/{OPENCODE_MAX_RESUMES}, {remaining_secs}s left)",
                        invocation.phase.label()
                    ),
                );
                // The resumed run extends the log: the first segment holds
                // everything the agent did before the stream died, which
                // trace analysis still needs.
                status = run_agent(
                    invocation,
                    log_path,
                    remaining_secs,
                    &opencode_run_args(invocation, Some(&session_id), OPENCODE_RESUME_PROMPT),
                    &env,
                )?;
            }
        }
    }

    Ok(status)
}

/// Sent when resuming a session whose stream died. It names the failure so the
/// model does not mistake the gap for a compaction, and points it at the plan
/// file, which is the only state that survived intact.
const OPENCODE_RESUME_PROMPT: &str = "\
Your previous response was cut off by a provider failure. \
Re-read your plan file to see where you were. \
Check the filesystem to see which files actually exist before you trust any \
earlier claim that one was written. \
Then continue the work from the first unfinished step.";

fn opencode_run_args<'a>(
    invocation: &AgentInvocation<'a>,
    session_id: Option<&'a str>,
    prompt: &'a str,
) -> Vec<&'a str> {
    let mut args = vec!["run", "--format", "json", "--thinking"];
    args.extend([
        "--dangerously-skip-permissions",
        "--agent",
        invocation.phase.opencode_agent_name(),
    ]);
    if let Some(session_id) = session_id {
        args.extend(["--session", session_id]);
    }
    if let Some(model) = invocation.model {
        args.extend(["--model", model]);
    }
    args.push(prompt);
    args
}

/// How an OpenCode run ended, judged from its own JSONL log.
#[derive(Debug, Clone, PartialEq, Eq)]
enum OpenCodeOutcome {
    /// The session reached a real end of turn.
    Healthy,
    /// The session stopped early in a way a `continue` can pick up.
    Resumable { session_id: String, reason: String },
    /// The provider refused in a way retrying cannot fix.
    Fatal(String),
}

/// Maximum `--session … continue` attempts after one dead stream. A provider
/// that keeps dropping the stream must not spin here.
const OPENCODE_MAX_RESUMES: u32 = 3;

/// Provider errors that no amount of retrying fixes. Matched case-insensitively
/// against the API error text. Anything else (dropped stream, 5xx, timeout) is
/// treated as transient and resumable.
const OPENCODE_FATAL_ERROR_MARKERS: &[&str] = &[
    "insufficient balance",
    "insufficient_quota",
    "exceeded your current quota",
    "invalid api key",
    "invalid_api_key",
    "authentication",
    "unauthorized",
    "payment required",
    "billing",
];

/// Judges how an OpenCode run ended by reading the run's own JSONL log.
///
/// Read the per-agent log, never a `&>` capture: the shared `output.log` is
/// appended across stages, so a capture of it can carry sessions from earlier
/// runs and mislead this check.
///
/// The health test is a WHITELIST: a turn is healthy only when the last
/// `step_finish` says `stop`. OpenCode reports every abnormal ending as a
/// normal one — it exits 0 after a stream dies mid-response — and it spells
/// those endings differently each time. A blacklist of known-bad
/// values misses the next spelling. Requiring the one known-good value does
/// not.
fn assess_opencode_run(log_path: &Path) -> OpenCodeOutcome {
    let Ok(file) = fs::File::open(log_path) else {
        return OpenCodeOutcome::Healthy;
    };

    let mut last_finish_reason: Option<String> = None;
    let mut last_session_id: Option<String> = None;
    let mut fatal_error: Option<String> = None;
    let mut saw_any_event = false;

    for line in BufReader::new(file).lines().map_while(Result::ok) {
        let trimmed = line.trim();
        if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
            continue;
        }
        let Ok(event) = serde_json::from_str::<serde_json::Value>(trimmed) else {
            continue;
        };
        let Some(event_type) = event.get("type").and_then(|v| v.as_str()) else {
            continue;
        };
        if let Some(sid) = event.get("sessionID").and_then(|v| v.as_str()) {
            saw_any_event = true;
            last_session_id = Some(sid.to_string());
        }
        match event_type {
            "step_finish" => {
                last_finish_reason = event
                    .get("part")
                    .and_then(|p| p.get("reason"))
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
            }
            "error" => {
                let text = event
                    .get("error")
                    .map(|e| e.to_string())
                    .unwrap_or_default();
                let lowered = text.to_lowercase();
                if OPENCODE_FATAL_ERROR_MARKERS
                    .iter()
                    .any(|marker| lowered.contains(marker))
                {
                    fatal_error = Some(text);
                }
            }
            _ => {}
        }
    }

    if let Some(error) = fatal_error {
        return OpenCodeOutcome::Fatal(error);
    }
    // No parsable events at all: the run never produced a session (a launch
    // failure, or a format this parser does not know). Leave it to the
    // caller's exit-status handling rather than inventing a resume.
    if !saw_any_event {
        return OpenCodeOutcome::Healthy;
    }
    if last_finish_reason.as_deref() == Some("stop") {
        return OpenCodeOutcome::Healthy;
    }
    match last_session_id {
        Some(session_id) if !session_id.is_empty() => OpenCodeOutcome::Resumable {
            session_id,
            reason: last_finish_reason.unwrap_or_else(|| "missing".to_string()),
        },
        _ => OpenCodeOutcome::Healthy,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writes a JSONL log with the given events and assesses it.
    fn assess_log(lines: &[&str]) -> OpenCodeOutcome {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("agent.log");
        fs::write(&path, lines.join("\n")).expect("write log");
        assess_opencode_run(&path)
    }

    #[test]
    fn healthy_run_needs_a_stop_finish() {
        let outcome = assess_log(&[
            r#"{"type":"step_start","sessionID":"ses_a","part":{}}"#,
            r#"{"type":"step_finish","sessionID":"ses_a","part":{"reason":"tool-calls"}}"#,
            r#"{"type":"step_finish","sessionID":"ses_a","part":{"reason":"stop"}}"#,
        ]);
        assert_eq!(outcome, OpenCodeOutcome::Healthy);
    }

    #[test]
    fn dead_stream_is_resumable() {
        // trace_pcre2_3 / trace_libpng_1: reasoning cut mid-sentence, the
        // synthesized step_finish reports "unknown", and opencode exits 0.
        let outcome = assess_log(&[
            r#"{"type":"step_finish","sessionID":"ses_a","part":{"reason":"tool-calls"}}"#,
            r#"{"type":"reasoning","sessionID":"ses_a","part":{"text":"cut off mid-thou"}}"#,
            r#"{"type":"step_finish","sessionID":"ses_a","part":{"reason":"unknown"}}"#,
        ]);
        assert_eq!(
            outcome,
            OpenCodeOutcome::Resumable {
                session_id: "ses_a".to_string(),
                reason: "unknown".to_string(),
            }
        );
    }

    #[test]
    fn whitelist_catches_endings_no_blacklist_would_list() {
        // A step_finish with no `reason` at all, and a truncated turn: both
        // must be caught by requiring "stop" rather than listing bad values.
        for part in [
            r#"{}"#,
            r#"{"reason":"length"}"#,
            r#"{"reason":"tool-calls"}"#,
        ] {
            let line = format!(r#"{{"type":"step_finish","sessionID":"ses_a","part":{part}}}"#);
            assert!(
                matches!(assess_log(&[&line]), OpenCodeOutcome::Resumable { .. }),
                "part {part} should be resumable"
            );
        }
    }

    #[test]
    fn balance_error_is_fatal_not_resumable() {
        // trace_mujs_6: the provider refused outright. Resuming would burn the
        // timeout, and grading the partial run would record an environment
        // failure as a score.
        let outcome = assess_log(&[
            r#"{"type":"step_finish","sessionID":"ses_a","part":{"reason":"tool-calls"}}"#,
            r#"{"type":"error","sessionID":"ses_a","error":{"name":"APIError","data":{"message":"Upstream request failed: [invalid_request_error] Insufficient Balance"}}}"#,
        ]);
        match outcome {
            OpenCodeOutcome::Fatal(msg) => assert!(msg.contains("Insufficient Balance")),
            other => panic!("expected Fatal, got {other:?}"),
        }
    }

    #[test]
    fn a_log_without_events_is_left_alone() {
        assert_eq!(assess_log(&["not json", ""]), OpenCodeOutcome::Healthy);
    }
}
