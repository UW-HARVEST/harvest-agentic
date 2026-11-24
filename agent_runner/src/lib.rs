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
mod opencode;
mod prompt;
#[cfg(any(test, feature = "test-support"))]
pub mod prompt_check;
mod skills;
mod toolchain;

pub use case_dir::CaseDir;
use claude::claude_uses_ccr;
pub use claude::claude_uses_teamclaude;
use prompt::RegistryLimits;
pub use prompt::{AdditionalPrompt, PROMPTS_PATH, StageFact, main_prompt, tool_wishlist};
pub use toolchain::{RustToolchainContext, detect_rust_toolchain_context, locate_test_corpus};

#[derive(Debug, Clone, Copy)]
pub enum AgentPhase {
    Translate,
    Verify,
    Quality,
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
            AgentPhase::Quality => "quality",
            AgentPhase::Conform => "conformance",
        }
    }

    fn log_file_name(self) -> &'static str {
        match self {
            AgentPhase::Translate => "translation.log",
            AgentPhase::Verify => "verify.log",
            AgentPhase::Quality => "quality.log",
            AgentPhase::Conform => "conform.log",
        }
    }

    /// The directory of the stage's prompt files in `inputs/prompts/`.
    pub fn prompt_dir(self) -> &'static str {
        match self {
            AgentPhase::Translate => "translate",
            AgentPhase::Verify => "verify",
            AgentPhase::Quality => "quality",
            AgentPhase::Conform => "conform",
        }
    }

    /// The files that the main agent must read again after a context compaction: the rules of
    /// every main agent, the prompt file of the stage, and the state files in the workspace.
    /// Only meaningful when plan files are enabled.
    fn recovery_files(self) -> Vec<String> {
        let state: &[&str] = match self {
            AgentPhase::Translate => &["PLAN.md"],
            AgentPhase::Verify => &["PLAN.md", "HYPOTHESES.md"],
            AgentPhase::Quality => &["QUALITY.md"],
            AgentPhase::Conform => &["CONFORM.md"],
        };
        let rules = [
            format!("{PROMPTS_PATH}/main_agent.md"),
            format!("{PROMPTS_PATH}/{}/main.md", self.prompt_dir()),
        ];
        rules
            .into_iter()
            .chain(state.iter().map(|file| file.to_string()))
            .collect()
    }
}

pub const DEFAULT_AGENT_TIMEOUT_SECS: u64 = 48 * 60 * 60;

pub struct AgentInvocation<'a> {
    pub phase: AgentPhase,
    pub agent: AgentKind,
    /// The run directory. The agent works in its workspace.
    pub case: &'a CaseDir,
    /// The prompt of the main agent: [`main_prompt`], or the whole task for a stage that runs
    /// without plan files.
    pub prompt: &'a str,
    /// The prompt files that the options of this run add for the main agent.
    /// [`prepare_agent`] writes them, and `run.md` lists them.
    pub additional_prompts: &'a [AdditionalPrompt],
    /// The facts of the stage that `run.md` gives after the common ones.
    pub stage_facts: &'a [StageFact<'a>],
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
    /// Whether the main agent keeps a state file (PLAN.md, HYPOTHESES.md, QUALITY.md or
    /// CONFORM.md). Without one (`no_plan`), nothing tells the agent what to read again after a
    /// compaction, so the backends add no compaction-recovery text, and the run gets no
    /// `main_agent.md`.
    fn plan_files_enabled(&self) -> bool {
        !self.no_plan
    }
}

