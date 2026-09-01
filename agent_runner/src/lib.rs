use harvest_core::config::AgentKind;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};
use tracing::{info, warn};

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

    /// File the rendered prompt is recorded to, next to this phase's log.
    ///
    /// Per phase, because a single run's stages each send their own prompt and
    /// a resumed run's earlier phase happened in a previous process.
    fn prompt_file_name(self) -> &'static str {
        match self {
            AgentPhase::Translate => "translation-prompt.md",
            AgentPhase::Verify => "verify-prompt.md",
            AgentPhase::Conform => "conform-prompt.md",
        }
    }

    /// File the `--append-system-prompt` text is recorded to, when one is used.
    fn append_system_prompt_file_name(self) -> &'static str {
        match self {
            AgentPhase::Translate => "translation-append-system-prompt.md",
            AgentPhase::Verify => "verify-append-system-prompt.md",
            AgentPhase::Conform => "conform-append-system-prompt.md",
        }
    }

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

    /// Compaction-recovery hint, injected per turn. Claude-only: the OpenCode
    /// backend replaces this with the compaction-recovery plugin (see
    /// `OPENCODE_COMPACTION_PLUGIN`), which fires exactly once per compaction
    /// instead of diluting every turn's system prompt.
    fn append_system_prompt(self) -> &'static str {
        match self {
            AgentPhase::Translate => "After any context compaction, you MUST first read PLAN.md.",
            AgentPhase::Verify => {
                "After any context compaction, you MUST first read PLAN.md and HYPOTHESES.md."
            }
            AgentPhase::Conform => "After any context compaction, you MUST first read CONFORM.md.",
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

pub struct AgentInvocation<'a> {
    pub phase: AgentPhase,
    pub agent: AgentKind,
    pub work_dir: &'a Path,
    pub prompt: &'a str,
    pub timeout_secs: u64,
    pub model: Option<&'a str>,
    pub no_plan: bool,
    pub no_plan_file: bool,
    pub extra_env: &'a HashMap<String, String>,
    pub output_log_path: Option<&'a Path>,
    pub rust_toolchain: Option<&'a str>,
}

impl AgentInvocation<'_> {
    /// Whether the prompt instructs the agent to maintain persistent plan
    /// files (PLAN.md / HYPOTHESES.md). Both `no_plan` (no plan, no sub-agent
    /// push) and `no_plan_file` (sub-agent push kept, plan files never
    /// mentioned) drop that instruction, so the per-turn "read PLAN.md after
    /// compaction" system prompt must not be injected in either mode.
    fn plan_files_enabled(&self) -> bool {
        !self.no_plan && !self.no_plan_file
    }
}

/// Temporary workaround for a Claude Code CLI bug: recent versions default to
/// asynchronous sub-agents, and in headless (`claude -p`) mode the process
/// exits as soon as the main agent ends its turn — killing any sub-agents
/// still running in the background. Remove once the CLI is fixed.
const CLAUDE_ASYNC_SUBAGENT_WARNING: &str = "\
**Claude Code async sub-agent bug** \
Recent Claude Code versions launch sub-agents asynchronously by default. \
In this headless (`claude -p`) session that is fatal: ending your turn \
with an asynchronous sub-agent call ends the entire session \
instead of waiting for the sub-agent to finish.
Therefore, you MUST launch EVERY sub-agent with `run_in_background: false` \
(synchronous). You are still encouraged to launch multiple sub-agents \
in a single turn when parallel execution is beneficial, but make sure \
all of them are synchronous.";

/// Temporary workaround for an OpenCode bug (upstream issue #29363): each
/// model response is capped at 32000 output tokens regardless of the model's
/// `limit.output`, and thinking tokens count against the same cap. A turn
/// that burns the full cap in thinking ends without a tool call, OpenCode
/// treats it as complete, and a sub-agent that ends this way returns an
/// empty result. `run_bash_agent` raises the cap through
/// `OPENCODE_EXPERIMENTAL_OUTPUT_TOKEN_MAX`, but a hard cap remains, so the
/// prompt must also steer the model away from long thinking and monolithic
/// writes. Remove once the upstream cap respects `limit.output`.
const OPENCODE_OUTPUT_CAP_WARNING: &str = "\
**OpenCode output-token cap bug** \
OpenCode caps the output tokens of each model response (upstream issue #29363). \
Thinking tokens count against the same cap. \
If thinking uses the full cap before your first tool call, the turn ends as if it were complete. \
The session then stops silently. \
A sub-agent that stops this way returns an empty result and writes no files.
Therefore: keep thinking short. Do not draft a whole file in thinking. \
Write long files in parts: create the file with one `write` call, \
then append each next part with `edit`. Keep each part under ~300 lines. \
Copy this whole warning into EVERY sub-agent prompt.";

