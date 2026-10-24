//! The Kiro backend (legacy, kept for comparison).

use std::path::Path;
use std::process::ExitStatus;
use tracing::info;

use crate::{AgentInvocation, BackendEnv, run_agent};

pub(crate) fn invoke_kiro(
    invocation: &AgentInvocation<'_>,
    log_path: &Path,
) -> Result<ExitStatus, Box<dyn std::error::Error>> {
    info!(
        "Invoking Kiro {} agent (timeout={}s, extra_env={} vars)",
        invocation.phase.label(),
        invocation.timeout_secs,
        invocation.extra_env.len()
    );
    run_agent(
        invocation,
        log_path,
        invocation.timeout_secs,
        "kiro-cli",
        &[
            "chat",
            "--no-interactive",
            "--trust-all-tools",
            invocation.prompt,
        ],
        None,
        &BackendEnv::default(),
    )
}
