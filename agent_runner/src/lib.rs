use harvest_core::config::AgentKind;
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};
use tracing::{info, warn};

mod agent_env;
pub mod case_dir;
mod claude;
mod kiro;
mod opencode;
mod prompt;
mod toolchain;

pub use case_dir::CaseDir;
use claude::claude_uses_ccr;
pub use claude::claude_uses_teamclaude;
use prompt::RegistryLimits;
pub use toolchain::{RustToolchainContext, detect_rust_toolchain_context, locate_test_corpus};

#[derive(Debug, Clone, Copy)]
pub enum AgentPhase {
    Translate,
    Verify,
    /// Third-stage refinement: an already-translated project is given the
    /// external test suite (gtest / tractor vectors) and refined until every
    /// external test passes. Runs standalone, decoupled from translate/verify.
    Conform,
}

impl AgentPhase {
    fn label(self) -> &'static str {
        match self {
            AgentPhase::Translate => "translation",
            AgentPhase::Verify => "verification",
            AgentPhase::Conform => "conformance",
        }
    }

    fn log_file_name(self) -> &'static str {
        match self {
            AgentPhase::Translate => "translation.log",
            AgentPhase::Verify => "verify.log",
            AgentPhase::Conform => "conform.log",
        }
    }

    /// The persistent memory files the agent must re-read to recover after a
    /// context compaction. Only meaningful when plan files are enabled.
    fn recovery_files(self) -> &'static [&'static str] {
        match self {
            AgentPhase::Translate => &["PLAN.md"],
            AgentPhase::Verify => &["PLAN.md", "HYPOTHESES.md"],
            AgentPhase::Conform => &["CONFORM.md"],
        }
    }
}

pub const DEFAULT_AGENT_TIMEOUT_SECS: u64 = 48 * 60 * 60;

pub struct AgentInvocation<'a> {
    pub phase: AgentPhase,
    pub agent: AgentKind,
    /// The run directory. The agent works in its workspace.
    pub case: &'a CaseDir,
    /// The prompt template, with the placeholders of the tool filled in. [`invoke_agent`] fills
    /// the shared ones (`AgentInvocation::render_prompt`).
    pub prompt: &'a str,
    pub timeout_secs: u64,
    pub model: Option<&'a str>,
    pub subagent_model: Option<&'a str>,
    pub no_plan: bool,
    pub teamclaude: bool,
    pub extra_env: &'a HashMap<String, String>,
    pub output_log_path: Option<&'a Path>,
    pub rust_toolchain: &'a RustToolchainContext,
}

impl AgentInvocation<'_> {
    /// Whether the prompt instructs the agent to maintain persistent plan
    /// files (PLAN.md / HYPOTHESES.md). `no_plan` (no plan, no sub-agent push)
    /// drops that instruction, so the per-turn "read PLAN.md after compaction"
    /// system prompt must not be injected then.
    fn plan_files_enabled(&self) -> bool {
        !self.no_plan
    }
}

/// The sub-agent model of a run: `None` when the sub-agents use the main
/// model (none given, or the same as `model`), which makes the run identical to
/// one without a sub-agent model. Errors when the backend cannot give its
/// sub-agents a model of their own: Kiro has no such setting, and Claude Code
/// routes all of its requests through one endpoint, so both models must use
/// CCR (`provider,model`) or neither.
pub fn effective_subagent_model<'a>(
    agent: AgentKind,
    model: Option<&str>,
    subagent_model: Option<&'a str>,
) -> Result<Option<&'a str>, String> {
    let Some(sub) = subagent_model.filter(|sub| Some(*sub) != model) else {
        return Ok(None);
    };
    match agent {
        AgentKind::Kiro => Err(format!(
            "a sub-agent model ({sub}) is not supported with Kiro: kiro-cli has no setting for it"
        )),
        AgentKind::Claude if claude_uses_ccr(model) != claude_uses_ccr(Some(sub)) => Err(format!(
            "Claude Code sends the main and the sub-agent requests to one endpoint: the main \
             model ({}) and the sub-agent model ({sub}) must both use CCR (`provider,model`) or \
             neither",
            model.unwrap_or("CLI default")
        )),
        AgentKind::Claude | AgentKind::OpenCode => Ok(Some(sub)),
    }
}