/// Agent-specific temporary bug workarounds, injected into every prompt.
/// Prompt templates carry an `{AGENT_BUG_WORKAROUNDS}` placeholder, and
/// prompt-building tools substitute this text for it. Each entry documents a
/// known upstream bug and the behavior that avoids it. Add new entries here
/// when an agent backend needs a temporary fix, and remove them when the
/// upstream fix ships.
pub fn agent_bug_workarounds(agent: AgentKind) -> &'static str {
    match agent {
        AgentKind::Kiro => "",
        AgentKind::Claude => CLAUDE_ASYNC_SUBAGENT_WARNING,
        AgentKind::OpenCode => OPENCODE_OUTPUT_CAP_WARNING,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RustToolchainContext {
    pub required_version: String,
    pub prompt_block: String,
    /// The Test-Corpus / cando2 checkout the contract was read from, if one
    /// was found. Recorded in the stage manifest so a run resuming from a
    /// snapshot can find it again without the C source's original location.
    pub test_corpus_root: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenCodeModelLimits {
    pub context: u64,
    pub output: Option<u64>,
}

pub fn load_opencode_model_limits(
    model: &str,
) -> Result<OpenCodeModelLimits, Box<dyn std::error::Error>> {
    let (provider, metadata_id) = parse_opencode_model(model)?;
    let provider_output = run_opencode_models(Some(&provider))?;
    if let Some(limits) =
        extract_model_limits_from_output(&provider_output, &provider, &metadata_id)
    {
        info!(
            "Resolved OpenCode model limits from provider listing (provider={provider}, id={metadata_id}): context={}, output={:?}",
            limits.context, limits.output,
        );
        return Ok(limits);
    }

    let all_output = run_opencode_models(None)?;
    if let Some(limits) = extract_model_limits_from_output(&all_output, &provider, &metadata_id) {
        info!(
            "Resolved OpenCode model limits from global listing (provider={provider}, id={metadata_id}): context={}, output={:?}",
            limits.context, limits.output,
        );
        return Ok(limits);
    }

    Err(format!(
        "OpenCode model metadata not found for {model}; run `opencode models --verbose` and verify the model exists with a limit.context field"
    ).into())
}

pub fn render_model_limits_block(limits: &OpenCodeModelLimits) -> String {
    let mut lines = vec![
        "### Registry context limits".to_string(),
        format!("context_limit: {}", limits.context),
    ];
    if let Some(output) = limits.output {
        lines.push(format!("output_limit: {output}"));
    }
    lines.join("\n")
}

/// Detects the Rust toolchain contract for a run.
///
/// The version comes from the Test-Corpus / cando2 checkout, found by walking
/// up from `input_path`. That walk only works while the C source still sits
/// inside the corpus, which stops being true once a run resumes from a
/// self-contained snapshot (whose C source lives under the snapshot's own
/// `.harvest/c_src/`). So the corpus root is detected once, on the run that
/// starts from the bench directory, recorded in the stage manifest, and passed
/// back in as `test_corpus_root` by later runs. The resolved root is reported
/// in [`RustToolchainContext::test_corpus_root`] for the caller to persist.
pub fn detect_rust_toolchain_context(
    input_path: &Path,
    test_corpus_root: Option<&Path>,
) -> Result<RustToolchainContext, Box<dyn std::error::Error>> {
    let repo_root = std::env::current_dir()?;
    let root_toolchain = read_toolchain_channel(&repo_root);
    let test_corpus_root = match test_corpus_root {
        Some(root) if root.is_dir() => Some(root.to_path_buf()),
        Some(root) => {
            warn!(
                "recorded Test-Corpus root {} no longer exists; falling back to detection",
                root.display()
            );
            find_test_corpus_root(input_path, &repo_root)
        }
        None => find_test_corpus_root(input_path, &repo_root),
    };
    let test_corpus_toolchain = test_corpus_root
        .as_ref()
        .and_then(|root| read_toolchain_channel(root));
    let cando2_cargo = test_corpus_root
        .as_ref()
        .map(|root| root.join("tools/cando2/Cargo.toml"));
    let cando2_rust_version = cando2_cargo
        .as_ref()
        .and_then(|path| read_cargo_rust_version(path));

    let required_version = cando2_rust_version
        .clone()
        .or_else(|| test_corpus_toolchain.clone())
        .or_else(|| root_toolchain.clone())
        .ok_or("Unable to determine required Rust toolchain version from rust-toolchain.toml or cando2 Cargo.toml")?;

    check_version_match(
        "HARVEST rust-toolchain.toml",
        root_toolchain.as_deref(),
        &required_version,
    )?;
    check_version_match(
        "Test-Corpus rust-toolchain.toml",
        test_corpus_toolchain.as_deref(),
        &required_version,
    )?;
    check_version_match(
        "cando2 rust-version",
        cando2_rust_version.as_deref(),
        &required_version,
    )?;

    let rustc_version = command_stdout("rustc", &["--version"])?;
    let cargo_version = command_stdout("cargo", &["--version"])?;
    let active_rustc = parse_rustc_semver(&rustc_version).ok_or_else(|| {
        format!(
            "Could not parse rustc version from `{}`",
            rustc_version.trim()
        )
    })?;
    if active_rustc != required_version {
        return Err(format!(
            "Rust toolchain mismatch: required {required_version} from Test-Corpus/cando2 contract, but active rustc is {active_rustc} (`{}`)",
            rustc_version.trim()
        )
        .into());
    }

    let prompt_block = format!(
        "### Rust toolchain contract\n\
         - Required Rust toolchain: `{required_version}`\n\
         - Detected rustc: `{}`\n\
         - Detected cargo: `{}`\n\
         - All self-tests and build checks MUST use this exact toolchain. Run Cargo as `RUSTUP_TOOLCHAIN={required_version} cargo ...` (or verify `rustc --version` reports `{required_version}` first). If a different Rust version is active, stop and report an environment/toolchain problem instead of treating test failures as translation bugs.",
        rustc_version.trim(),
        cargo_version.trim(),
    );

    Ok(RustToolchainContext {
        required_version,
        prompt_block,
        test_corpus_root,
    })
}

/// Locates the Test-Corpus / cando2 checkout for `input_path` without
/// performing the toolchain checks, so a caller can record it (see
/// [`detect_rust_toolchain_context`]) before any agent runs.
pub fn locate_test_corpus(input_path: &Path) -> Option<PathBuf> {
    let repo_root = std::env::current_dir().ok()?;
    find_test_corpus_root(input_path, &repo_root)
}

fn find_test_corpus_root(input_path: &Path, repo_root: &Path) -> Option<PathBuf> {
    let absolute = if input_path.is_absolute() {
        input_path.to_path_buf()
    } else {
        repo_root.join(input_path)
    };
    for ancestor in absolute.ancestors() {
        if ancestor.file_name().and_then(|s| s.to_str()) == Some("Test-Corpus") {
            return Some(ancestor.to_path_buf());
        }
    }
    let candidate = repo_root.join("Test-Corpus");
    candidate.exists().then_some(candidate)
}

fn read_toolchain_channel(dir: &Path) -> Option<String> {
    read_quoted_value(&dir.join("rust-toolchain.toml"), "channel")
        .or_else(|| fs::read_to_string(dir.join("rust-toolchain")).ok())
        .map(|s| s.trim().trim_matches('"').to_string())
        .filter(|s| !s.is_empty())
}

fn read_cargo_rust_version(path: &Path) -> Option<String> {
    read_quoted_value(path, "rust-version")
}

fn read_quoted_value(path: &Path, key: &str) -> Option<String> {
    let content = fs::read_to_string(path).ok()?;
    for line in content.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with(key) {
            continue;
        }
        let (_, value) = trimmed.split_once('=')?;
        return Some(value.trim().trim_matches('"').to_string()).filter(|s| !s.is_empty());
    }
    None
}

fn check_version_match(
    label: &str,
    found: Option<&str>,
    required: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(found) = found
        && found != required
    {
        return Err(format!(
            "Rust toolchain contract mismatch: {label} is {found}, required version is {required}"
        )
        .into());
    }
    Ok(())
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

fn parse_rustc_semver(version: &str) -> Option<String> {
    version.split_whitespace().nth(1).map(|v| v.to_string())
}

fn parse_opencode_model(model: &str) -> Result<(String, String), Box<dyn std::error::Error>> {
    // Split "provider/id" or "provider/id:suffix"
    // The colon suffix (e.g. ":floor") is a provider-specific routing hint
    // that must be passed to OpenCode as-is, but stripped when matching
    // model metadata (limit.context etc.) from the registry.
    match model.split_once('/') {
        Some((provider, raw_id)) if !provider.is_empty() && !raw_id.is_empty() => {
            let metadata_id = raw_id.split_once(':').map(|(id, _)| id).unwrap_or(raw_id);
            Ok((provider.to_string(), metadata_id.to_string()))
        }
        _ => Err(format!("OpenCode model must be in provider/model format, got: {model}").into()),
    }
}

fn run_opencode_models(provider: Option<&str>) -> Result<String, Box<dyn std::error::Error>> {
    let mut cmd = Command::new("opencode");
    cmd.arg("models");
    if let Some(provider) = provider {
        cmd.arg(provider);
    }
    cmd.arg("--verbose");
    let output = cmd.output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "opencode models --verbose failed (status={}): {}",
            output.status,
            stderr.trim()
        )
        .into());
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn extract_model_limits_from_output(
    output: &str,
    expected_provider: &str,
    expected_id: &str,
) -> Option<OpenCodeModelLimits> {
    let mut buf: Vec<String> = Vec::new();
    let mut collecting = false;
    let mut brace_depth: i32 = 0;

    for raw_line in output.lines() {
        let trimmed = raw_line.trim();
        if !collecting {
            if trimmed.starts_with('{') {
                buf.clear();
                buf.push(trimmed.to_string());
                collecting = true;
                brace_depth = trimmed.chars().filter(|&c| c == '{').count() as i32
                    - trimmed.chars().filter(|&c| c == '}').count() as i32;
                if brace_depth <= 0 {
                    // Single-line JSON object
                    collecting = false;
                    if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) {
                        let provider = value
                            .get("providerID")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let id = value.get("id").and_then(|v| v.as_str()).unwrap_or("");
                        if provider == expected_provider && id == expected_id {
                            let Some(limit) = value.get("limit") else {
                                continue;
                            };
                            let Some(context) = limit.get("context").and_then(|v| v.as_u64())
                            else {
                                continue;
                            };
                            let output = limit.get("output").and_then(|v| v.as_u64());
                            return Some(OpenCodeModelLimits { context, output });
                        }
                    }
                }
            }
            continue;
        }

        buf.push(trimmed.to_string());
        brace_depth += trimmed.chars().filter(|&c| c == '{').count() as i32
            - trimmed.chars().filter(|&c| c == '}').count() as i32;
        if brace_depth > 0 {
            continue;
        }

        let joined = buf.join("\n");
        collecting = false;
        buf.clear();

        let value = match serde_json::from_str::<serde_json::Value>(&joined) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let provider = value
            .get("providerID")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let id = value.get("id").and_then(|v| v.as_str()).unwrap_or("");
        if provider == expected_provider && id == expected_id {
            let Some(limit) = value.get("limit") else {
                continue;
            };
            let Some(context) = limit.get("context").and_then(|v| v.as_u64()) else {
                continue;
            };
            let output = limit.get("output").and_then(|v| v.as_u64());
            return Some(OpenCodeModelLimits { context, output });
        }
    }

    None
}

