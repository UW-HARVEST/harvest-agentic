use std::io::Write;
use std::process::{Command, Output, Stdio};

fn parse_output(output: Option<Output>) -> Option<String> {
    Some(String::from_utf8(output?.stdout).ok()?.trim().to_string())
}

fn git_sha() -> String {
    parse_output(
        Command::new("git")
            .args(["describe", "--always", "--abbrev=0", "--dirty=*"])
            .output()
            .ok(),
    )
    .unwrap_or("unknown".to_string())
}

fn git_date() -> String {
    parse_output(
        Command::new("git")
            .args([
                "show",
                "-s",
                "--format=%cd",
                "--date=format:%Y-%m-%d",
                "HEAD",
            ])
            .output()
            .ok(),
    )
    .unwrap_or("unknown".to_string())
}

/// Short hash of `git diff HEAD` (tracked files), or `None` for a clean tree or without git. It
/// tells two builds from changed working trees apart.
fn git_diff_hash() -> Option<String> {
    let diff = Command::new("git")
        .args(["diff", "HEAD", "--binary"])
        .output()
        .ok()
        .filter(|out| out.status.success())?
        .stdout;
    if diff.is_empty() {
        return None;
    }
    let mut hasher = Command::new("git")
        .args(["hash-object", "--stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .ok()?;
    hasher.stdin.take()?.write_all(&diff).ok()?;
    let hash = parse_output(hasher.wait_with_output().ok())?;
    Some(hash.chars().take(12).collect())
}

/// Tells cargo when to run this script again: when HEAD or the index changes (a checkout, a
/// commit, staging), or a file in a tracked top-level entry of the repository changes (an edit,
/// for the `*` and the diff hash). Paths come from git, so a git worktree works too. Without git,
/// the script runs on every build (cargo reruns when a watched path does not exist).
fn watch_inputs() {
    let git = |args: &[&str]| parse_output(Command::new("git").args(args).output().ok());
    let cwd = std::env::current_dir().unwrap_or_default();
    let (Some(head), Some(index), Some(top)) = (
        git(&["rev-parse", "--git-path", "HEAD"]),
        git(&["rev-parse", "--git-path", "index"]),
        git(&["rev-parse", "--show-toplevel"]),
    ) else {
        println!("cargo:rerun-if-changed=build.rs.no-git");
        return;
    };
    // --git-path can be relative to the current directory.
    for path in [head, index] {
        println!("cargo:rerun-if-changed={}", cwd.join(path).display());
    }
    let entries = git(&["-C", &top, "ls-tree", "--name-only", "HEAD"]).unwrap_or_default();
    for entry in entries.lines() {
        println!("cargo:rerun-if-changed={top}/{entry}");
    }
}

fn main() {
    watch_inputs();
    let diff = git_diff_hash()
        .map(|hash| format!(" diff {hash}"))
        .unwrap_or_default();
    println!(
        "cargo:rustc-env=HARVEST_VERSION={} ({}){diff}",
        git_sha(),
        git_date()
    );
}
