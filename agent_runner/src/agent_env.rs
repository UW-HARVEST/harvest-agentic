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
const SLICE_MEMORY_GIB: u32 = 5;
const SLICE_SWAP_GIB: u32 = 10;

/// The shared limits of the agent commands, for `run.md`.
pub(crate) fn shared_limits() -> String {
    format!("{SLICE_MEMORY_GIB} GiB of memory and {SLICE_SWAP_GIB} GiB of swap (`{SLICE}`)")
}

/// Whether [`install`] can set the shared limit: the systemd user manager answers. Changes
/// nothing on the host.
pub(crate) fn limit_available() -> bool {
    Command::new("systemctl")
        .args(["--user", "show", "--property=Version"])
        .output()
        .is_ok_and(|out| out.status.success())
}

/// What [`install`] does about the shared limits.
pub(crate) enum Limits {
    /// A run: sets them on the host.
    Set,
    /// A dry-run: changes nothing on the host. The value tells whether the host can set them
    /// ([`limit_available`]).
    Available(bool),
}

/// Writes the shell into `<case_dir>/agent_env/` and returns its path.
/// `None` when the slice limits cannot be set: commands then run unlimited.
pub(crate) fn install(case_root: &Path, limits: Limits) -> Option<PathBuf> {
    try_install(case_root, limits)
        .inspect_err(|e| warn!("Agent commands run without memory limits: {e}"))
        .ok()
}

fn try_install(case_root: &Path, limits: Limits) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let shell = write_scripts(case_root)?;
    let dir = case_root.join("agent_env");
    match limits {
        Limits::Set => {
            let output = Command::new("systemctl")
                .args(["--user", "set-property", "--runtime", SLICE])
                .arg(format!("MemoryMax={SLICE_MEMORY_GIB}G"))
                .arg(format!("MemorySwapMax={SLICE_SWAP_GIB}G"))
                .output()?;
            if !output.status.success() {
                return Err(String::from_utf8_lossy(&output.stderr).trim().into());
            }
        }
        Limits::Available(true) => {}
        Limits::Available(false) => {
            return Err("the systemd user manager does not answer".into());
        }
    }
    log_sandbox(&dir.join("harvest-sandbox"));
    Ok(shell)
}

/// The command that runs `script` as an agent command of the run in `case_root` runs: under the
/// shared memory limit and the file allowlist of the case directory. It writes the scripts of
/// `agent_env/` again, so a change that the agent made to them has no effect. A hook of a
/// baseline arm uses it for a program that works on files of the agent. Without the systemd
/// user manager the command has the allowlist and no memory limit.
pub fn agent_command(case_root: &Path, script: &str) -> std::io::Result<Command> {
    let shell = write_scripts(case_root)?;
    let mut command = if limit_available() {
        let mut command = Command::new(&shell);
        command.arg("-c");
        command
    } else {
        sandbox(&shell.with_file_name("harvest-sandbox"))
    };
    command.arg(script);
    Ok(command)
}

/// Writes the scripts into `<case_dir>/agent_env/` and returns the path of the shell. Changes
/// nothing on the host.
pub(crate) fn write_scripts(case_root: &Path) -> std::io::Result<PathBuf> {
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
    Ok(dir.join("harvest-shell"))
}

/// The start of the script `harvest-sandbox` at `script`. Python runs it in isolated mode: it
/// then takes no module from the directory of the script, which is in the case directory and
/// which the agent can write. A module from there would run before the allowlist applies.
fn sandbox(script: &Path) -> Command {
    let mut command = Command::new("python3");
    command.arg("-I").arg(script);
    command
}

/// Logs whether the commands get the Landlock allowlist on this host.
fn log_sandbox(script: &Path) {
    let abi = sandbox(script)
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
        let script = dir.join("harvest-sandbox");
        fs::write(&script, HARVEST_SANDBOX).unwrap();
        sandbox(&script).arg(command).output().unwrap()
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn a_module_beside_the_sandbox_script_does_not_run() {
        let root = crate::case_dir::large_temp_dir();
        let case = tempfile::tempdir_in(&root).unwrap();
        let other = tempfile::tempdir_in(&root).unwrap();
        // The script imports ctypes. This one would run with the rights of the framework.
        let planted = case.path().join("agent_env/ctypes");
        fs::create_dir_all(&planted).unwrap();
        fs::write(
            planted.join("__init__.py"),
            format!(
                "open({:?}, 'w').write('escaped')\n",
                other.path().join("marker")
            ),
        )
        .unwrap();
        let out = sandboxed(case.path(), "echo ok");
        assert_eq!(String::from_utf8_lossy(&out.stdout), "ok\n", "{out:?}");
        assert!(!other.path().join("marker").exists());
        // The shell of the agent commands starts the script in the same way.
        assert!(HARVEST_SHELL_INNER.contains("python3 -I \"$here/harvest-sandbox\""));
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
