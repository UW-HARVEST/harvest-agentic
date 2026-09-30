//! Memory limits for the commands an OpenCode agent runs.
//!
//! OpenCode runs every bash tool call as `<shell> -c <command>`, and the
//! project config points `shell` at `harvest-shell`. Each command then runs in
//! its own systemd scope under `harvest.slice`, which caps the memory and swap
//! of all harvest agent commands on the machine together. When they run out,
//! the OOM killer stops the largest command, usually the runaway one, and the
//! agent is told why; OpenCode and the host keep running.

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

/// Writes the shell into `<run_tempdir>/agent_env/` and returns its path.
/// `None` when the slice limits cannot be set: commands then run unlimited.
pub(crate) fn install(run_tempdir: &Path) -> Option<PathBuf> {
    try_install(run_tempdir)
        .inspect_err(|e| warn!("Agent commands run without memory limits: {e}"))
        .ok()
}

fn try_install(run_tempdir: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let dir = run_tempdir.join("agent_env");
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