/// Writes the rendered prompt(s) for this phase next to its log.
///
/// Each file is the text AS SENT and nothing else, so it can be diffed directly
/// against a prompt template or against another run's. Claude additionally
/// receives `--append-system-prompt` when plan files are enabled, which is part
/// of what it was told and so is recorded too — in its own file, rather than
/// appended to the main one, so neither stops being verbatim. The other agents
/// pass no such prompt (see `invoke_kiro` / `invoke_opencode`), and its file is
/// then simply absent.
///
/// Best-effort: a run that has already cost hours of agent time must not be
/// killed by a failure to write provenance. A missing file is itself detectable,
/// and the warning says why.
fn record_prompt(invocation: &AgentInvocation<'_>, logs_dir: &Path) {
    let phase = invocation.phase;
    let main = logs_dir.join(phase.prompt_file_name());
    if let Err(e) = fs::write(&main, invocation.prompt) {
        warn!("Failed to record prompt at {}: {e}", main.display());
    }

    // Mirrors the condition in `invoke_claude` / `run_bash_agent`: the appended
    // system prompt is only in effect for Claude, and only with plan files on.
    if invocation.agent == AgentKind::Claude && invocation.plan_files_enabled() {
        let extra = logs_dir.join(phase.append_system_prompt_file_name());
        if let Err(e) = fs::write(&extra, phase.append_system_prompt()) {
            warn!(
                "Failed to record appended system prompt at {}: {e}",
                extra.display()
            );
        }
    }
}

pub fn invoke_agent(invocation: AgentInvocation<'_>) -> Result<(), Box<dyn std::error::Error>> {
    prepare_agent_files(&invocation)?;

    let logs_dir = invocation
        .work_dir
        .parent()
        .unwrap_or(invocation.work_dir)
        .join("logs");
    fs::create_dir_all(&logs_dir)?;
    let log_path = logs_dir.join(invocation.phase.log_file_name());

    // Record the prompt the agent is about to receive, before it runs, so the
    // record exists even if the agent dies. Prompts are compiled into the
    // binary with include_str!, so without this the only way to recover what a
    // result was produced from is to identify the commit the binary was built
    // at — impractical after the fact, and impossible if the build is not
    // identifiable. A prompt ablation is uninterpretable if its treatment
    // cannot be retrieved.
    record_prompt(&invocation, &logs_dir);

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
        AgentKind::Kiro => invoke_kiro(&invocation, &log_path)?,
        AgentKind::Claude => invoke_claude(&invocation, &log_path)?,
        AgentKind::OpenCode => invoke_opencode(&invocation, &log_path)?,
    };

    if !status.success() {
        warn!("{} agent exited with {status}", invocation.phase.label());
        // Mirror the abnormal exit into the shared output log: the agent's
        // own stdout/stderr are already tee'd into the per-agent log, but
        // runner diagnostics otherwise exist only on stderr.
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
        if let Err(e) = export_opencode_sessions(&log_path) {
            warn!("OpenCode session export failed (non-fatal): {e}");
            append_output_log_line(
                invocation.output_log_path,
                &format!("WARNING: OpenCode session export failed (non-fatal): {e}"),
            );
        }
    }

    append_trace_if_requested(&log_path, invocation.output_log_path)?;
    append_output_log_line(
        invocation.output_log_path,
        &format!(
            "Appended agent trace ({} phase done)",
            invocation.phase.label()
        ),
    );
    Ok(())
}