pub fn invoke_agent(invocation: AgentInvocation<'_>) -> Result<(), Box<dyn std::error::Error>> {
    let mut invocation = invocation;
    invocation.subagent_model = effective_subagent_model(
        invocation.agent,
        invocation.model,
        invocation.subagent_model,
    )?;
    let limits = match invocation.agent {
        AgentKind::OpenCode => opencode::load_registry_limits(&invocation)?,
        AgentKind::Claude | AgentKind::Kiro => RegistryLimits::default(),
    };
    let prompt = invocation.render_prompt(&limits);
    invocation.prompt = &prompt;
    ensure_workdir_git(&invocation.case.workspace());
    invocation.case.commit_links();

    prepare_agent_env(&invocation)?;

    let logs_dir = invocation.case.root().join("logs");
    fs::create_dir_all(&logs_dir)?;
    let log_path = logs_dir.join(invocation.phase.log_file_name());
    // Start the phase with an empty log; the first run and every resume append to it.
    fs::write(&log_path, "")?;

    let agent_display = match invocation.agent {
        AgentKind::Kiro => "Kiro",
        AgentKind::Claude => "Claude Code",
        AgentKind::OpenCode => "OpenCode",
    };
    // Phase markers mirrored into the shared output log so it is
    // self-sufficient for trace analysis: parse_trace derives the agent
    // *process* lifetime (stall detection, session-end inference) from the
    // "Invoking …" → "Exporting …"/"Appended …" ISO-timestamped window,
    // which previously only existed on stderr (i.e. in manually captured
    // `&>` trace files).
    append_output_log_line(
        invocation.output_log_path,
        &format!(
            "Invoking {agent_display} {} agent (marker)",
            invocation.phase.label()
        ),
    );

    let status = match invocation.agent {
        AgentKind::Kiro => kiro::invoke_kiro(&invocation, &log_path)?,
        AgentKind::Claude => claude::invoke_claude(&invocation, &log_path)?,
        AgentKind::OpenCode => opencode::invoke_opencode(&invocation, &log_path, &limits)?,
    };

    if !status.success() {
        warn!("{} agent exited with {status}", invocation.phase.label());
        // Mirror the abnormal exit into the shared output log: the agent's
        // own output is already there, but runner diagnostics otherwise
        // exist only on stderr.
        append_output_log_line(
            invocation.output_log_path,
            &format!(
                "WARNING: {} agent exited with {status}",
                invocation.phase.label()
            ),
        );
    }

    if invocation.agent == AgentKind::OpenCode {
        append_output_log_line(
            invocation.output_log_path,
            &format!(
                "Exporting OpenCode sessions ({} agent exited: {status})",
                invocation.phase.label()
            ),
        );
        if let Err(e) = opencode::export_opencode_sessions(&log_path, invocation.output_log_path) {
            warn!("OpenCode session export failed (non-fatal): {e}");
            append_output_log_line(
                invocation.output_log_path,
                &format!("WARNING: OpenCode session export failed (non-fatal): {e}"),
            );
        }
    }

    append_output_log_line(
        invocation.output_log_path,
        &format!(
            "Appended agent trace ({} phase done)",
            invocation.phase.label()
        ),
    );
    Ok(())
}

fn prepare_agent_env(invocation: &AgentInvocation<'_>) -> Result<(), Box<dyn std::error::Error>> {
    match invocation.agent {
        AgentKind::Kiro => {}
        AgentKind::Claude => claude::write_claude_sandbox(invocation.case.root())?,
        AgentKind::OpenCode => opencode::write_project(invocation)?,
    }
    Ok(())
}

/// The exit code of `timeout` when the deadline ends the command.
const TIMEOUT_EXIT_CODE: i32 = 124;

