//! Memory limits for the commands an OpenCode or Claude Code agent runs.
//!
//! Both run every bash tool call as `harvest-shell -c <command>`: OpenCode
//! through the `shell` of its project config, Claude Code through
//! `CLAUDE_CODE_SHELL_PREFIX`. Each command then runs in
//! its own systemd scope under `harvest.slice`, which caps the memory and swap
//! of all harvest agent commands on the machine together. When they run out,
//! the OOM killer stops the largest command, usually the runaway one, and the
//! agent is told why; the agent and the host keep running.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::warn;

const HARVEST_SHELL: &str = include_str!("agent_env/harvest-shell");
const HARVEST_SHELL_INNER: &str = include_str!("agent_env/harvest-shell-inner");

/// The slice `harvest-shell` puts every command in, and its limits.
const SLICE: &str = "harvest.slice";
const SLICE_LIMITS: [&str; 2] = ["MemoryMax=5G", "MemorySwapMax=10G"];

/// Writes the shell into `<case_dir>/agent_env/` and returns its path.
/// `None` when the slice limits cannot be set: commands then run unlimited.
pub(crate) fn install(case_root: &Path) -> Option<PathBuf> {
    try_install(case_root)
        .inspect_err(|e| warn!("Agent commands run without memory limits: {e}"))
        .ok()
}

fn try_install(case_root: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let dir = case_root.join("agent_env");
    fs::create_dir_all(&dir)?;
    for (name, script) in [
        ("harvest-shell", HARVEST_SHELL),
        ("harvest-shell-inner", HARVEST_SHELL_INNER),
    ] {
        let path = dir.join(name);
        fs::write(&path, script)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755))?;
    }
    let output = Command::new("systemctl")
        .args(["--user", "set-property", "--runtime", SLICE])
        .args(SLICE_LIMITS)
        .output()?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().into());
    }
    Ok(dir.join("harvest-shell"))
}