/// Appends one ISO-timestamped runner marker line to the shared output log,
/// formatted like a tracing log line so `parse_trace.py` picks it up with the
/// same regexes it already uses on stderr-captured traces. Best-effort: a
/// missing/unwritable output log is not an error.
fn append_output_log_line(output_log_path: Option<&Path>, msg: &str) {
    let Some(out_path) = output_log_path else {
        return;
    };
    use std::io::Write;
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

fn prepare_agent_files(invocation: &AgentInvocation<'_>) -> Result<(), Box<dyn std::error::Error>> {
    match invocation.agent {
        AgentKind::Kiro => Ok(()),
        AgentKind::Claude => {
            let case_dir = invocation.work_dir.parent().unwrap_or(invocation.work_dir);
            write_claude_sandbox(case_dir)?;
            Ok(())
        }
        AgentKind::OpenCode => write_opencode_agent(
            invocation.work_dir,
            OpenCodeAgentConfig {
                name: invocation.phase.opencode_agent_name(),
                description: invocation.phase.opencode_description(),
                // The recovery files (PLAN.md/HYPOTHESES.md/CONFORM.md) do
                // not exist when plan files are disabled.
                recovery_command: invocation
                    .plan_files_enabled()
                    .then(|| format!("cat {}", invocation.phase.recovery_files().join(" "))),
            },
            invocation.model,
        ),
    }
}

fn invoke_kiro(
    invocation: &AgentInvocation<'_>,
    log_path: &Path,
) -> Result<ExitStatus, Box<dyn std::error::Error>> {
    info!(
        "Invoking Kiro {} agent (timeout={}s, extra_env={} vars)",
        invocation.phase.label(),
        invocation.timeout_secs,
        invocation.extra_env.len()
    );
    run_bash_agent(
        invocation,
        log_path,
        format!(
            "set -o pipefail; timeout {} kiro-cli chat \
             --no-interactive --trust-all-tools \"$PROMPT\" < /dev/null 2>&1 | tee \"$LOG\"",
            invocation.timeout_secs
        ),
        None,
    )
}

fn invoke_claude(
    invocation: &AgentInvocation<'_>,
    log_path: &Path,
) -> Result<ExitStatus, Box<dyn std::error::Error>> {
    let use_ccr = claude_uses_ccr(invocation.model);
    info!(
        "Invoking Claude Code {} agent (model={}, no_plan={}, no_plan_file={}, timeout={}s, ccr={}, extra_env={} vars)",
        invocation.phase.label(),
        invocation.model.unwrap_or("(cli default)"),
        invocation.no_plan,
        invocation.no_plan_file,
        invocation.timeout_secs,
        use_ccr,
        invocation.extra_env.len()
    );

    let model_flag = invocation
        .model
        .map(|_| "--model \"$MODEL\" ")
        .unwrap_or_default();
    let append_sys_flag = if invocation.plan_files_enabled() {
        "--append-system-prompt \"$APPEND_SYS\" "
    } else {
        ""
    };

    let status = run_bash_agent(
        invocation,
        log_path,
        format!(
            "set -o pipefail; timeout {} claude -p \"$PROMPT\" \
             {model_flag}\
             --allowed-tools 'Bash(*)' 'Write' 'Edit' \
             --dangerously-skip-permissions \
             {append_sys_flag}\
             --max-turns 1000 \
             --output-format stream-json --verbose \
             < /dev/null 2>&1 | tee \"$LOG\"",
            invocation.timeout_secs
        ),
        Some(invocation.phase.append_system_prompt()),
    )?;

    Ok(status)
}

fn invoke_opencode(
    invocation: &AgentInvocation<'_>,
    log_path: &Path,
) -> Result<ExitStatus, Box<dyn std::error::Error>> {
    info!(
        "Invoking OpenCode {} agent (model={}, timeout={}s, extra_env={} vars)",
        invocation.phase.label(),
        invocation.model.unwrap_or("(cli default)"),
        invocation.timeout_secs,
        invocation.extra_env.len()
    );

    let model_flag = invocation
        .model
        .map(|_| "--model \"$MODEL\" ")
        .unwrap_or_default();
    // No `--pure`: its single effect (verified in the opencode source) is to
    // clear the external-plugin list, which would also disable the
    // project-local compaction-recovery plugin written by
    // `write_opencode_agent`. Isolation from the user-global config is
    // instead achieved by pointing XDG_CONFIG_HOME at a run-private empty
    // directory in `run_bash_agent` — stronger than `--pure` (hides global
    // plugins AND global config/instructions), while project-local
    // `.opencode/` still loads.
    let mut status = run_bash_agent(
        invocation,
        log_path,
        format!(
            "set -o pipefail; timeout {} opencode run \
             --format json \
             --thinking \
             --dangerously-skip-permissions \
             --agent {} \
             {model_flag}\
             \"$PROMPT\" \
             < /dev/null 2>&1 | tee \"$LOG\"",
            invocation.timeout_secs,
            invocation.phase.opencode_agent_name()
        ),
        None,
    )?;

    // OpenCode ends the process with success when a provider stream dies
    // mid-response: it treats the truncated turn as a finished one. Half a
    // translation is then graded as if the agent had chosen to stop.
    // Resume the session so the run continues from the work it already did
    // instead of being scored on it.
    let mut resumes = 0;
    loop {
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
                resumes += 1;
                warn!(
                    "OpenCode {} session {session_id} ended abnormally (last step reason: {reason}); \
                     resuming (attempt {resumes}/{OPENCODE_MAX_RESUMES})",
                    invocation.phase.label()
                );
                append_output_log_line(
                    invocation.output_log_path,
                    &format!(
                        "WARNING: {} session {session_id} ended abnormally (reason: {reason}); \
                         resuming (attempt {resumes}/{OPENCODE_MAX_RESUMES})",
                        invocation.phase.label()
                    ),
                );
                // `tee -a`: the resumed run must extend the log, not replace
                // it — the first segment holds everything the agent did before
                // the stream died, which trace analysis still needs.
                status = run_bash_agent(
                    invocation,
                    log_path,
                    format!(
                        "set -o pipefail; timeout {} opencode run \
                         --format json \
                         --thinking \
                         --dangerously-skip-permissions \
                         --agent {} \
                         --session {} \
                         {model_flag}\
                         \"$RESUME_PROMPT\" \
                         < /dev/null 2>&1 | tee -a \"$LOG\"",
                        invocation.timeout_secs,
                        invocation.phase.opencode_agent_name(),
                        session_id,
                    ),
                    None,
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
Your previous response was cut off by a provider failure, not by you finishing. \
Nothing after that point ran. \
Re-read your plan file to see where you were. \
Check the filesystem to see which files actually exist before you trust any \
earlier claim that one was written. \
Then continue the work from the first unfinished step.";

fn run_bash_agent(
    invocation: &AgentInvocation<'_>,
    log_path: &Path,
    script: String,
    append_system_prompt: Option<&str>,
) -> Result<ExitStatus, Box<dyn std::error::Error>> {
    let openssl_dir = std::env::var("OPENSSL_DIR").unwrap_or_else(|_| "/usr".into());
    let mut cmd = Command::new("bash");
    cmd.arg("-c")
        .arg(script)
        .env("PROMPT", invocation.prompt)
        .env("RESUME_PROMPT", OPENCODE_RESUME_PROMPT)
        .env("LOG", log_path)
        .env("OPENSSL_DIR", openssl_dir)
        .current_dir(invocation.work_dir);

    if let Some(system_prompt) = append_system_prompt
        && invocation.plan_files_enabled()
    {
        cmd.env("APPEND_SYS", system_prompt);
    }

    if let Some(model) = invocation.model {
        cmd.env("MODEL", model);
    }

    if let Some(toolchain) = invocation.rust_toolchain {
        info!("Injecting RUSTUP_TOOLCHAIN={toolchain}");
        cmd.env("RUSTUP_TOOLCHAIN", toolchain);
    }

    if invocation.agent == AgentKind::OpenCode {
        // Hide the user-global OpenCode config from the run. OpenCode
        // resolves its global config dir via xdg-basedir
        // (XDG_CONFIG_HOME/opencode), so a run-private empty directory makes
        // global plugins, global opencode.json{,c}, and global AGENTS.md
        // instructions unreachable — the isolation `--pure` used to provide,
        // without disabling the project-local compaction-recovery plugin.
        // Auth (XDG data dir) and the models cache (XDG cache dir) are
        // unaffected.
        let xdg_config = run_tempdir(invocation.work_dir).join("xdg-config");
        info!(
            "Injecting XDG_CONFIG_HOME={} (run-private, replaces --pure isolation)",
            xdg_config.display()
        );
        cmd.env("XDG_CONFIG_HOME", &xdg_config);
    }

    // Temporary workaround for an OpenCode bug (upstream issue #29363): each
    // model response is capped at min(limit.output, 32000) output tokens.
    // Raise the cap to the registry output limit through the experimental
    // escape hatch. `extra_env` is applied after this, so a value from the
    // run config wins. Remove once the upstream cap respects `limit.output`.
    if invocation.agent == AgentKind::OpenCode
        && let Some(model) = invocation.model
    {
        match load_opencode_model_limits(model) {
            Ok(OpenCodeModelLimits {
                output: Some(output),
                ..
            }) => {
                info!(
                    "Injecting OPENCODE_EXPERIMENTAL_OUTPUT_TOKEN_MAX={output} (opencode#29363 32k output-cap workaround)"
                );
                cmd.env("OPENCODE_EXPERIMENTAL_OUTPUT_TOKEN_MAX", output.to_string());
            }
            Ok(_) => warn!(
                "OpenCode model {model} has no registry output limit; the 32k output cap stays (opencode#29363)"
            ),
            Err(e) => warn!(
                "Could not resolve OpenCode model limits; the 32k output cap stays (opencode#29363): {e}"
            ),
        }
    }

    for (key, value) in invocation.extra_env {
        info!("Injecting env var: {key}");
        cmd.env(key, value);
    }

    if invocation.agent == AgentKind::Claude && claude_uses_ccr(invocation.model) {
        cmd.env("ANTHROPIC_BASE_URL", "http://127.0.0.1:3456");
    }

    Ok(cmd.status()?)
}

fn claude_uses_ccr(model: Option<&str>) -> bool {
    model.is_some_and(|m| m.contains(','))
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
        // The id is interpolated into the resume shell command, so accept only
        // the shape OpenCode actually emits (`ses_` + base62). A surprising id
        // means the log is not what this parser thinks it is. Skip the resume
        // rather than build a command out of it.
        Some(session_id)
            if !session_id.is_empty()
                && session_id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') =>
        {
            OpenCodeOutcome::Resumable {
                session_id,
                reason: last_finish_reason.unwrap_or_else(|| "missing".to_string()),
            }
        }
        _ => OpenCodeOutcome::Healthy,
    }
}

/// Extract all unique session IDs from an OpenCode JSONL log file.
fn extract_session_ids_from_log(log_path: &Path) -> Vec<String> {
    let Ok(file) = fs::File::open(log_path) else {
        return Vec::new();
    };
    let reader = BufReader::new(file);
    let mut seen = HashSet::new();
    let mut ids = Vec::new();
    for line in reader.lines().map_while(Result::ok) {
        let trimmed = line.trim();
        if !trimmed.starts_with('{') {
            continue;
        }
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(trimmed)
            && let Some(sid) = val.get("sessionID").and_then(|v| v.as_str())
            && seen.insert(sid.to_string())
        {
            ids.push(sid.to_string());
        }
    }
    ids
}

/// Recursively extract sub-agent session IDs from an OpenCode export JSON.
/// Looks for `task` tool_use entries whose `metadata.sessionID` points to a child session.
fn extract_sub_session_ids_from_export(export_json: &serde_json::Value) -> Vec<String> {
    let mut ids = Vec::new();
    let Some(messages) = export_json.get("messages").and_then(|v| v.as_array()) else {
        return ids;
    };
    for msg in messages {
        let Some(parts) = msg.get("parts").and_then(|v| v.as_array()) else {
            continue;
        };
        for part in parts {
            let Some(tool_name) = part.get("tool").and_then(|v| v.as_str()) else {
                continue;
            };
            if tool_name != "task" {
                continue;
            }
            if let Some(sid) = part
                .pointer("/state/metadata/sessionID")
                .or_else(|| part.pointer("/state/metadata/sessionId"))
                .or_else(|| part.pointer("/state/metadata/session_id"))
                .and_then(|v| v.as_str())
                && !sid.is_empty()
            {
                ids.push(sid.to_string());
            }
        }
    }
    ids
}

/// Export an OpenCode session by ID, returning the raw JSON string.
///
/// Known bug (opencode#14948): `opencode export` truncates JSON when stdout is
/// piped, but works correctly when redirected to a file.  We work around this
/// by redirecting stdout to a temp file instead of capturing it via pipe.
fn export_opencode_session(session_id: &str) -> Result<String, Box<dyn std::error::Error>> {
    let tmp = tempfile::NamedTempFile::new()?;
    let status = Command::new("opencode")
        .args(["export", session_id])
        .stdout(std::fs::File::create(tmp.path())?)
        .status()?;
    if !status.success() {
        return Err(format!("opencode export {session_id} failed (exit {status})").into());
    }
    let stdout = std::fs::read_to_string(tmp.path())?;
    let json_start = stdout.find('{').ok_or("opencode export produced no JSON")?;
    let raw = &stdout[json_start..];

    // Validate the JSON. If it is still broken (e.g. literal control chars
    // inside strings), return an error so the caller can fall back to JSONL.
    serde_json::from_str::<serde_json::Value>(raw)
        .map_err(|e| format!("opencode export {session_id} produced invalid JSON: {e}"))?;

    Ok(raw.to_string())
}
/// sub-agent sessions, appending each export block to `log_path` only.
/// The shared output log receives exactly one copy of everything (exports
/// included) via `append_trace_if_requested`, which copies the whole
/// per-agent log afterwards — appending here too used to double every
/// export block in output.log.
fn export_opencode_sessions(log_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let session_ids = extract_session_ids_from_log(log_path);
    if session_ids.is_empty() {
        info!("No session IDs found in OpenCode log; skipping export");
        return Ok(());
    }

    let mut exported: HashSet<String> = HashSet::new();
    let mut queue: Vec<String> = session_ids;

    while let Some(sid) = queue.pop() {
        if !exported.insert(sid.clone()) {
            continue;
        }
        info!("Exporting OpenCode session {sid}");
        match export_opencode_session(&sid) {
            Ok(json) => {
                let marker = format!("## opencode-export: {sid}\n");
                let block = format!("{marker}{json}\n");

                // Append to the per-agent log file. (Not to the shared output
                // log: append_trace_if_requested copies this whole file there
                // afterwards — a direct append here would duplicate it.)
                fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(log_path)
                    .and_then(|mut f| {
                        use std::io::Write;
                        f.write_all(block.as_bytes())
                    })?;

                // Write the export block to stderr (same stream as tracing log
                // messages) under a lock so the entire multi-line JSON object is
                // emitted atomically — no log lines can interleave mid-block.
                {
                    use std::io::Write;
                    let stderr = std::io::stderr();
                    let mut handle = stderr.lock();
                    let _ = handle.write_all(block.as_bytes());
                }

                // Discover sub-agent sessions from this export and enqueue them.
                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&json) {
                    for child_sid in extract_sub_session_ids_from_export(&parsed) {
                        if !exported.contains(&child_sid) {
                            queue.push(child_sid);
                        }
                    }
                }
            }
            Err(e) => {
                warn!("Failed to export OpenCode session {sid}: {e}");
            }
        }
    }
    Ok(())
}

fn append_trace_if_requested(
    log_path: &Path,
    output_log_path: Option<&Path>,
) -> Result<(), Box<dyn std::error::Error>> {
    let Some(out_path) = output_log_path else {
        return Ok(());
    };
    if !log_path.exists() {
        return Ok(());
    }

    match fs::read_to_string(log_path) {
        Ok(trace) => {
            use std::io::Write;
            if let Ok(mut file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(out_path)
            {
                let _ = writeln!(file, "\n{}", trace);
                info!("Appended agent trace to {}", out_path.display());
            }
        }
        Err(e) => warn!(
            "Failed to read agent trace from {}: {}",
            log_path.display(),
            e
        ),
    }

    Ok(())
}

fn write_claude_sandbox(case_dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let claude_dir = case_dir.join(".claude");
    fs::create_dir_all(&claude_dir)?;
    fs::write(
        claude_dir.join("settings.json"),
        serde_json::json!({
            "sandbox": {
                "enabled": true,
                "allowUnsandboxedCommands": false,
                "filesystem": {
                    "allowRead": [case_dir.to_string_lossy()],
                    "allowWrite": [case_dir.to_string_lossy()]
                }
            }
        })
        .to_string(),
    )?;
    Ok(())
}

struct OpenCodeAgentConfig<'a> {
    name: &'a str,
    description: &'a str,
    /// `cat <files>` command for post-compaction memory recovery. None when
    /// plan files are disabled (nothing to recover). Some → the
    /// compaction-recovery plugin is written next to the agent definition.
    recovery_command: Option<String>,
}

/// Project-local OpenCode plugin (auto-loaded from `.opencode/plugin/`) that
/// makes post-compaction plan-file recovery reliable. A per-turn system-prompt
/// hint ("after any compaction, read PLAN.md first") is both wasteful and
/// weakly followed (observed in trace_zstd_44: the instruction was present in
/// the system prompt AND in the summary text, and was still skipped). See the
/// plugin source for the mechanism. `{RECOVERY_CMD}` is substituted with the
/// phase's recovery command before writing.
const OPENCODE_COMPACTION_PLUGIN: &str = include_str!("opencode_compaction_recovery.js");

/// Project-local OpenCode plugin that rewrites an empty sub-agent `task`
/// result into an explicit failure report. Written for every OpenCode run
/// (unlike the compaction plugin, this needs no per-phase substitution). See
/// the plugin source for the mechanism and the upstream issues.
const OPENCODE_RESILIENCE_PLUGIN: &str = include_str!("opencode_resilience.js");

const OPENCODE_LOCAL_PERMISSIONS: &[(&str, &str)] = &[
    ("bash", "allow"),
    ("read", "allow"),
    ("edit", "allow"),
    ("write", "allow"),
    ("glob", "allow"),
    ("grep", "allow"),
    ("task", "allow"),
    ("todowrite", "allow"),
    ("lsp", "allow"),
    ("webfetch", "deny"),
    ("websearch", "deny"),
    ("skill", "deny"),
];

/// The run's private temp directory: the parent of `work_dir`
/// (`/tmp/.tmpXXXX` for `/tmp/.tmpXXXX/translated_rust`).
fn run_tempdir(work_dir: &Path) -> &Path {
    work_dir.parent().unwrap_or(work_dir)
}

/// Project-level OpenCode config written alongside the agent definitions.
/// `external_directory` defaults to "ask", and task sub-agent sessions do not
/// inherit `--dangerously-skip-permissions`, so in a headless run any
/// sub-agent tool call that touches a path outside the project directory
/// blocks forever on the unanswerable permission prompt. Scoping external
/// access to this run's temp directory and denying everything else makes "ask"
/// unreachable.
fn opencode_project_config(
    work_dir: &Path,
    model: Option<&str>,
    staged_provider: Option<&(String, serde_json::Value)>,
) -> String {
    let tempdir_pattern = format!("{}/**", run_tempdir(work_dir).display());
    // The pin wins over a staged entry: a user shadowing a built-in provider
    // (e.g. openrouter) keeps today's routing behavior.
    let provider_value = openrouter_provider_pin(model).or_else(|| {
        staged_provider.map(|(id, definition)| {
            let mut providers = serde_json::Map::new();
            providers.insert(id.clone(), definition.clone());
            serde_json::Value::Object(providers)
        })
    });
    let provider_block = match provider_value {
        Some(value) => format!(
            ",\n  \"provider\": {}",
            serde_json::to_string(&value).expect("provider block serializes to JSON"),
        ),
        None => String::new(),
    };
    format!(
        r#"{{
  "$schema": "https://opencode.ai/config.json",
  "permission": {{
    "external_directory": {{
      "*": "deny",
      {}: "allow"
    }}
  }}{}
}}
"#,
        serde_json::to_string(&tempdir_pattern).expect("path pattern serializes to JSON"),
        provider_block,
    )
}