/// The sub-agent model of a run: `None` when the sub-agents use the main
/// model (none given, or the same as `model`), which makes the run identical to
/// one without a sub-agent model. Errors when Claude Code cannot give its
/// sub-agents a model of their own: it routes all of its requests through one
/// endpoint, so both models must use CCR (`provider,model`) or neither.
pub fn effective_subagent_model<'a>(
    agent: AgentKind,
    model: Option<&str>,
    subagent_model: Option<&'a str>,
) -> Result<Option<&'a str>, String> {
    let Some(sub) = subagent_model.filter(|sub| Some(*sub) != model) else {
        return Ok(None);
    };
    if agent == AgentKind::Claude && claude_uses_ccr(model) != claude_uses_ccr(Some(sub)) {
        return Err(format!(
            "Claude Code sends the main and the sub-agent requests to one endpoint: the main \
             model ({}) and the sub-agent model ({sub}) must both use CCR (`provider,model`) or \
             neither",
            model.unwrap_or("CLI default")
        ));
    }
    Ok(Some(sub))
}

/// An agent that is ready to run: its models are resolved and the prompt files that every stage
/// gets are written. Before [`PreparedAgent::run`], the tool writes the prompt files of its stage
/// ([`CaseDir::write_prompts`]) and locks the inputs ([`CaseDir::lock_inputs`]).
pub struct PreparedAgent<'a> {
    invocation: AgentInvocation<'a>,
    limits: RegistryLimits,
    /// The OpenRouter routes of the models (OpenCode only).
    routes: Vec<opencode::Route<'a>>,
}

/// Resolves the models of the stage and their registry limits, and writes the prompt files that
/// every stage gets into `inputs/prompts/`: the shared rules, the additional prompt files of the
/// run, and `run.md`, the facts of the run.
/// Call it before [`CaseDir::lock_inputs`].
pub fn prepare_agent(
    invocation: AgentInvocation<'_>,
) -> Result<PreparedAgent<'_>, Box<dyn std::error::Error>> {
    let mut invocation = invocation;
    invocation.subagent_model = effective_subagent_model(
        invocation.agent,
        invocation.model,
        invocation.subagent_model,
    )?;
    let routes = match invocation.agent {
        AgentKind::OpenCode => {
            opencode::take_routes(&mut invocation.model, &mut invocation.subagent_model)?
        }
        AgentKind::Claude => Vec::new(),
    };
    for route in &routes {
        match &route.only {
            Some(only) => info!(
                "OpenRouter route of {}: only {}",
                route.model_id,
                only.join(",")
            ),
            None => info!("OpenRouter route of {}: default routing", route.model_id),
        }
    }
    let limits = match invocation.agent {
        AgentKind::OpenCode => opencode::load_registry_limits(&invocation)?,
        AgentKind::Claude => RegistryLimits::default(),
    };
    invocation.write_shared_prompts(&limits)?;
    Ok(PreparedAgent {
        invocation,
        limits,
        routes,
    })
}

impl PreparedAgent<'_> {
    /// Runs the agent on its prompt, to the end of the stage.
    pub fn run(self) -> Result<(), Box<dyn std::error::Error>> {
        run_prepared(&self.invocation, &self.limits, &self.routes)
    }
}

