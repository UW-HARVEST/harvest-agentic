use harvest_core::config::{AgentKind, Profile};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use tracing::{info, warn};

mod agent_env;
mod arm;
pub mod case_dir;
mod claude;
mod dry_run;
mod given;
mod opencode;
mod prompt;
#[cfg(any(test, feature = "test-support"))]
pub mod prompt_check;
mod shared;
mod skills;
mod toolchain;

pub use agent_env::agent_command;
pub use arm::{Arm, Baseline, Hook, HookContext, no_hook};
pub use case_dir::CaseDir;
use claude::claude_uses_ccr;
pub use claude::claude_uses_teamclaude;
pub use dry_run::DryRun;
pub use given::hash_file;
use prompt::RegistryLimits;
pub use prompt::{AdditionalPrompt, PROMPTS_PATH, StageFact, main_prompt, tool_wishlist};
pub use shared::shared_reading_list;
pub use skills::materialize as materialize_skill;
pub use toolchain::{RustToolchainContext, detect_rust_toolchain_context, locate_test_corpus};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

    /// The files that the main agent of a HARVEST stage must read again after a context
    /// compaction: the rules of every main agent, the prompt file of the stage, and the state
    /// files in the workspace.
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
    /// The arm of the study that the stage runs as. It decides the prompt of the main agent,
    /// the files that [`prepare_agent`] writes, and the extras of the agent runtime.
    pub arm: Arm<'a>,
    pub timeout_secs: u64,
    pub model: Option<&'a str>,
    pub subagent_model: Option<&'a str>,
    pub teamclaude: bool,
    pub extra_env: &'a HashMap<String, String>,
    pub output_log_path: Option<&'a Path>,
    /// A directory of the output that keeps what the agents of the stage are given (see
    /// `given::record`). `None`: no record.
    pub prompt_record_dir: Option<&'a Path>,
    pub rust_toolchain: &'a RustToolchainContext,
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

/// An agent that is ready to run: its models are resolved and the files of its arm are written.
/// Before [`PreparedAgent::run`], the tool of a HARVEST stage writes the prompt files of its
/// stage ([`CaseDir::write_prompts`]), and every tool locks the inputs
/// ([`CaseDir::lock_inputs`]).
pub struct PreparedAgent<'a> {
    invocation: AgentInvocation<'a>,
    /// The files that the workspace had before the framework wrote anything: the crate of the
    /// earlier stage (see `given::earlier_stage`).
    earlier: BTreeMap<String, String>,
    limits: RegistryLimits,
    /// The host can set the shared memory limit of the agent commands (see `agent_env`).
    shared_limit: bool,
    /// The OpenRouter routes of the models (OpenCode only).
    routes: Vec<opencode::Route<'a>>,
}

/// Resolves the models of the stage and their registry limits, and writes what the arm gives
/// every stage (see `AgentInvocation::write_files`). A baseline arm takes no sub-agent model.
/// Call it when the workspace holds only the crate of the earlier stage, if there is one, and
/// before [`CaseDir::lock_inputs`].
pub fn prepare_agent(
    invocation: AgentInvocation<'_>,
) -> Result<PreparedAgent<'_>, Box<dyn std::error::Error>> {
    let mut invocation = invocation;
    if let (Arm::Baseline(baseline), Some(sub)) = (invocation.arm, invocation.subagent_model) {
        return Err(format!(
            "subagent_model = {sub} is an option of the HARVEST pipeline: the {} baseline does \
             not take it",
            baseline.profile
        )
        .into());
    }
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
    // Only a check here: the run sets the limit (`agent_env::install`), and a test of the
    // prepared files must not change the host.
    let earlier = given::earlier_stage(invocation.case)?;
    let shared_limit = agent_env::limit_available();
    invocation.write_files(&limits, shared_limit)?;
    Ok(PreparedAgent {
        invocation,
        earlier,
        limits,
        shared_limit,
        routes,
    })
}