/// OpenRouter multiplexes each model across several upstream endpoints and
/// load-balances across them by default, so `openrouter/xiaomi/mimo-v2.5-pro`
/// may be served by any host — including ones with a smaller context window,
/// higher price, or worse uptime than the model author's own first-party
/// endpoint (a run was once routed to a 262k-context, 5x-priced third party
/// that then dropped the stream). Pin the request to the author's first-party
/// endpoint and disable fallbacks: for a first-party author OpenRouter's
/// provider slug equals the author segment of the model id (verified for
/// `xiaomi` and holds for `deepseek`, `minimax`, etc.), so a run never silently
/// lands on an inferior host. Returns None for non-OpenRouter models (direct
/// providers route to a single upstream and need no hint).
fn openrouter_provider_pin(model: Option<&str>) -> Option<serde_json::Value> {
    let rest = model?.strip_prefix("openrouter/")?;
    // Drop any routing suffix (e.g. ":floor") so the key matches the registry
    // model id, then take the author segment as the first-party provider slug.
    let model_id = rest.split(':').next().unwrap_or(rest);
    let author = model_id.split('/').next().filter(|s| !s.is_empty())?;
    Some(serde_json::json!({
        "openrouter": {
            "models": {
                model_id: {
                    "options": {
                        "provider": { "only": [author], "allow_fallbacks": false }
                    }
                }
            }
        }
    }))
}