fn run_prepared(
    invocation: &AgentInvocation<'_>,
    limits: &RegistryLimits,
    routes: &[opencode::Route<'_>],
) -> Result<(), Box<dyn std::error::Error>> {
    ensure_workdir_git(&invocation.case.workspace());
    invocation.case.commit_links();

    prepare_agent_env(invocation, routes)?;

    let logs_dir = invocation.case.root().join("logs");
    fs::create_dir_all(&logs_dir)?;
    let log_path = logs_dir.join(invocation.phase.log_file_name());
    // Start the phase with an empty log; the first run and every resume append to it.
    fs::write(&log_path, "")?;

    let agent_display = match invocation.agent {
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
        AgentKind::Claude => claude::invoke_claude(invocation, &log_path)?,
        AgentKind::OpenCode => opencode::invoke_opencode(invocation, &log_path, limits)?,
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

fn prepare_agent_env(
    invocation: &AgentInvocation<'_>,
    routes: &[opencode::Route<'_>],
) -> Result<(), Box<dyn std::error::Error>> {
    match invocation.agent {
        AgentKind::Claude => claude::write_claude_sandbox(invocation.case.root()),
        AgentKind::OpenCode => opencode::write_project(invocation, routes),
    }
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
/// ends it with exit code 124. `prompt` is its stdin. Its output is
/// copied, as it comes, to the per-agent log, the shared output log (if any)
/// and our stdout.
fn run_agent(
    invocation: &AgentInvocation<'_>,
    log_path: &Path,
    timeout_secs: u64,
    program: &str,
    args: &[&str],
    prompt: &str,
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
    // The prompt goes on stdin, not on the command line: there, an agent's
    // `pgrep -f`/`pkill -f` for a word of its task matches `timeout` and the
    // CLI, and the agent kills its own run.
    let prompt_path = log_path.with_extension("prompt");
    fs::write(&prompt_path, prompt)?;
    let stdin = Stdio::from(fs::File::open(&prompt_path)?);
    end_with_parent(&mut cmd);
    Ok(run_teed(cmd, stdin, sinks)?)
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

/// Runs `cmd` with `stdin` and with stdout and stderr merged into one pipe,
/// writing everything it prints to every sink as it arrives. Returns the
/// command's own exit status.
fn run_teed(
    mut cmd: Command,
    stdin: Stdio,
    mut sinks: Vec<(String, Box<dyn Write>)>,
) -> std::io::Result<ExitStatus> {
    let (mut reader, writer) = std::io::pipe()?;
    cmd.stdin(stdin).stdout(writer.try_clone()?).stderr(writer);
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

/// The framework's `.gitignore` entries: build outputs, scratch and worktrees stay untracked.
const GITIGNORE_ENTRIES: [&str; 6] = [
    "target/",
    "build*/",
    "cbuild/",
    "scratch/",
    "wt/",
    ".opencode/",
];

/// More `.gitignore` entries, only for a workspace that has the FuzzTest scaffold of the verify
/// stage ([`FUZZ_SCAFFOLD`]). A FuzzTest build installs its ANTLR runtime into `verify_env/dist`,
/// outside its build directory. Of a fuzz campaign, git records only the `reproducers/`. A
/// workspace without the scaffold does not get these entries: its `.gitignore` must not tell the
/// agent about a mechanism that the run does not use.
const FUZZ_GITIGNORE_ENTRIES: [&str; 5] = [
    "verify_env/dist/",
    "verify_env/clang-rt/",
    "verify_env/fuzz-artifacts/*/corpus/",
    "verify_env/fuzz-artifacts/*/run.log",
    "verify_env/fuzz-artifacts/*/exit_status",
];

/// The file of the workspace that the FuzzTest scaffold always has.
const FUZZ_SCAFFOLD: &str = "verify_env/run_fuzz.sh";

/// The `.gitignore` entries that the workspace needs.
fn gitignore_entries(work_dir: &Path) -> Vec<&'static str> {
    let fuzz = work_dir.join(FUZZ_SCAFFOLD).exists();
    GITIGNORE_ENTRIES
        .into_iter()
        .chain(FUZZ_GITIGNORE_ENTRIES.into_iter().filter(|_| fuzz))
        .collect()
}

/// Adds the framework's `.gitignore` entries that an existing repository lacks: snapshots made
/// before an entry was introduced (`scratch/`) would otherwise commit what it should hide.
fn complete_gitignore(work_dir: &Path) -> std::io::Result<()> {
    let path = work_dir.join(".gitignore");
    let mut text = fs::read_to_string(&path).unwrap_or_default();
    let missing: Vec<&str> = gitignore_entries(work_dir)
        .into_iter()
        .filter(|entry| !text.lines().any(|line| line.trim() == *entry))
        .collect();
    if missing.is_empty() {
        return Ok(());
    }
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    for entry in missing {
        text.push_str(entry);
        text.push('\n');
    }
    fs::write(path, text)
}

/// Prepare the agent working directory as a git repository. A fresh
/// directory gets `git init`, a local identity (agents run without a global
/// git config), a `.gitignore` that keeps build outputs and worktrees
/// untracked, and a baseline commit. An existing repository only has stale
/// worktree metadata pruned and its `.gitignore` completed, so a resumed stage
/// keeps its history.
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
        if let Err(e) = complete_gitignore(work_dir) {
            warn!(
                "failed to complete .gitignore in {}: {e}",
                work_dir.display()
            );
        }
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
    let gitignore = format!(
        "# Framework-written: build outputs, scratch and worktrees stay untracked.\n{}\n",
        gitignore_entries(work_dir).join("\n")
    );
    if let Err(e) = fs::write(work_dir.join(".gitignore"), gitignore) {
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
        assert!(eff(AgentKind::Claude, Some("sonnet"), Some("openrouter,a/b")).is_err());
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn resumed_repository_gets_missing_gitignore_entries() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("src.txt"), "content").expect("write file");
        ensure_workdir_git(dir.path());
        // A snapshot from before `scratch/` was ignored.
        fs::write(dir.path().join(".gitignore"), "# old\ntarget/\nwt/").expect("write gitignore");
        ensure_workdir_git(dir.path());
        let text = fs::read_to_string(dir.path().join(".gitignore")).expect("read");
        assert_eq!(
            text,
            "# old\ntarget/\nwt/\nbuild*/\ncbuild/\nscratch/\n.opencode/\n"
        );
        ensure_workdir_git(dir.path());
        assert_eq!(
            fs::read_to_string(dir.path().join(".gitignore")).expect("read"),
            text
        );
    }

    /// A fresh repository and a resumed one with an old `.gitignore`: `git add -A` records the
    /// sources and the fuzz reproducers of verify_env, never its build output or campaign data.
    #[test]
    #[cfg_attr(miri, ignore)]
    fn verify_env_records_only_sources_and_reproducers() {
        let campaign = "verify_env/fuzz-artifacts/Suite.Prop.ab12";
        // In the order of `git ls-files`.
        let recorded = [
            format!("{campaign}/reproducers/input"),
            FUZZ_SCAFFOLD.to_owned(),
            "verify_env/verification_tests.cc".to_owned(),
        ];
        let products = [
            "verify_env/dist/libantlr4-runtime.a".to_owned(),
            "verify_env/clang-rt/libclang_rt.asan.a".to_owned(),
            "verify_env/cbuild/a.o".to_owned(),
            "verify_env/build-fuzz/verification_tests".to_owned(),
            format!("{campaign}/corpus/input"),
            format!("{campaign}/run.log"),
            format!("{campaign}/exit_status"),
        ];
        for old_gitignore in [None, Some("target/\nwt/\n")] {
            let dir = tempfile::tempdir().expect("tempdir");
            // The workspace of a verify stage with FuzzTest: the scaffold is there first.
            fs::create_dir_all(dir.path().join("verify_env")).expect("mkdir");
            fs::write(dir.path().join(FUZZ_SCAFFOLD), "x").expect("write scaffold");
            ensure_workdir_git(dir.path());
            if let Some(text) = old_gitignore {
                fs::write(dir.path().join(".gitignore"), text).expect("write gitignore");
                ensure_workdir_git(dir.path());
            }
            for rel in recorded.iter().chain(&products) {
                let path = dir.path().join(rel);
                fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
                fs::write(path, "x").expect("write file");
            }
            let git = |args: &[&str]| {
                let out = Command::new("git")
                    .args(args)
                    .current_dir(dir.path())
                    .output()
                    .expect("git");
                assert!(out.status.success(), "git {args:?}");
                String::from_utf8(out.stdout).expect("utf8")
            };
            git(&["add", "-A"]);
            let tracked = git(&["ls-files", "verify_env"]);
            assert_eq!(
                tracked.lines().collect::<Vec<_>>(),
                recorded,
                "{old_gitignore:?}"
            );
        }
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
        let status = run_teed(cmd, Stdio::null(), sinks).expect("run");
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