impl PreparedAgent<'_> {
    /// Runs the agent on its prompt, to the end of the stage, and returns `None`. In a dry-run
    /// (the case directory is the one that [`CaseDir::for_run`] makes for a dry-run) no agent
    /// starts: the stage stops when everything that the agent receives is ready, and the
    /// [`DryRun`] comes back. The tool then returns it in place of its result.
    pub fn run(self) -> Result<Option<DryRun>, Box<dyn std::error::Error>> {
        let (invocation, limits) = (&self.invocation, &self.limits);
        let dry_run = invocation.case.dry_run();
        // The shell that runs the agent commands under the shared memory limit. A dry-run does
        // not set the limit on the host.
        let limit = match dry_run {
            None => agent_env::Limits::Set,
            Some(_) => agent_env::Limits::Available(self.shared_limit),
        };
        let shell = agent_env::install(invocation.case.root(), limit);
        let shell = shell.as_deref();
        let env = invocation.environment(shell, limits)?;
        let started = start_stage(invocation, &self.routes, shell, &self.earlier, &env)?;
        if let Some(dir) = dry_run {
            return Ok(Some(DryRun::keep(invocation, &started.prompt, &env, dir)?));
        }
        end_stage(invocation, &started, |prompt, log_path| {
            match invocation.agent {
                AgentKind::Claude => claude::invoke_claude(invocation, prompt, log_path, &env),
                AgentKind::OpenCode => {
                    opencode::invoke_opencode(invocation, prompt, log_path, &env)
                }
            }
        })?;
        Ok(None)
    }
}

/// A stage at the moment when its agent starts.
struct StartedStage {
    /// The prompt of the main agent.
    prompt: String,
    /// The log of the agent, empty.
    log_path: PathBuf,
}