/// The selected model's custom-provider definition, for staging into the
/// run's project config.
///
/// `run_bash_agent` points XDG_CONFIG_HOME at an empty directory, which hides
/// the user's global opencode.json{,c} and every custom provider in it.
/// auth.json keys survive (XDG data dir), so without staging the run holds a
/// key to an endpoint it cannot name and fails with opaque provider errors.
/// `opencode debug config` resolves the user's real config in the harness
/// environment. Staging that one entry gives the run its endpoint while
/// global plugins, AGENTS.md instructions, and unrelated config stay hidden.
/// Built-in providers are absent from the user config and stage nothing. A
/// provider defined nowhere fails the run before spawn: after spawn the same
/// mistake surfaces as generic provider stream errors with nothing
/// actionable in them.
fn custom_provider_stage(
    model: Option<&str>,
) -> Result<Option<(String, serde_json::Value)>, String> {
    let Some(model) = model else {
        return Ok(None);
    };
    let Ok((provider_id, _)) = parse_opencode_model(model) else {
        return Ok(None);
    };
    let resolved = match command_stdout("opencode", &["debug", "config"]).and_then(|output| {
        serde_json::from_str::<serde_json::Value>(&output)
            .map_err(|error| -> Box<dyn std::error::Error> { error.into() })
    }) {
        Ok(value) => value,
        Err(error) => {
            warn!("opencode debug config unreadable, staging nothing: {error}");
            return Ok(None);
        }
    };
    if let Some(definition) = resolved.get("provider").and_then(|p| p.get(&provider_id)) {
        return Ok(Some((provider_id, definition.clone())));
    }
    match run_opencode_models(Some(&provider_id)) {
        // A built-in provider resolves without user config.
        Ok(_) => Ok(None),
        Err(_) => Err(format!(
            "OpenCode provider `{provider_id}` is defined nowhere: `opencode models {provider_id}` \
             fails and `opencode debug config` has no entry for it. Define it in \
             ~/.config/opencode/opencode.jsonc, or export OPENCODE_CONFIG to a config file \
             that defines it, and rerun."
        )),
    }
}

const WORKDIR_BOUNDARY_TEMPLATE: &str = r#"### Filesystem boundary
- `{TEMPDIR}` is the ONLY directory you may read or write.
- Keep all scratch files inside the project directory.
- Prefer relative paths. Never retype the absolute temp-directory prefix
  by hand: a single typo in it makes the path "external" and the call is
  denied.
- When you dispatch sub-agents, copy this entire "Filesystem boundary"
  section into every sub-agent prompt verbatim."#;

/// Prompt block telling the agent it may only touch files under this run's
/// temp directory, mirroring the `external_directory` policy that
/// `opencode_project_config` enforces. Rendered for OpenCode only. Other
/// agents get an empty string (substituted for `{WORKDIR_BOUNDARY}`).
pub fn render_workdir_boundary(agent: AgentKind, work_dir: &Path) -> String {
    if agent != AgentKind::OpenCode {
        return String::new();
    }
    WORKDIR_BOUNDARY_TEMPLATE.replace("{TEMPDIR}", &run_tempdir(work_dir).display().to_string())
}