/// Environment variables a backend sets on its CLI process, around the run
/// config's `extra_env`.
#[derive(Default)]
struct BackendEnv {
    /// Set before `extra_env`, so a value from the run config wins.
    defaults: Vec<(&'static str, String)>,
    /// Set after `extra_env`: values the backend cannot run without.
    overrides: Vec<(&'static str, String)>,
}

/// Runs `program args` in the work dir under `timeout`, so the stage deadline
/// ends it with exit code 124. Its output is copied, as it comes, to the
/// per-agent log, the shared output log (if any) and our stdout.
fn run_agent(
    invocation: &AgentInvocation<'_>,
    log_path: &Path,
    timeout_secs: u64,
    program: &str,
    args: &[&str],
    env: &BackendEnv,
) -> Result<ExitStatus, Box<dyn std::error::Error>> {
    let openssl_dir = std::env::var("OPENSSL_DIR").unwrap_or_else(|_| "/usr".into());
    let mut cmd = Command::new("timeout");
    cmd.arg(timeout_secs.to_string())
        .arg(program)
        .args(args)
        .env("OPENSSL_DIR", openssl_dir)
        .current_dir(invocation.case.workspace())
        // OpenCode takes its project directory from $PWD before the real cwd,
        // and `current_dir` leaves the inherited $PWD pointing at ours.
        .env("PWD", std::path::absolute(invocation.case.workspace())?);

    let toolchain = &invocation.rust_toolchain.required_version;
    info!("Injecting RUSTUP_TOOLCHAIN={toolchain}");
    cmd.env("RUSTUP_TOOLCHAIN", toolchain);

    // Keep the temporary files of the agent and its commands (compilers,
    // linkers) in case_dir, which is on disk and is removed with the run.
    let tmp = invocation.case.tmp();
    info!("Injecting TMPDIR={}", tmp.display());
    cmd.env("TMPDIR", &tmp);

    for (key, value) in &env.defaults {
        info!("Injecting {key}={value}");
        cmd.env(key, value);
    }
    for (key, value) in invocation.extra_env {
        info!("Injecting env var: {key}");
        cmd.env(key, value);
    }
    for (key, value) in &env.overrides {
        info!("Injecting {key}={value}");
        cmd.env(key, value);
    }

    let mut sinks: Vec<(String, Box<dyn Write>)> = vec![(
        log_path.display().to_string(),
        Box::new(fs::OpenOptions::new().append(true).open(log_path)?),
    )];
    if let Some(output_log) = invocation.output_log_path {
        match fs::OpenOptions::new().append(true).open(output_log) {
            Ok(file) => sinks.push((output_log.display().to_string(), Box::new(file))),
            Err(e) => warn!(
                "Cannot open {} for the agent trace: {e}",
                output_log.display()
            ),
        }
    }
    sinks.push(("stdout".into(), Box::new(std::io::stdout())));
    end_with_parent(&mut cmd);
    Ok(run_teed(cmd, sinks)?)
}

/// Makes the kernel send `timeout` SIGTERM when the thread that spawns it
/// ends, so the agent cannot outlive the run.
///
/// GNU `timeout` moves itself into a process group of its own. A terminal
/// Ctrl+C signals only the foreground group, so without this it ends the
/// benchmark and leaves `timeout` and the agent running under PID 1 until the
/// stage deadline. On SIGTERM, `timeout` sends the signal to its whole process
/// group, which includes the agent. This also covers a SIGKILL of the
/// benchmark. The spawning thread waits for the child (see `run_teed`), so it
/// does not end first.
fn end_with_parent(cmd: &mut Command) {
    use std::os::unix::process::CommandExt;
    let parent = std::process::id();
    // SAFETY: the closure runs in the child between fork and exec. It calls
    // only async-signal-safe functions and does not allocate.
    unsafe {
        cmd.pre_exec(move || {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            // The parent can exit before the prctl call; then no signal comes.
            if libc::getppid() as u32 != parent {
                return Err(std::io::Error::from_raw_os_error(libc::ESRCH));
            }
            Ok(())
        });
    }
}

/// Runs `cmd` with stdin closed and stdout and stderr merged into one pipe,
/// writing everything it prints to every sink as it arrives. Returns the
/// command's own exit status.
fn run_teed(
    mut cmd: Command,
    mut sinks: Vec<(String, Box<dyn Write>)>,
) -> std::io::Result<ExitStatus> {
    let (mut reader, writer) = std::io::pipe()?;
    cmd.stdin(Stdio::null())
        .stdout(writer.try_clone()?)
        .stderr(writer);
    let mut child = cmd.spawn()?;
    // `cmd` holds our copies of the pipe's write end; the read below only
    // sees EOF once they are closed.
    drop(cmd);
    let mut buf = [0u8; 8192];
    loop {
        let n = match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => {
                warn!("Reading the agent output failed: {e}");
                break;
            }
        };
        sinks.retain_mut(|(name, sink)| match sink.write_all(&buf[..n]) {
            Ok(()) => true,
            Err(e) => {
                warn!("Writing the agent output to {name} failed: {e}");
                false
            }
        });
    }
    for (_, sink) in &mut sinks {
        let _ = sink.flush();
    }
    child.wait()
}

/// Prepare the agent working directory as a git repository. A fresh
/// directory gets `git init`, a local identity (agents run without a global
/// git config), a `.gitignore` that keeps build outputs and worktrees
/// untracked, and a baseline commit. An existing repository only has stale
/// worktree metadata pruned, so a resumed stage keeps its history.
pub fn ensure_workdir_git(work_dir: &Path) {
    let git = |args: &[&str]| -> Result<ExitStatus, std::io::Error> {
        Command::new("git")
            .args(args)
            .current_dir(work_dir)
            .output()
            .map(|out| out.status)
    };
    let ok = |args: &[&str], what: &str| {
        if let Err(e) = git(args) {
            warn!("git {what} failed in {}: {e}", work_dir.display());
        }
    };

    if work_dir.join(".git").exists() {
        ok(&["worktree", "prune"], "worktree prune");
        return;
    }
    match git(&["init", "-q", "-b", "main"]) {
        Ok(status) if status.success() => {}
        Ok(status) => {
            warn!(
                "git init exited with {status} in {}: continuing without git",
                work_dir.display()
            );
            return;
        }
        Err(e) => {
            warn!(
                "git unavailable in {}: {e}; continuing without the git work record",
                work_dir.display()
            );
            return;
        }
    }
    ok(
        &["config", "user.name", "harvest-agent"],
        "config user.name",
    );
    ok(
        &["config", "user.email", "harvest-agent@invalid"],
        "config user.email",
    );
    if let Err(e) = fs::write(
        work_dir.join(".gitignore"),
        "# Framework-written: build outputs, scratch and worktrees stay untracked.\ntarget/\nbuild*/\ncbuild/\nscratch/\nwt/\n.opencode/\n",
    ) {
        warn!("failed to write .gitignore in {}: {e}", work_dir.display());
    }
    ok(&["add", "-A"], "add");
    ok(
        &["commit", "-q", "-m", "framework: baseline before agent run"],
        "baseline commit",
    );
    info!("initialized git work record in {}", work_dir.display());
}

/// Appends one ISO-timestamped runner marker line to the shared output log,
/// formatted like a tracing log line so `parse_trace.py` picks it up with the
/// same regexes it already uses on stderr-captured traces. Best-effort: a
/// missing/unwritable output log is not an error.
fn append_output_log_line(output_log_path: Option<&Path>, msg: &str) {
    let Some(out_path) = output_log_path else {
        return;
    };
    if let Ok(mut f) = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(out_path)
    {
        let _ = writeln!(f, "{}  INFO agent_runner: {}", iso_utc_now(), msg);
    }
}

/// Current UTC time as ISO-8601 (`2026-07-18T12:34:56.789Z`) without a chrono
/// dependency (civil-from-days algorithm).
fn iso_utc_now() -> String {
    let d = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = d.as_secs() as i64;
    let millis = d.subsec_millis();
    let days = secs.div_euclid(86400);
    let sod = secs.rem_euclid(86400);
    let (h, m, s) = (sod / 3600, (sod % 3600) / 60, sod % 60);
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { y + 1 } else { y };
    format!("{year:04}-{month:02}-{day:02}T{h:02}:{m:02}:{s:02}.{millis:03}Z")
}

fn command_stdout(cmd: &str, args: &[&str]) -> Result<String, Box<dyn std::error::Error>> {
    let output = Command::new(cmd).args(args).output()?;
    if !output.status.success() {
        return Err(format!(
            "{} {} failed with status {}: {}",
            cmd,
            args.join(" "),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effective_subagent_model_normalizes_and_checks() {
        let eff = effective_subagent_model;
        // Absent or equal to the main model: the sub-agents use the main model.
        assert_eq!(eff(AgentKind::Claude, Some("sonnet"), None), Ok(None));
        assert_eq!(
            eff(AgentKind::Claude, Some("sonnet"), Some("sonnet")),
            Ok(None)
        );
        assert_eq!(eff(AgentKind::Kiro, Some("x"), Some("x")), Ok(None));
        assert_eq!(
            eff(AgentKind::Claude, None, Some("haiku")),
            Ok(Some("haiku"))
        );
        assert_eq!(
            eff(AgentKind::Claude, Some("sonnet"), Some("haiku")),
            Ok(Some("haiku"))
        );
        assert_eq!(
            eff(AgentKind::OpenCode, Some("a/b"), Some("c/d")),
            Ok(Some("c/d"))
        );
        assert_eq!(
            eff(
                AgentKind::Claude,
                Some("openrouter,a/b"),
                Some("openrouter,c/d")
            ),
            Ok(Some("openrouter,c/d"))
        );
        // Unsupported setups.
        assert!(eff(AgentKind::Kiro, None, Some("x")).is_err());
        assert!(eff(AgentKind::Claude, Some("sonnet"), Some("openrouter,a/b")).is_err());
    }

    fn file_sink(path: &Path) -> (String, Box<dyn Write>) {
        let file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .expect("open sink");
        (path.display().to_string(), Box::new(file))
    }

    struct BrokenSink;

    impl Write for BrokenSink {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("broken"))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn run_teed_streams_merged_output_to_every_sink() {
        let dir = tempfile::tempdir().expect("tempdir");
        let log = dir.path().join("agent.log");
        let output_log = dir.path().join("output.log");
        // "second" is printed only once "first" is in the output log, so the
        // copy must happen while the command still runs.
        let mut cmd = Command::new("bash");
        cmd.args([
            "-c",
            "echo first; until grep -q first \"$1\"; do sleep 0.05; done; \
             echo second >&2; exit 3",
            "_",
        ])
        .arg(&output_log);
        let sinks = vec![
            file_sink(&log),
            ("broken".into(), Box::new(BrokenSink) as Box<dyn Write>),
            file_sink(&output_log),
        ];
        let status = run_teed(cmd, sinks).expect("run");
        assert_eq!(status.code(), Some(3));
        assert_eq!(fs::read_to_string(&log).unwrap(), "first\nsecond\n");
        assert_eq!(fs::read_to_string(&output_log).unwrap(), "first\nsecond\n");
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn child_gets_sigterm_when_the_spawning_thread_ends() {
        use std::os::unix::process::ExitStatusExt;
        let mut child = std::thread::spawn(|| {
            let mut cmd = Command::new("sleep");
            cmd.arg("30");
            end_with_parent(&mut cmd);
            cmd.spawn().expect("spawn sleep")
        })
        .join()
        .expect("spawning thread");
        let status = child.wait().expect("wait");
        assert_eq!(status.signal(), Some(libc::SIGTERM), "{status}");
    }

    #[test]
    #[cfg_attr(miri, ignore)] // spawns git; Miri cannot run subprocesses
    fn ensure_workdir_git_creates_repo_and_stays_idempotent() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("src.txt"), "content").expect("write file");
        ensure_workdir_git(dir.path());
        assert!(dir.path().join(".git").is_dir(), "repository initialized");
        assert!(dir.path().join(".gitignore").is_file(), "gitignore written");
        let log = String::from_utf8(
            Command::new("git")
                .args(["log", "--oneline"])
                .current_dir(dir.path())
                .output()
                .expect("git log")
                .stdout,
        )
        .expect("utf8");
        assert!(log.contains("baseline"), "baseline commit present: {log}");
        // Second call must not re-init or add a second commit.
        ensure_workdir_git(dir.path());
        let count = String::from_utf8(
            Command::new("git")
                .args(["rev-list", "--count", "HEAD"])
                .current_dir(dir.path())
                .output()
                .expect("rev-list")
                .stdout,
        )
        .expect("utf8");
        assert_eq!(count.trim(), "1", "no extra commit on re-ensure");
    }
}