/// The stage before its agent: makes the workspace ready. A baseline arm has a hook immediately
/// before the agent starts. The record of what the agent is given (see `given::record`) is made
/// after the hook, so that it has what the hook wrote. A baseline stage fails when the record
/// cannot be kept: the record is the proof of what the arm received. `env` are the variables of
/// the agent process, for the record. A run, a dry-run and the tests of what an agent receives
/// all start a stage here.
fn start_stage(
    invocation: &AgentInvocation<'_>,
    routes: &[opencode::Route<'_>],
    shell: Option<&Path>,
    earlier: &BTreeMap<String, String>,
    env: &[EnvVar],
) -> Result<StartedStage, Box<dyn std::error::Error>> {
    set_up_workspace(invocation, routes, shell)?;
    let prompt = invocation.prompt();
    let baseline = invocation.arm.baseline();
    let kept = |result: Result<(), Box<dyn std::error::Error>>, record: &Path| match result {
        Err(e) if baseline.is_some() => Err(format!(
            "cannot keep the record of what the agent is given in {}: {e}",
            record.display()
        )),
        Err(e) => {
            warn!(
                "Cannot keep the record of what the agent is given in {}: {e}",
                record.display()
            );
            Ok(())
        }
        Ok(()) => Ok(()),
    };
    if let Some(record) = invocation.prompt_record_dir {
        kept(given::start_record(record).map_err(Into::into), record)?;
    }
    if let Some(baseline) = baseline {
        (baseline.before_agent)(&invocation.hook_context())?;
    }

    let logs_dir = invocation.case.root().join("logs");
    fs::create_dir_all(&logs_dir)?;
    let log_path = logs_dir.join(invocation.phase.log_file_name());
    // Start the phase with an empty log; the first run and every resume append to it.
    fs::write(&log_path, "")?;

    if let Some(record) = invocation.prompt_record_dir {
        kept(
            given::record(invocation, &prompt, earlier, env, record),
            record,
        )?;
    }
    Ok(StartedStage { prompt, log_path })
}

/// The stage from the start of its agent on: runs `agent` on the prompt of the main agent with
/// the path of its log. A baseline arm has a hook when the agent has ended, also when the agent
/// could not run.
fn end_stage(
    invocation: &AgentInvocation<'_>,
    started: &StartedStage,
    agent: impl FnOnce(&str, &Path) -> Result<ExitStatus, Box<dyn std::error::Error>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let baseline = invocation.arm.baseline();
    let hook_context = invocation.hook_context();
    let log_path = &started.log_path;
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

    let status = match agent(&started.prompt, log_path) {
        Ok(status) => status,
        Err(e) => {
            // The hook runs also when the agent could not run. The first error is the result.
            if let Some(baseline) = baseline
                && let Err(hook_error) = (baseline.after_agent)(&hook_context)
            {
                warn!("The hook after the agent failed: {hook_error}");
            }
            return Err(e);
        }
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
        if let Err(e) = opencode::export_opencode_sessions(log_path, invocation.output_log_path) {
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
    if let Some(baseline) = baseline {
        (baseline.after_agent)(&hook_context)?;
    }
    Ok(())
}

impl AgentInvocation<'_> {
    fn hook_context(&self) -> HookContext<'_> {
        HookContext {
            case: self.case,
            rust_toolchain: &self.rust_toolchain.required_version,
            record_dir: self.prompt_record_dir,
        }
    }
}

/// Makes the workspace as the agent finds it: the git repository with the framework's
/// `.gitignore`, the links to `inputs/` and `shared/` in git, and the project files of the agent
/// runtime. `shell` runs the agent commands (see `agent_env`).
fn set_up_workspace(
    invocation: &AgentInvocation<'_>,
    routes: &[opencode::Route<'_>],
    shell: Option<&Path>,
) -> Result<(), Box<dyn std::error::Error>> {
    ensure_workdir_git(&invocation.case.workspace(), invocation.arm.profile());
    invocation.case.commit_links();
    match invocation.agent {
        AgentKind::Claude => claude::write_claude_sandbox(invocation.case.root()),
        AgentKind::OpenCode => opencode::write_project(invocation, routes, shell),
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

/// One environment variable that the framework sets on the agent process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EnvVar {
    pub(crate) name: String,
    pub(crate) value: String,
    /// The value is from the `env` table of the run config, which can hold the key of a
    /// provider: a log and a file for a person give only the name.
    pub(crate) from_config: bool,
}

impl AgentInvocation<'_> {
    /// Every environment variable that the framework sets on the agent process, each with its
    /// final value: `PWD`, the Rust toolchain, `TMPDIR`, the defaults of the backend, the `env`
    /// table of the run config (in the order of the names) and the overrides of the backend. A
    /// later one of these replaces an earlier one of the same name. All other variables of the
    /// agent are those of the framework process. `shell` runs the agent commands (see
    /// `agent_env`).
    pub(crate) fn environment(
        &self,
        shell: Option<&Path>,
        limits: &RegistryLimits,
    ) -> std::io::Result<Vec<EnvVar>> {
        let backend = match self.agent {
            AgentKind::Claude => claude::backend_env(self, shell),
            AgentKind::OpenCode => opencode::backend_env(self, limits),
        };
        let own = |(name, value): (&str, String)| (name.to_owned(), value, false);
        // OpenCode takes its project directory from $PWD before the real cwd, and the working
        // directory of the command leaves the inherited $PWD pointing at ours.
        let workspace = std::path::absolute(self.case.workspace())?;
        let fixed = [
            ("PWD", workspace.to_string_lossy().into_owned()),
            (
                "RUSTUP_TOOLCHAIN",
                self.rust_toolchain.required_version.clone(),
            ),
            // Keep the temporary files of the agent and its commands (compilers, linkers) in
            // case_dir, which is on disk and is removed with the run.
            ("TMPDIR", self.case.tmp().to_string_lossy().into_owned()),
        ];
        let mut from_config: Vec<(&String, &String)> = self.extra_env.iter().collect();
        from_config.sort();
        let all = (fixed.into_iter().map(own))
            .chain(backend.defaults.into_iter().map(own))
            .chain(
                from_config
                    .into_iter()
                    .map(|(name, value)| (name.clone(), value.clone(), true)),
            )
            .chain(backend.overrides.into_iter().map(own));
        let mut vars: Vec<EnvVar> = Vec::new();
        for (name, value, from_config) in all {
            vars.retain(|var| var.name != name);
            vars.push(EnvVar {
                name,
                value,
                from_config,
            });
        }
        Ok(vars)
    }
}

/// Runs `program args` in the work dir under `timeout`, so the stage deadline
/// ends it with exit code 124. `prompt` is its stdin, and `env` are the
/// variables that the framework sets on it ([`AgentInvocation::environment`]).
/// Its output is copied, as it comes, to the per-agent log, the shared output
/// log (if any) and our stdout.
fn run_agent(
    invocation: &AgentInvocation<'_>,
    log_path: &Path,
    timeout_secs: u64,
    program: &str,
    args: &[&str],
    prompt: &str,
    env: &[EnvVar],
) -> Result<ExitStatus, Box<dyn std::error::Error>> {
    let mut cmd = Command::new("timeout");
    cmd.arg(timeout_secs.to_string())
        .arg(program)
        .args(args)
        .current_dir(invocation.case.workspace());
    for var in env {
        if var.from_config {
            info!("Injecting env var: {}", var.name);
        } else {
            info!("Injecting {}={}", var.name, var.value);
        }
        cmd.env(&var.name, &var.value);
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
/// Only the HARVEST prompts make git worktrees, so a baseline does not get [`WORKTREES_ENTRY`].
const GITIGNORE_ENTRIES: [&str; 6] = [
    "target/",
    "build*/",
    "cbuild/",
    "scratch/",
    WORKTREES_ENTRY,
    ".opencode/",
];
const WORKTREES_ENTRY: &str = "wt/";

/// More `.gitignore` entries, only for a workspace that has the FuzzTest scaffold of the verify
/// stage ([`FUZZ_SCAFFOLD`]). A FuzzTest build installs its ANTLR runtime into `verify_env/dist`,
/// outside its build directory. Of a fuzz campaign, git records only the `reproducers/`. A
/// workspace without the scaffold does not get these entries: its `.gitignore` must not tell the
/// agent about a mechanism that the run does not use.
const FUZZ_GITIGNORE_ENTRIES: [&str; 6] = [
    "verify_env/dist/",
    "verify_env/clang-rt/",
    "verify_env/fuzz-artifacts/*/corpus/",
    "verify_env/fuzz-artifacts/*/cwd/",
    "verify_env/fuzz-artifacts/*/run.log",
    "verify_env/fuzz-artifacts/*/exit_status",
];

/// The file of the workspace that the FuzzTest scaffold always has.
const FUZZ_SCAFFOLD: &str = "verify_env/run_fuzz.sh";

/// The `.gitignore` entries that the workspace of a run with `profile` needs.
fn gitignore_entries(work_dir: &Path, profile: Profile) -> Vec<&'static str> {
    let fuzz = work_dir.join(FUZZ_SCAFFOLD).exists();
    GITIGNORE_ENTRIES
        .into_iter()
        .filter(|entry| profile == Profile::Harvest || *entry != WORKTREES_ENTRY)
        .chain(FUZZ_GITIGNORE_ENTRIES.into_iter().filter(|_| fuzz))
        .collect()
}

/// Adds the framework's `.gitignore` entries that an existing repository lacks: snapshots made
/// before an entry was introduced (`scratch/`) would otherwise commit what it should hide.
fn complete_gitignore(work_dir: &Path, profile: Profile) -> std::io::Result<()> {
    let path = work_dir.join(".gitignore");
    let mut text = fs::read_to_string(&path).unwrap_or_default();
    let missing: Vec<&str> = gitignore_entries(work_dir, profile)
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
/// keeps its history. `profile` selects the `.gitignore` entries.
pub fn ensure_workdir_git(work_dir: &Path, profile: Profile) {
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
        if let Err(e) = complete_gitignore(work_dir, profile) {
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
    let kept_out = match profile {
        Profile::Harvest => "build outputs, scratch and worktrees",
        _ => "build outputs and scratch",
    };
    let gitignore = format!(
        "# Framework-written: {kept_out} stay untracked.\n{}\n",
        gitignore_entries(work_dir, profile).join("\n")
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

    /// The list has each name once with its final value: a value of the run config replaces a
    /// default of the framework, and an override of the backend replaces a value of the run
    /// config.
    #[test]
    #[cfg_attr(miri, ignore)]
    fn environment_is_the_final_value_of_each_variable() {
        let case = CaseDir::new().expect("case");
        let toolchain = RustToolchainContext::for_version("1.99.0");
        let env: HashMap<String, String> = [
            ("TMPDIR", "/elsewhere"),
            ("B_API_KEY", "secret"),
            ("ANTHROPIC_BASE_URL", "https://example.invalid"),
            ("A", "1"),
        ]
        .into_iter()
        .map(|(name, value)| (name.to_owned(), value.to_owned()))
        .collect();
        let invocation = AgentInvocation {
            phase: AgentPhase::Translate,
            agent: AgentKind::Claude,
            case: &case,
            arm: Arm::Harvest {
                additional_prompts: &[],
                stage_facts: &[],
            },
            timeout_secs: 1,
            // A CCR model: the backend sends the run to its proxy.
            model: Some("openrouter,a/b"),
            subagent_model: Some("openrouter,c/d"),
            teamclaude: false,
            extra_env: &env,
            output_log_path: None,
            prompt_record_dir: None,
            rust_toolchain: &toolchain,
        };
        let vars = invocation
            .environment(Some(Path::new("/shell")), &RegistryLimits::default())
            .expect("environment");
        let tmp = case.tmp().display().to_string();
        let workspace = case.workspace().display().to_string();
        let expected = [
            ("PWD", &*workspace, false),
            ("RUSTUP_TOOLCHAIN", "1.99.0", false),
            ("CLAUDE_CODE_TMPDIR", &*tmp, false),
            ("CLAUDE_CODE_SUBAGENT_MODEL", "openrouter,c/d", false),
            ("CLAUDE_CODE_SUBAGENT_MODEL_FORCE", "1", false),
            ("CLAUDE_CODE_SHELL_PREFIX", "/shell -c", false),
            ("A", "1", true),
            ("B_API_KEY", "secret", true),
            ("TMPDIR", "/elsewhere", true),
            ("ANTHROPIC_BASE_URL", "http://127.0.0.1:3456", false),
        ]
        .map(|(name, value, from_config)| EnvVar {
            name: name.to_owned(),
            value: value.to_owned(),
            from_config,
        });
        assert_eq!(vars, expected);
        case.finish();
    }

    /// The stage around `agent`, a stand-in for the agent.
    fn run_stage(
        invocation: &AgentInvocation<'_>,
        routes: &[opencode::Route<'_>],
        shell: Option<&Path>,
        earlier: &BTreeMap<String, String>,
        agent: impl FnOnce(&str, &Path) -> Result<ExitStatus, Box<dyn std::error::Error>>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let started = start_stage(invocation, routes, shell, earlier, &[])?;
        end_stage(invocation, &started, agent)
    }

    /// The stage of a baseline around a stand-in for the agent: the hooks run immediately
    /// before and after it, the record has what the first hook wrote, and the errors of a hook
    /// and of the record fail the stage.
    #[test]
    #[cfg_attr(miri, ignore)]
    fn baseline_hooks_run_around_the_agent() {
        use std::os::unix::process::ExitStatusExt;

        fn log(context: &HookContext<'_>, line: &str) -> std::io::Result<()> {
            let record = context.record_dir.expect("the test keeps a record");
            let mut file = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(record.join("order.log"))?;
            writeln!(file, "{line}")
        }
        fn before(context: &HookContext<'_>) -> Result<(), Box<dyn std::error::Error>> {
            // The workspace is ready, and the record of this run has no list yet.
            assert!(context.case.workspace().join(".git").is_dir());
            assert!(!context.record_dir.unwrap().join("given.json").exists());
            fs::write(context.case.workspace().join("from_hook.txt"), "hook\n")?;
            Ok(log(context, "before")?)
        }
        fn after(context: &HookContext<'_>) -> Result<(), Box<dyn std::error::Error>> {
            Ok(log(context, "after")?)
        }
        fn fail(_: &HookContext<'_>) -> Result<(), Box<dyn std::error::Error>> {
            Err("the hook failed".into())
        }

        // `agent`: the exit code of the stand-in, or `None` when the agent cannot start.
        let stage = |before_agent: Hook, after_agent: Hook, agent: Option<i32>| {
            let case = CaseDir::new().expect("case");
            let out = tempfile::tempdir().expect("tempdir");
            let record = out.path().join("prompts/translate");
            // A record of an earlier run.
            fs::create_dir_all(&record).expect("mkdir");
            fs::write(record.join("order.log"), "stale\n").expect("write");
            let baseline = Baseline {
                profile: Profile::Vanilla,
                prompt: "the task\n".to_owned(),
                prompt_files: Vec::new(),
                skills: Vec::new(),
                workspace_files: Vec::new(),
                before_agent,
                after_agent,
            };
            let toolchain = RustToolchainContext::for_version("1.99.0");
            let env = HashMap::new();
            let prepared = prepare_agent(AgentInvocation {
                phase: AgentPhase::Translate,
                agent: AgentKind::Claude,
                case: &case,
                arm: Arm::Baseline(&baseline),
                timeout_secs: 1,
                model: None,
                subagent_model: None,
                teamclaude: false,
                extra_env: &env,
                output_log_path: None,
                prompt_record_dir: Some(&record),
                rust_toolchain: &toolchain,
            })
            .expect("prepare");
            let _guard = case.lock_inputs().expect("lock");
            let result = run_stage(
                &prepared.invocation,
                &[],
                None,
                &prepared.earlier,
                |prompt, _log| {
                    assert_eq!(prompt, "the task\n");
                    // The record is complete when the agent starts, with the file of the hook.
                    let list = fs::read_to_string(record.join("given.json"))?;
                    assert!(
                        list.contains("\"path\": \"workspace/from_hook.txt\""),
                        "{list}"
                    );
                    assert_eq!(
                        fs::read_to_string(record.join("files/workspace/from_hook.txt"))?,
                        "hook\n"
                    );
                    fs::OpenOptions::new()
                        .append(true)
                        .open(record.join("order.log"))?
                        .write_all(b"agent\n")?;
                    match agent {
                        Some(exit_code) => Ok(ExitStatus::from_raw(exit_code << 8)),
                        None => Err("the agent cannot start".into()),
                    }
                },
            );
            let order = fs::read_to_string(record.join("order.log")).unwrap_or_default();
            drop(prepared);
            case.finish();
            (result.map_err(|e| e.to_string()), order)
        };
        let all = "before\nagent\nafter\n".to_owned();
        assert_eq!(stage(before, after, Some(0)), (Ok(()), all.clone()));
        // The hook after the agent runs also when the agent failed, and when it could not start.
        assert_eq!(stage(before, after, Some(1)), (Ok(()), all.clone()));
        assert_eq!(
            stage(before, after, None),
            (Err("the agent cannot start".to_owned()), all.clone())
        );
        let failed = Err("the hook failed".to_owned());
        assert_eq!(stage(fail, after, Some(0)), (failed.clone(), String::new()));
        assert_eq!(
            stage(before, fail, Some(0)),
            (failed, "before\nagent\n".to_owned())
        );
        // The first error is the result when both the agent and the hook fail.
        assert_eq!(
            stage(before, fail, None).0,
            Err("the agent cannot start".to_owned())
        );
    }

    /// A baseline stage does not run without its record. A HARVEST stage only warns.
    #[test]
    #[cfg_attr(miri, ignore)]
    fn a_baseline_stage_fails_when_its_record_cannot_be_kept() {
        use std::os::unix::process::ExitStatusExt;
        for (baseline, expected) in [(true, false), (false, true)] {
            let case = CaseDir::new().expect("case");
            let out = tempfile::tempdir().expect("tempdir");
            // A file where the record directory must be.
            fs::write(out.path().join("prompts"), "").expect("write");
            let record = out.path().join("prompts/translate");
            let arm = Baseline {
                profile: Profile::Vanilla,
                prompt: String::new(),
                prompt_files: Vec::new(),
                skills: Vec::new(),
                workspace_files: Vec::new(),
                before_agent: no_hook,
                after_agent: no_hook,
            };
            let toolchain = RustToolchainContext::for_version("1.99.0");
            let env = HashMap::new();
            let prepared = prepare_agent(AgentInvocation {
                phase: AgentPhase::Translate,
                agent: AgentKind::Claude,
                case: &case,
                arm: if baseline {
                    Arm::Baseline(&arm)
                } else {
                    Arm::Harvest {
                        additional_prompts: &[],
                        stage_facts: &[],
                    }
                },
                timeout_secs: 1,
                model: None,
                subagent_model: None,
                teamclaude: false,
                extra_env: &env,
                output_log_path: None,
                prompt_record_dir: Some(&record),
                rust_toolchain: &toolchain,
            })
            .expect("prepare");
            let _guard = case.lock_inputs().expect("lock");
            let mut ran = false;
            let result = run_stage(
                &prepared.invocation,
                &[],
                None,
                &prepared.earlier,
                |_, _| {
                    ran = true;
                    Ok(ExitStatus::from_raw(0))
                },
            );
            assert_eq!(ran, expected);
            assert_eq!(result.is_ok(), expected);
            if let Err(e) = result {
                let message = e.to_string();
                assert!(message.starts_with("cannot keep the record of what the agent is given"));
            }
            drop(prepared);
            case.finish();
        }
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn a_baseline_takes_no_subagent_model() {
        let case = CaseDir::new().expect("case");
        let baseline = Baseline {
            profile: Profile::Actor,
            prompt: String::new(),
            prompt_files: Vec::new(),
            skills: Vec::new(),
            workspace_files: Vec::new(),
            before_agent: no_hook,
            after_agent: no_hook,
        };
        let toolchain = RustToolchainContext::for_version("1.99.0");
        let env = HashMap::new();
        let err = prepare_agent(AgentInvocation {
            phase: AgentPhase::Verify,
            agent: AgentKind::Claude,
            case: &case,
            arm: Arm::Baseline(&baseline),
            timeout_secs: 1,
            model: Some("sonnet"),
            subagent_model: Some("haiku"),
            teamclaude: false,
            extra_env: &env,
            output_log_path: None,
            prompt_record_dir: None,
            rust_toolchain: &toolchain,
        })
        .err()
        .expect("refused")
        .to_string();
        assert_eq!(
            err,
            "subagent_model = haiku is an option of the HARVEST pipeline: the actor baseline \
             does not take it"
        );
        case.finish();
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn baseline_gitignore_names_no_worktrees() {
        let dir = tempfile::tempdir().expect("tempdir");
        ensure_workdir_git(dir.path(), Profile::Vanilla);
        assert_eq!(
            fs::read_to_string(dir.path().join(".gitignore")).expect("read"),
            "# Framework-written: build outputs and scratch stay untracked.\n\
             target/\nbuild*/\ncbuild/\nscratch/\n.opencode/\n"
        );
        // The next stage of the baseline adds nothing.
        ensure_workdir_git(dir.path(), Profile::Vanilla);
        let text = fs::read_to_string(dir.path().join(".gitignore")).expect("read");
        assert!(!text.contains("wt/"), "{text}");
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn resumed_repository_gets_missing_gitignore_entries() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("src.txt"), "content").expect("write file");
        ensure_workdir_git(dir.path(), Profile::Harvest);
        // A snapshot from before `scratch/` was ignored.
        fs::write(dir.path().join(".gitignore"), "# old\ntarget/\nwt/").expect("write gitignore");
        ensure_workdir_git(dir.path(), Profile::Harvest);
        let text = fs::read_to_string(dir.path().join(".gitignore")).expect("read");
        assert_eq!(
            text,
            "# old\ntarget/\nwt/\nbuild*/\ncbuild/\nscratch/\n.opencode/\n"
        );
        ensure_workdir_git(dir.path(), Profile::Harvest);
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
            ensure_workdir_git(dir.path(), Profile::Harvest);
            if let Some(text) = old_gitignore {
                fs::write(dir.path().join(".gitignore"), text).expect("write gitignore");
                ensure_workdir_git(dir.path(), Profile::Harvest);
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
        ensure_workdir_git(dir.path(), Profile::Harvest);
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
        ensure_workdir_git(dir.path(), Profile::Harvest);
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