fn write_opencode_agent(
    work_dir: &Path,
    config: OpenCodeAgentConfig<'_>,
    model: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    let agents_dir = work_dir.join(".opencode/agents");
    fs::create_dir_all(&agents_dir)?;

    let staged = custom_provider_stage(model)
        .map_err(|error| -> Box<dyn std::error::Error> { error.into() })?;
    fs::write(
        work_dir.join(".opencode/opencode.json"),
        opencode_project_config(work_dir, model, staged.as_ref()),
    )?;

    let mut permissions = String::new();
    for (tool, policy) in OPENCODE_LOCAL_PERMISSIONS {
        permissions.push_str(&format!("  {tool}: {policy}\n"));
    }

    // The agent .md body becomes `agent.prompt` and would REPLACE OpenCode's
    // default provider system prompt (request assembly is
    // `agent.prompt ? [agent.prompt] : SystemPrompt.provider(model)`, the md
    // body is trimmed, and an empty string is falsy). Historically we put the
    // compaction-recovery hint here, which silently dropped the entire
    // default coding prompt (~2k tokens of tool-use/communication guidance).
    // Deliberately keep the body EMPTY: the official default prompt is a
    // stable prefix (billed once, then cache reads), and the recovery
    // instruction now lives in the compaction plugin instead.
    fs::write(
        agents_dir.join(format!("{}.md", config.name)),
        format!(
            "---\ndescription: {}\nmode: primary\npermission:\n{}---\n",
            config.description, permissions
        ),
    )?;

    let plugin_dir = work_dir.join(".opencode/plugin");
    fs::create_dir_all(&plugin_dir)?;
    fs::write(
        plugin_dir.join("harvest-resilience.js"),
        OPENCODE_RESILIENCE_PLUGIN,
    )?;

    if let Some(recovery_command) = &config.recovery_command {
        fs::write(
            plugin_dir.join("compaction-recovery.js"),
            OPENCODE_COMPACTION_PLUGIN.replace("{RECOVERY_CMD}", recovery_command),
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_ccr_detection_requires_comma_model() {
        assert!(claude_uses_ccr(Some("openrouter,deepseek/deepseek-v4-pro")));
        assert!(!claude_uses_ccr(Some("sonnet")));
        assert!(!claude_uses_ccr(None));
    }

    #[test]
    fn agent_bug_workarounds_are_agent_specific() {
        assert_eq!(agent_bug_workarounds(AgentKind::Kiro), "");
        assert!(agent_bug_workarounds(AgentKind::Claude).contains("run_in_background: false"));
        let opencode = agent_bug_workarounds(AgentKind::OpenCode);
        assert!(opencode.contains("#29363"));
        assert!(opencode.contains("sub-agent prompt"));
    }

    /// An invocation whose only purpose is to exercise prompt recording.
    fn prompt_invocation<'a>(
        agent: AgentKind,
        phase: AgentPhase,
        prompt: &'a str,
        work_dir: &'a Path,
        no_plan: bool,
        env: &'a HashMap<String, String>,
    ) -> AgentInvocation<'a> {
        AgentInvocation {
            phase,
            agent,
            work_dir,
            prompt,
            timeout_secs: 1,
            model: None,
            no_plan,
            no_plan_file: false,
            extra_env: env,
            output_log_path: None,
            rust_toolchain: None,
        }
    }

    #[test]
    fn prompt_is_recorded_verbatim_per_phase() {
        let dir = tempfile::tempdir().expect("tempdir");
        let logs = dir.path().join("logs");
        fs::create_dir_all(&logs).unwrap();
        let env = HashMap::new();
        // Content that would be mangled by any templating or trimming.
        let prompt = "  line one\n\nline two with `backticks` and {BRACES}\n\ttab\n";

        for (phase, expected) in [
            (AgentPhase::Translate, "translation-prompt.md"),
            (AgentPhase::Verify, "verify-prompt.md"),
            (AgentPhase::Conform, "conform-prompt.md"),
        ] {
            let inv = prompt_invocation(AgentKind::Kiro, phase, prompt, dir.path(), false, &env);
            record_prompt(&inv, &logs);
            let got = fs::read_to_string(logs.join(expected)).expect(expected);
            // Byte-identical: the file must be diffable against a template.
            assert_eq!(got, prompt, "{expected}");
        }
    }

    #[test]
    fn appended_system_prompt_is_recorded_only_when_it_is_actually_used() {
        let dir = tempfile::tempdir().expect("tempdir");
        let logs = dir.path().join("logs");
        fs::create_dir_all(&logs).unwrap();
        let env = HashMap::new();
        let extra = logs.join(AgentPhase::Translate.append_system_prompt_file_name());

        // Claude with plan files on: the appended prompt is in effect, so it is
        // part of what the agent was told and must be recoverable.
        let inv = prompt_invocation(
            AgentKind::Claude,
            AgentPhase::Translate,
            "p",
            dir.path(),
            false,
            &env,
        );
        record_prompt(&inv, &logs);
        let got = fs::read_to_string(&extra).expect("claude + plan files records it");
        assert_eq!(got, AgentPhase::Translate.append_system_prompt());
        fs::remove_file(&extra).unwrap();

        // --no-plan: `invoke_claude` omits the flag, so recording it would claim
        // the agent was told something it never saw.
        let no_plan = prompt_invocation(
            AgentKind::Claude,
            AgentPhase::Translate,
            "p",
            dir.path(),
            true,
            &env,
        );
        record_prompt(&no_plan, &logs);
        assert!(
            !extra.exists(),
            "--no-plan must not record an appended prompt"
        );

        // Kiro and OpenCode are passed None for it (see invoke_kiro/invoke_opencode).
        for agent in [AgentKind::Kiro, AgentKind::OpenCode] {
            let inv = prompt_invocation(agent, AgentPhase::Translate, "p", dir.path(), false, &env);
            record_prompt(&inv, &logs);
            assert!(
                !extra.exists(),
                "{agent:?} receives no appended system prompt"
            );
        }
    }

    #[test]
    fn recording_a_prompt_never_fails_a_run() {
        // Provenance is best-effort on purpose: a run that has already cost
        // hours must not die because a file could not be written. The missing
        // file is itself the signal.
        let env = HashMap::new();
        let missing = Path::new("/nonexistent-dir-for-harvest-test/logs");
        let inv = prompt_invocation(
            AgentKind::Claude,
            AgentPhase::Verify,
            "p",
            Path::new("/tmp"),
            false,
            &env,
        );
        record_prompt(&inv, missing); // must not panic
    }

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
    fn a_shell_unsafe_session_id_is_not_resumed() {
        let outcome = assess_log(&[
            r#"{"type":"step_finish","sessionID":"ses_a; rm -rf /","part":{"reason":"unknown"}}"#,
        ]);
        assert_eq!(outcome, OpenCodeOutcome::Healthy);
    }

    #[test]
    fn a_log_without_events_is_left_alone() {
        assert_eq!(assess_log(&["not json", ""]), OpenCodeOutcome::Healthy);
    }

    #[test]
    fn resilience_plugin_rewrites_empty_task_results() {
        assert!(OPENCODE_RESILIENCE_PLUGIN.contains("tool.execute.after"));
        assert!(OPENCODE_RESILIENCE_PLUGIN.contains("task_result"));
    }

    #[test]
    fn opencode_permissions_deny_web_and_skill() {
        assert!(OPENCODE_LOCAL_PERMISSIONS.contains(&("webfetch", "deny")));
        assert!(OPENCODE_LOCAL_PERMISSIONS.contains(&("websearch", "deny")));
        assert!(OPENCODE_LOCAL_PERMISSIONS.contains(&("skill", "deny")));
        assert!(OPENCODE_LOCAL_PERMISSIONS.contains(&("bash", "allow")));
    }

    #[test]
    fn opencode_project_config_scopes_external_directory_to_run_tempdir() {
        let work_dir = Path::new("/tmp/.tmpAbc123/translated_rust");
        let raw = opencode_project_config(work_dir, None, None);
        let config: serde_json::Value =
            serde_json::from_str(&raw).expect("project config must be valid JSON");
        let rules = config
            .pointer("/permission/external_directory")
            .and_then(|v| v.as_object())
            .expect("external_directory rules present");
        assert_eq!(rules.get("*").and_then(|v| v.as_str()), Some("deny"));
        assert_eq!(
            rules.get("/tmp/.tmpAbc123/**").and_then(|v| v.as_str()),
            Some("allow")
        );
        // OpenCode resolves rules last-match-wins: the catch-all deny must
        // appear before the tempdir allow in the serialized config.
        assert!(raw.find("\"*\"").unwrap() < raw.find("/tmp/.tmpAbc123/**").unwrap());
        // No model → no provider routing block.
        assert!(config.get("provider").is_none());
    }

    #[test]
    fn opencode_project_config_pins_openrouter_to_author_endpoint() {
        let work_dir = Path::new("/tmp/.tmpAbc123/translated_rust");
        let raw = opencode_project_config(work_dir, Some("openrouter/xiaomi/mimo-v2.5-pro"), None);
        let config: serde_json::Value =
            serde_json::from_str(&raw).expect("project config must be valid JSON");
        let opts = config
            .pointer("/provider/openrouter/models/xiaomi~1mimo-v2.5-pro/options/provider")
            .expect("openrouter provider pin present");
        assert_eq!(
            opts.pointer("/only").and_then(|v| v.as_array()),
            Some(&vec![serde_json::json!("xiaomi")])
        );
        assert_eq!(
            opts.pointer("/allow_fallbacks").and_then(|v| v.as_bool()),
            Some(false)
        );
    }

    #[test]
    fn opencode_project_config_no_pin_for_non_openrouter() {
        let work_dir = Path::new("/tmp/.tmpAbc123/translated_rust");
        let raw = opencode_project_config(work_dir, Some("opencode-go/mimo-v2.5"), None);
        let config: serde_json::Value =
            serde_json::from_str(&raw).expect("project config must be valid JSON");
        assert!(config.get("provider").is_none());
    }

    #[test]
    fn opencode_project_config_stages_custom_provider() {
        let work_dir = Path::new("/tmp/.tmpAbc123/translated_rust");
        let definition = serde_json::json!({
            "npm": "@ai-sdk/openai-compatible",
            "options": { "baseURL": "https://endpoint.example/v1" }
        });
        let raw = opencode_project_config(
            work_dir,
            Some("harvest-hyak/qwen3.8-27b"),
            Some(&("harvest-hyak".to_string(), definition)),
        );
        let config: serde_json::Value =
            serde_json::from_str(&raw).expect("project config must be valid JSON");
        assert_eq!(
            config.pointer("/provider/harvest-hyak/options/baseURL"),
            Some(&serde_json::json!("https://endpoint.example/v1"))
        );
        // Staging must not disturb the permission boundary.
        assert!(config.pointer("/permission/external_directory/*").is_some());
    }

    #[test]
    fn opencode_project_config_prefers_openrouter_pin_over_staged_entry() {
        let work_dir = Path::new("/tmp/.tmpAbc123/translated_rust");
        let raw = opencode_project_config(
            work_dir,
            Some("openrouter/xiaomi/mimo-v2.5-pro"),
            Some(&(
                "openrouter".to_string(),
                serde_json::json!({"npm": "shadowed"}),
            )),
        );
        let config: serde_json::Value =
            serde_json::from_str(&raw).expect("project config must be valid JSON");
        assert!(config.pointer("/provider/openrouter/models").is_some());
        assert!(config.pointer("/provider/openrouter/npm").is_none());
    }

    #[test]
    fn custom_provider_stage_skips_shellout_without_model() {
        assert!(custom_provider_stage(None).unwrap().is_none());
    }

    #[test]
    fn write_opencode_agent_writes_project_config() {
        let dir = tempfile::tempdir().unwrap();
        write_opencode_agent(
            dir.path(),
            OpenCodeAgentConfig {
                name: "harvest-translate",
                description: "test",
                recovery_command: None,
            },
            None,
        )
        .unwrap();
        let config = fs::read_to_string(dir.path().join(".opencode/opencode.json")).unwrap();
        assert_eq!(config, opencode_project_config(dir.path(), None, None));
        let agent_md =
            fs::read_to_string(dir.path().join(".opencode/agents/harvest-translate.md")).unwrap();
        // The body must be EMPTY so `agent.prompt` stays falsy and OpenCode's
        // official default provider system prompt applies. The compaction
        // hint lives in the plugin, not here.
        assert!(agent_md.ends_with("---\n"));
        assert!(!agent_md.contains("compaction"));
        // No recovery command -> no plugin.
        assert!(
            !dir.path()
                .join(".opencode/plugin/compaction-recovery.js")
                .exists()
        );
    }

    #[test]
    fn write_opencode_agent_writes_compaction_recovery_plugin() {
        let dir = tempfile::tempdir().unwrap();
        write_opencode_agent(
            dir.path(),
            OpenCodeAgentConfig {
                name: "harvest-verify",
                description: "test",
                recovery_command: Some("cat PLAN.md HYPOTHESES.md".to_string()),
            },
            None,
        )
        .unwrap();
        let plugin =
            fs::read_to_string(dir.path().join(".opencode/plugin/compaction-recovery.js")).unwrap();
        assert!(plugin.contains("run `cat PLAN.md HYPOTHESES.md` to restore"));
        assert!(!plugin.contains("{RECOVERY_CMD}"));
        assert!(plugin.contains("experimental.chat.messages.transform"));
        assert!(plugin.contains("experimental.session.compacting"));
        // Fallback predicate must not depend solely on the unstable metadata marker.
        assert!(plugin.contains("Continue if you have next steps"));
    }

    #[test]
    fn workdir_boundary_rendered_for_opencode_only() {
        let work_dir = Path::new("/tmp/.tmpAbc123/translated_rust");
        let block = render_workdir_boundary(AgentKind::OpenCode, work_dir);
        assert!(block.starts_with("### Filesystem boundary"));
        assert!(block.contains("/tmp/.tmpAbc123"));
        assert!(block.contains("sub-agent prompt"));
        assert_eq!(render_workdir_boundary(AgentKind::Claude, work_dir), "");
        assert_eq!(render_workdir_boundary(AgentKind::Kiro, work_dir), "");
    }

    #[test]
    fn extract_model_limits_matches_provider_and_id() {
        let sample = concat!(
            "opencode-go/mimo-v2.5-pro\n",
            "{\n",
            "  \"id\": \"mimo-v2.5-pro\",\n",
            "  \"providerID\": \"opencode-go\",\n",
            "  \"name\": \"MiMo V2.5 Pro\",\n",
            "  \"limit\": {\n",
            "    \"context\": 1048576,\n",
            "    \"output\": 128000\n",
            "  }\n",
            "}\n",
        );
        let limits = extract_model_limits_from_output(sample, "opencode-go", "mimo-v2.5-pro")
            .expect("limits must be found");
        assert_eq!(limits.context, 1_048_576);
        assert_eq!(limits.output, Some(128_000));
    }

    #[test]
    fn extract_model_limits_requires_exact_match() {
        let sample = concat!(
            "opencode-go/mimo-v2.5-pro\n",
            "{\n",
            "  \"id\": \"mimo-v2.5-pro\",\n",
            "  \"providerID\": \"opencode-go\",\n",
            "  \"limit\": {\n",
            "    \"context\": 1048576,\n",
            "    \"output\": 128000\n",
            "  }\n",
            "}\n",
        );
        assert!(extract_model_limits_from_output(sample, "opencode-go", "mimo-v2.5").is_none());
        assert!(
            extract_model_limits_from_output(sample, "other-provider", "mimo-v2.5-pro").is_none()
        );
    }

    #[test]
    fn parse_opencode_model_strips_colon_suffix() {
        let (provider, metadata_id) =
            parse_opencode_model("openrouter/xiaomi/mimo-v2.5-pro:floor").unwrap();
        assert_eq!(provider, "openrouter");
        assert_eq!(metadata_id, "xiaomi/mimo-v2.5-pro");

        let (provider, metadata_id) = parse_opencode_model("opencode-go/deepseek-v4-pro").unwrap();
        assert_eq!(provider, "opencode-go");
        assert_eq!(metadata_id, "deepseek-v4-pro");
    }
}
