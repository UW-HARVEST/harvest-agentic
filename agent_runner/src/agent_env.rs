//! Memory limits for the commands an OpenCode or Claude Code agent runs.
//!
//! Both run every bash tool call as `harvest-shell -c <command>`: OpenCode
//! through the `shell` of its project config, Claude Code through
//! `CLAUDE_CODE_SHELL_PREFIX`. Each command then runs in
//! its own systemd scope under `harvest.slice`, which caps the memory and swap
//! of all harvest agent commands on the machine together. When they run out,
//! the OOM killer stops the largest command, usually the runaway one, and the
//! agent is told why; the agent and the host keep running.
//!
//! `harvest-sandbox` runs the command under a Landlock allowlist: the command
//! reads the system files and the Rust toolchain, and writes only the case
//! directory, the Cargo home and `/dev`. Other files of the host (other runs,
//! their results, the home directory) are not reachable. The allowlist names
//! what a command needs, not what to hide, so it works on any Linux host with
//! Landlock (>= 5.13); without Landlock the command runs as before.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::{info, warn};

const HARVEST_SHELL: &str = include_str!("agent_env/harvest-shell");
const HARVEST_SHELL_INNER: &str = include_str!("agent_env/harvest-shell-inner");
const HARVEST_SANDBOX: &str = include_str!("agent_env/harvest-sandbox");

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
        ("harvest-sandbox", HARVEST_SANDBOX),
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
    log_sandbox(&dir.join("harvest-sandbox"));
    Ok(dir.join("harvest-shell"))
}

/// Logs whether the commands get the Landlock allowlist on this host.
fn log_sandbox(sandbox: &Path) {
    let abi = Command::new("python3")
        .arg(sandbox)
        .arg("--probe")
        .output()
        .ok()
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .and_then(|text| text.trim().parse::<u32>().ok())
        .unwrap_or(0);
    if abi > 0 {
        info!("Agent commands run under a Landlock allowlist (ABI {abi})");
    } else {
        warn!("Landlock is not available: agent commands can read the whole host");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs `command` through `harvest-sandbox` of a case directory in `case`.
    fn sandboxed(case: &Path, command: &str) -> std::process::Output {
        let dir = case.join("agent_env");
        fs::create_dir_all(&dir).unwrap();
        let sandbox = dir.join("harvest-sandbox");
        fs::write(&sandbox, HARVEST_SANDBOX).unwrap();
        Command::new("python3")
            .arg(&sandbox)
            .arg(command)
            .output()
            .unwrap()
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn sandbox_allows_the_case_directory_and_nothing_else_of_the_host() {
        let root = crate::case_dir::large_temp_dir();
        let case = tempfile::tempdir_in(&root).unwrap();
        let probe = sandboxed(case.path(), "--probe");
        if String::from_utf8_lossy(&probe.stdout).trim() == "0" {
            eprintln!("Landlock is not available here; skipped");
            return;
        }
        let other = tempfile::tempdir_in(&root).unwrap();
        fs::write(other.path().join("secret"), "x").unwrap();
        let out = sandboxed(
            case.path(),
            &format!(
                "echo ok > {case}/inside && cat {case}/inside && echo $HOME && ls /usr >/dev/null && \
                 ! cat {other}/secret 2>/dev/null && ! ls {other} 2>/dev/null",
                case = case.path().display(),
                other = other.path().display()
            ),
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(out.status.success(), "{out:?}");
        assert!(stdout.starts_with("ok\n"), "{stdout}");
        // The command gets a home in the case directory.
        assert!(
            stdout.contains(&format!("{}/home", case.path().display())),
            "{stdout}"
        );
    }
}
