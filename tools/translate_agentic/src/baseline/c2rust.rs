//! The c2rust arm: the transpiler c2rust, driven by the agent.
//!
//! The agent gets a short prompt and the skill `c2rust` (`agent_skills/c2rust/`), whose tool
//! runs the transpiler. The framework does three things around the agent:
//! - Before the stage starts, it checks that c2rust [`REQUIRED_VERSION`] is installed.
//! - Before the agent starts, it records what decides the output of the tool: the programs and
//!   libraries that the agent can write, with their hashes, and the versions of the others.
//! - When the agent has ended, it runs the report of the skill (`hand_changes.py`) from its own
//!   copy of the scripts. The report translates again and compares the result with the working
//!   directory, so the record tells what the package has that the transpiler did not write. The
//!   report runs before the tool of the stage changes `Cargo.toml`.
//!
//! The record is `c2rust/` in the prompt record of the stage. No outcome of the report fails the
//! stage: a package that c2rust never wrote, a comparison that could not be made and a changed
//! program are facts of the run, and the summary states them.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use agent_runner::{Baseline, CaseDir, HookContext, hash_file, shared_reading_list};
use harvest_core::config::Profile;
use harvest_core::stage_manifest::arm_summary_path;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tracing::{info, warn};

use super::Program;

/// The main prompt, after the list of the shared files: it names the prompt file of the arm.
const MAIN: &str = include_str!("../../prompts/baseline/c2rust/main.md");
/// The prompt file of the arm, by its path in `inputs/prompts/`.
const TRANSLATE: (&str, &str) = (
    "c2rust/translate.md",
    include_str!("../../prompts/baseline/c2rust/translate.md"),
);
const SKILL: &str = "c2rust";
/// What `c2rust --version` prints for the version of the study.
const REQUIRED_VERSION: &str = "C2Rust 0.22.1";
/// The time limit of the report. It makes one C build and one c2rust call.
const REPORT_SECONDS: u32 = 1800;
/// Where the report finds its programs, besides the rustfmt of the start: the directories of
/// the system. The Cargo home, which the agent can write, is not among them.
const SYSTEM_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";
/// The programs of the system that the tool runs, and how each tells its version. `ldd` tells
/// the version of the C library, whose headers decide what c2rust can read.
const SYSTEM_PROGRAMS: [(&str, &str); 4] = [
    ("cmake", "--version"),
    ("cc", "--version"),
    ("python3", "--version"),
    ("ldd", "--version"),
];
/// The files of a working directory that the tool does not give and that are not changes by
/// hand, with who writes each. The summary names them apart from the other files.
const NOT_HAND_CHANGES: [(&str, &str); 3] = [
    (".gitignore", "the framework writes it"),
    ("Cargo.lock", "Cargo writes it at the first build"),
    (
        "c2rust_record/calls.jsonl",
        "the tool appends each call to it",
    ),
];
/// The file of the record that [`before_agent`] writes.
const START_FILE: &str = "start.json";
/// The c2rust output of the last call of the agent, as the tool left it in the scratch
/// directory: a file of the agent, kept because it tells why a call gave no package.
const LAST_CALL_LOG: (&str, &str) = ("scratch/c2rust/transpile.log", "agent_last_call.log");
const LAST_CALL_LOG_LIMIT: u64 = 20 << 20;

/// What the translate stage gives the agent of the arm. Fails when the c2rust of the study is
/// not installed.
pub(crate) fn stage(_program: &Program) -> Result<Baseline, String> {
    required_c2rust(std::env::var_os("PATH").as_deref())?;
    Ok(baseline())
}

/// The stage of the arm. It does not depend on the host.
fn baseline() -> Baseline {
    let shared = shared_reading_list();
    Baseline {
        profile: Profile::C2rust,
        prompt: format!("{shared}\n{MAIN}"),
        prompt_files: vec![(TRANSLATE.0.to_owned(), TRANSLATE.1.to_owned())],
        skills: vec![SKILL],
        workspace_files: Vec::new(),
        before_agent,
        after_agent,
    }
}

/// The `c2rust` program on `path` (the value of PATH), when it has the version of the study.
fn required_c2rust(path: Option<&OsStr>) -> Result<PathBuf, String> {
    let program = find_program("c2rust", path).ok_or_else(|| {
        format!("the c2rust baseline needs c2rust: no `c2rust` on PATH. {INSTALL}")
    })?;
    let version = output_of(Command::new(&program).arg("--version"))?;
    check_version(&program, &version)?;
    Ok(program)
}

const INSTALL: &str =
    "Install it with `cargo install c2rust --version 0.22.1 --locked`, or put it on PATH.";

/// Refuses a c2rust `program` that prints another `version` than the one of the study.
fn check_version(program: &Path, version: &str) -> Result<(), String> {
    if version == REQUIRED_VERSION {
        return Ok(());
    }
    Err(format!(
        "the c2rust baseline needs {REQUIRED_VERSION}: {} is `{version}`. {INSTALL}",
        program.display()
    ))
}

/// The first file `name` in the directories of `path` (the value of PATH), as an absolute path
/// without symbolic links.
fn find_program(name: &str, path: Option<&OsStr>) -> Option<PathBuf> {
    std::env::split_paths(path?)
        .map(|dir| dir.join(name))
        .find(|file| file.is_file())
        .and_then(|file| file.canonicalize().ok())
}

/// The trimmed standard output of a command that succeeds.
fn output_of(command: &mut Command) -> Result<String, String> {
    let output = command
        .output()
        .map_err(|e| format!("cannot run {command:?}: {e}"))?;
    if !output.status.success() {
        return Err(format!("{command:?} failed with {}", output.status));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// What the run records before the agent starts: what decides the output of the tool.
#[derive(Debug, PartialEq, Deserialize, Serialize)]
struct Start {
    c2rust_version: String,
    /// The Rust toolchain of the run. The tool formats with its rustfmt.
    rust_toolchain: String,
    /// The first line that each program of the system prints for its version
    /// ([`SYSTEM_PROGRAMS`]). The agent cannot write these programs.
    system: BTreeMap<String, String>,
    /// The programs and libraries of the tool with their hashes: `c2rust` first, then
    /// `c2rust-transpile` and the rustfmt of the toolchain, then the libraries that these two
    /// load from outside the system directories, and their LLVM and clang libraries. The agent
    /// can write the Cargo home and the rustup home, where most of them are.
    files: Vec<ToolFile>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
struct ToolFile {
    name: String,
    path: PathBuf,
    sha256: String,
}

impl ToolFile {
    fn of(name: &str, path: PathBuf) -> std::io::Result<Self> {
        let (_, sha256) = hash_file(&path).map_err(|e| {
            let file = path.display();
            std::io::Error::other(format!(
                "cannot read {name} of the c2rust tool at {file}: {e}"
            ))
        })?;
        Ok(ToolFile {
            name: name.to_owned(),
            sha256,
            path,
        })
    }
}

impl Start {
    fn detect(rust_toolchain: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let path = std::env::var_os("PATH");
        let c2rust = required_c2rust(path.as_deref())?;
        let transpile = c2rust.with_file_name("c2rust-transpile");
        // The rustfmt of the toolchain of the run. Without rustup, the one on PATH.
        let of_toolchain = ["which", "rustfmt", "--toolchain", rust_toolchain];
        let rustfmt = output_of(Command::new("rustup").args(of_toolchain))
            .map(PathBuf::from)
            .or_else(|e| find_program("rustfmt", path.as_deref()).ok_or(e))?;
        let mut files = vec![ToolFile::of("c2rust", c2rust)?];
        for (name, program) in [("c2rust-transpile", transpile), ("rustfmt", rustfmt)] {
            let libraries = loaded_libraries(&program);
            files.push(ToolFile::of(name, program)?);
            for library in libraries {
                let library = library.canonicalize()?;
                let name = library.file_name().unwrap_or_default().to_string_lossy();
                files.push(ToolFile::of(&name, library.clone())?);
            }
        }
        let system = SYSTEM_PROGRAMS
            .iter()
            .filter_map(|(program, flag)| {
                let version = output_of(Command::new(program).arg(flag).env("PATH", SYSTEM_PATH));
                let line = version.ok()?.lines().next()?.to_owned();
                Some(((*program).to_owned(), line))
            })
            .collect();
        Ok(Start {
            c2rust_version: REQUIRED_VERSION.to_owned(),
            rust_toolchain: rust_toolchain.to_owned(),
            system,
            files,
        })
    }

    /// The names of the files that are not the files of the start any more.
    fn changed_files(&self) -> Vec<String> {
        self.files
            .iter()
            .filter(|file| {
                hash_file(&file.path).ok().map(|(_, hash)| hash).as_ref() != Some(&file.sha256)
            })
            .map(|file| file.name.clone())
            .collect()
    }

    fn file(&self, name: &str) -> Option<&ToolFile> {
        self.files.iter().find(|file| file.name == name)
    }
}

/// The shared libraries of `program` that the record keeps: those outside the directories of
/// the system, and the LLVM and clang libraries. From the lines `name => path (address)` of
/// `ldd`.
fn loaded_libraries(program: &Path) -> Vec<PathBuf> {
    let listed = output_of(Command::new("ldd").arg(program)).unwrap_or_default();
    listed
        .lines()
        .filter_map(|line| line.split(" => ").nth(1)?.split(" (").next())
        .map(PathBuf::from)
        .filter(|library| {
            let name = library.file_name().unwrap_or_default().to_string_lossy();
            let system = ["/usr", "/lib"].iter().any(|dir| library.starts_with(dir));
            library.is_file() && (!system || name.contains("LLVM") || name.contains("clang"))
        })
        .collect()
}

/// The directory of the arm in the prompt record of the stage: the directory of its summary.
/// None for a run that keeps no prompt record.
fn record_dir(context: &HookContext<'_>) -> Option<PathBuf> {
    let summary = arm_summary_path(context.record_dir?, Profile::C2rust);
    summary.parent().map(Path::to_path_buf)
}

/// Records what decides the output of the tool. A run without a prompt record keeps nothing.
fn before_agent(context: &HookContext<'_>) -> Result<(), Box<dyn std::error::Error>> {
    let Some(dir) = record_dir(context) else {
        return Ok(());
    };
    fs::create_dir_all(&dir)?;
    let start = Start::detect(context.rust_toolchain)?;
    fs::write(
        dir.join(START_FILE),
        serde_json::to_string_pretty(&start)? + "\n",
    )?;
    Ok(())
}

/// Makes the report on the working directory and writes it, with the summary for the stage
/// manifest, into the record. Only a record that cannot be read or written is an error.
fn after_agent(context: &HookContext<'_>) -> Result<(), Box<dyn std::error::Error>> {
    let (Some(dir), Some(record)) = (record_dir(context), context.record_dir) else {
        return Ok(());
    };
    let case = context.case;
    let start: Start = serde_json::from_str(&fs::read_to_string(dir.join(START_FILE))?)?;
    keep_last_call_log(&case.workspace(), &dir)?;
    // The report is made only with the programs of the start, and is good only when they are
    // still the same after it.
    let mut changed = start.changed_files();
    let report = if changed.is_empty() {
        let report = run_report(case, &start);
        changed = start.changed_files();
        report
    } else {
        Err("a file of the tool changed before it".to_owned())
    };
    match &report {
        Ok(report) => write_report(report, &dir)?,
        Err(reason) => warn!("The report on the c2rust run was not made: {reason}"),
    }
    let summary = summary(
        &start,
        &inputs_link_problems(case),
        &changed,
        report.as_ref().map_err(String::as_str),
    );
    info!("c2rust run: comparison {}", summary["comparison"]);
    fs::write(
        arm_summary_path(record, Profile::C2rust),
        serde_json::to_string_pretty(&summary)? + "\n",
    )?;
    Ok(())
}

/// Copies the c2rust output of the last call of the agent into the record, when the scratch
/// directory has it as a regular file of a usual size.
fn keep_last_call_log(workspace: &Path, record: &Path) -> std::io::Result<()> {
    let log = workspace.join(LAST_CALL_LOG.0);
    let usual = fs::symlink_metadata(&log)
        .is_ok_and(|file| file.is_file() && file.len() <= LAST_CALL_LOG_LIMIT);
    if usual {
        fs::copy(log, record.join(LAST_CALL_LOG.1))?;
    }
    Ok(())
}

/// The problem of a workspace whose link `inputs` is not the link of the framework. The tool
/// of the agent reads the C source through it. The report does not: it gets the path of the C
/// source from the framework.
fn inputs_link_problems(case: &CaseDir) -> Vec<String> {
    let link = case.workspace().join("inputs");
    if fs::read_link(&link).is_ok_and(|target| target == case.inputs()) {
        return Vec::new();
    }
    vec![
        "the link `inputs` of the working directory does not point to the inputs of the run: \
         the tool of the agent read another C source. The report compares with the C source of \
         the run, so the difference is in the C edits"
            .to_owned(),
    ]
}

/// Runs `hand_changes.py` of the skill on the workspace of `case` and returns its report: one
/// JSON document with the counts, the full differences and the c2rust output.
///
/// The report works on files of the agent and runs the CMake code of its C copy, so it runs as
/// the agent commands do: under the file allowlist and the memory limit of the run. What the
/// agent controls must not reach it or its result:
/// - The scripts come from the binary, not from `inputs/`. They are written into a new directory
///   of the run directory, where the allowlist lets the report read them.
/// - Its programs are those of the system directories, the c2rust of the start by its path, and
///   the rustfmt of the start through a link in a new directory. Nothing of the Cargo home is
///   on its PATH, and it gets no other variable of the agent.
/// - It gets the C source of the run by the path of the framework, not through the workspace.
/// - Its result is its standard output, not a file: a file in the run directory could be written
///   by the CMake code. The programs that the report starts do not get this stream. The
///   document carries a token that only the report process received, on its standard input.
fn run_report(case: &CaseDir, start: &Start) -> Result<Value, String> {
    let io = |e: std::io::Error| e.to_string();
    let (c2rust, rustfmt) = match (start.file("c2rust"), start.file("rustfmt")) {
        (Some(c2rust), Some(rustfmt)) => (c2rust, rustfmt),
        _ => return Err("the record of the start has no c2rust or no rustfmt".to_owned()),
    };
    let work = tempfile::Builder::new()
        .prefix("c2rust_report.")
        .tempdir_in(case.root())
        .map_err(io)?;
    let skill = work.path().join("skill");
    agent_runner::materialize_skill(SKILL, &skill).map_err(io)?;
    let programs = work.path().join("bin");
    fs::create_dir(&programs).map_err(io)?;
    std::os::unix::fs::symlink(&rustfmt.path, programs.join("rustfmt")).map_err(io)?;
    // -I: Python takes no module from the home of the command or from its working directory.
    let script = format!(
        "cd {} && exec timeout --kill-after=30 {REPORT_SECONDS} python3 -I {} --json --c2rust {} \
         --original {}",
        quoted(&case.workspace()),
        quoted(&skill.join("scripts/hand_changes.py")),
        quoted(&c2rust.path),
        quoted(&case.c_src()),
    );
    let mut command = agent_runner::agent_command(case.root(), &script).map_err(io)?;
    command.env_clear();
    // The variables that the command environment itself needs, and the homes of the toolchain.
    for name in [
        "HOME",
        "XDG_RUNTIME_DIR",
        "DBUS_SESSION_BUS_ADDRESS",
        "CARGO_HOME",
        "RUSTUP_HOME",
    ] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    command.env("PATH", format!("{}:{SYSTEM_PATH}", programs.display()));
    command.env("TMPDIR", case.tmp());

    let token = random_token().map_err(io)?;
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(io)?;
    if let Some(mut stdin) = child.stdin.take() {
        // A report that ended before it read the token tells so with its status.
        let _ = writeln!(stdin, "{token}");
    }
    let output = child.wait_with_output().map_err(io)?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .lines()
        .rev()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|document| document["token"] == token.as_str())
        .ok_or_else(|| {
            let end = if output.status.code() == Some(124) {
                format!("it did not end in {REPORT_SECONDS} s")
            } else {
                format!("it ended with {}", output.status)
            };
            let stderr = String::from_utf8_lossy(&output.stderr);
            let last = stderr.trim().lines().last().unwrap_or_default();
            format!("{end} and gave no report. {last}")
        })
}

/// A word that a process without access to this one cannot guess.
fn random_token() -> std::io::Result<String> {
    let mut bytes = [0u8; 16];
    fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// `path` as one word of a shell command.
fn quoted(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', r"'\''"))
}

/// The texts of the report document that the record keeps as files of their own.
const REPORT_TEXTS: [(&str, &str); 3] = [
    ("hand_changes_diff", "hand_changes.diff"),
    ("c_edits_diff", "c_edits.diff"),
    ("transpile_log", "transpile.log"),
];

/// Writes the report into the record `dir`: `report.json`, and each text as its own file.
fn write_report(report: &Value, dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let mut document = report.clone();
    if let Some(fields) = document.as_object_mut() {
        fields.remove("token");
        for (field, file) in REPORT_TEXTS {
            if let Some(Value::String(text)) = fields.remove(field) {
                fs::write(dir.join(file), text)?;
            }
        }
    }
    fs::write(
        dir.join("report.json"),
        serde_json::to_string_pretty(&document)? + "\n",
    )?;
    Ok(())
}

/// The summary of the stage for the manifest. The record of the stage has the full report.
///
/// What the new translation of the report confirms is apart from what the tool recorded during
/// the run, which the agent could write. Without a comparison there is no count of C edits or
/// of hand changes.
fn summary(
    start: &Start,
    problems: &[String],
    changed_files: &[String],
    report: Result<&Value, &str>,
) -> Value {
    let c2rust = &start.files[0];
    let mut summary = json!({
        "c2rust": {"version": start.c2rust_version, "path": c2rust.path, "sha256": c2rust.sha256},
        "rust_toolchain": start.rust_toolchain,
        "problems": problems,
        "files_of_the_tool_changed_during_the_run": changed_files,
        "comparison": Value::Null,
        "new_translation": Value::Null,
        "c_edits": Value::Null,
        "rust_hand_changes": Value::Null,
        "record": format!(
            "{START_FILE}, report.json, {}, {} in this directory of the prompt record",
            REPORT_TEXTS.map(|(_, file)| file).join(", "),
            LAST_CALL_LOG.1
        ),
    });
    let counts = |changes: &Value| match changes {
        Value::Null => Value::Null,
        changes => json!({
            "files": changes["files"], "added": changes["added"], "removed": changes["removed"],
            "files_not_compared_by_lines": changes["files_not_compared_by_lines"],
        }),
    };
    let not_made = |reason: String| json!(format!("not made: {reason}"));
    // A changed file of the tool is the first reason: a report of another program does not count.
    let changed = (!changed_files.is_empty()).then(|| {
        not_made(format!(
            "{} changed during the run",
            changed_files.join(", ")
        ))
    });
    let report = match report {
        Ok(report) => report,
        Err(reason) => {
            let no_report = not_made(format!("the report was not made: {reason}"));
            summary["comparison"] = changed.unwrap_or(no_report);
            return summary;
        }
    };
    let made = changed.is_none() && report["comparison"]["made"] == true;
    summary["comparison"] = if let Some(changed) = changed {
        changed
    } else if made {
        json!(format!(
            "made with a new translation with the options of call {}",
            report["comparison"]["call"]
        ))
    } else {
        let reason = report["comparison"]["not_made_because"].as_str();
        not_made(reason.unwrap_or("no reason given").to_owned())
    };
    if changed_files.is_empty() && !report["messages"].is_null() {
        summary["new_translation"] = json!({
            "options": report["compared_options"],
            "c2rust_messages": report["messages"],
            "left_out_by_c2rust": report["left_out"],
        });
    }
    if made {
        summary["c_edits"] = counts(&report["c_edits"]);
        summary["rust_hand_changes"] = counts(&report["package_changes"]);
    }
    summary["calls_as_the_tool_recorded_them_not_verified"] = report["calls"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|call| {
            json!({
                "call": call["call"],
                "arguments": call["command"].as_array().map(|words| &words[words.len().min(1)..]),
                "exit_status": call["exit_status"],
                "wrote_package": !call["package"].is_null(),
            })
        })
        .collect();
    // The other files in two groups: those that are not changes by hand, and the rest.
    let reason = |change: &Value| {
        NOT_HAND_CHANGES
            .iter()
            .find(|(path, _)| change[0] == *path)
            .map(|(_, reason)| *reason)
    };
    let list = report["other_changes"]["list"].as_array();
    let (expected, other): (Vec<&Value>, Vec<&Value>) = list
        .into_iter()
        .flatten()
        .partition(|change| reason(change).is_some());
    let lines = |changes: &[&Value], column: usize| -> u64 {
        changes
            .iter()
            .filter_map(|change| change[column].as_u64())
            .sum()
    };
    summary[if made { "other_files" } else { "all_files" }] = json!({
        "files": other.len(), "added": lines(&other, 1), "removed": lines(&other, 2),
        "paths": other.iter().map(|change| &change[0]).collect::<Vec<_>>(),
    });
    summary["files_not_counted_as_hand_changes"] = expected
        .iter()
        .map(|change| json!({"path": change[0], "lines": change[1], "because": reason(change)}))
        .collect();
    summary
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::baseline::tests::{check_received, kinds};
    use crate::tests::received_from;
    use crate::{Config, Kit};
    use agent_runner::prompt_check::Received;
    use harvest_core::config::AgentKind;

    /// What the agent of the arm receives for a program of `kind`. The stage is the one that
    /// the arm builds, without the check of the host: the test of what the agent is given
    /// must not depend on the c2rust of the host.
    fn received(agent: AgentKind, program: &Program) -> Received {
        let config: Config = serde_json::from_value(json!({})).unwrap();
        let kit = Kit::Baseline(baseline());
        received_from(&kit, &config, agent, program, |_| ())
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn the_agent_receives_the_prompt_the_skill_and_the_shared_files() {
        let entries = [
            "inputs/prompts/c2rust/translate.md",
            "inputs/agent_skills/c2rust/SKILL.md",
            "inputs/agent_skills/c2rust/scripts/c2rust_translate.py",
            "inputs/agent_skills/c2rust/scripts/hand_changes.py",
            "inputs/agent_skills/c2rust/scripts/c2rust_translate_lib/__init__.py",
            "inputs/agent_skills/c2rust/scripts/c2rust_translate_lib/adapt.py",
            "inputs/agent_skills/c2rust/scripts/c2rust_translate_lib/c_build.py",
            "inputs/agent_skills/c2rust/scripts/c2rust_translate_lib/compare.py",
            "inputs/agent_skills/c2rust/scripts/c2rust_translate_lib/translate.py",
            "inputs/agent_skills/c2rust/scripts/c2rust_translate_lib/transpile.py",
            "inputs/agent_skills/c2rust/scripts/c2rust_translate_lib/workspace.py",
        ];
        for kind in kinds() {
            let program = Program {
                kind: &kind,
                configuration: None,
            };
            // The smoke test is a step of the prompt of the arm.
            check_received(
                Profile::C2rust,
                &program,
                |agent| received(agent, &program),
                &entries,
                &["smoke test"],
            );
        }
    }

    /// The anti-cheating rules forbid a tool that the instructions do not name. The instructions
    /// of this arm name the transpiler: the main prompt names the prompt file, which names it.
    #[test]
    fn the_instructions_of_the_arm_name_the_transpiler() {
        assert!(baseline().prompt.ends_with(MAIN));
        assert!(MAIN.contains(&format!("`inputs/prompts/{}`", TRANSLATE.0)));
        assert!(TRANSLATE.1.contains("with the transpiler `c2rust`"));
    }

    #[test]
    fn a_missing_or_other_c2rust_stops_the_stage_with_the_cause() {
        let dir = tempfile::tempdir().unwrap();
        let path = std::env::join_paths([dir.path()]).unwrap();
        let err = required_c2rust(Some(&path)).unwrap_err();
        assert!(err.contains("no `c2rust` on PATH"), "{err}");
        assert!(required_c2rust(None).is_err());

        let program = Path::new("/opt/bin/c2rust");
        check_version(program, REQUIRED_VERSION).unwrap();
        let err = check_version(program, "C2Rust 0.21.0").unwrap_err();
        assert!(
            err.contains("needs C2Rust 0.22.1: /opt/bin/c2rust is `C2Rust 0.21.0`"),
            "{err}"
        );
    }

    /// The package that the tool makes of the c2rust output of the skill tests (a library with
    /// a variadic function and an executable), in `dir`.
    fn package_of_the_tool(dir: &Path) {
        let skill = tempfile::tempdir().unwrap();
        agent_runner::materialize_skill(SKILL, skill.path()).unwrap();
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../agent_skills/c2rust/tests/fixture/c2rust_output");
        harvest_core::cargo_utils::copy_directory_recursive(&fixture, dir).unwrap();
        let adapt = "import sys; sys.dont_write_bytecode = True; sys.path.insert(0, sys.argv[1]); \
                     from c2rust_translate_lib import adapt; adapt.adapt_package(sys.argv[2], ['m'])";
        let status = Command::new("python3")
            .args(["-c", adapt])
            .arg(skill.path().join("scripts"))
            .arg(dir)
            .status()
            .unwrap();
        assert!(status.success());
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn the_post_processing_of_the_stage_keeps_a_package_of_the_tool() {
        use crate::tests::{LIB_AND_BIN, LIB_ONLY, harvest_bench};
        let manifest = |dir: &Path| fs::read_to_string(dir.join("Cargo.toml")).unwrap();

        let both = tempfile::tempdir().unwrap();
        package_of_the_tool(both.path());
        let before = manifest(both.path());
        crate::post_process(both.path(), &harvest_bench(LIB_AND_BIN)).unwrap();
        // The tool already gives the names and the crate types of a library with an executable.
        assert_eq!(manifest(both.path()), before);
        assert!(before.contains("name = \"toy_c2rust\"\npath = \"lib.rs\""));
        assert!(before.contains("crate-type = [\"cdylib\", \"rlib\"]"));
        assert!(before.contains("[[bin]]\npath = \"src/main.rs\"\nname = \"driver\""));

        let library = tempfile::tempdir().unwrap();
        package_of_the_tool(library.path());
        crate::post_process(library.path(), &harvest_bench(LIB_ONLY)).unwrap();
        let after = manifest(library.path());
        assert!(after.contains("name = \"toy_c2rust\"\npath = \"lib.rs\""));
        assert!(after.contains("crate-type = [\"cdylib\"]"));
        assert!(!after.contains("[[bin]]"));
        // The unit of the executable goes with its target. It is not a module of the library.
        assert!(!library.path().join("src/main.rs").exists());
        assert!(library.path().join("src/toy.rs").exists());
        assert!(
            !fs::read_to_string(library.path().join("lib.rs"))
                .unwrap()
                .contains("main")
        );
    }

    fn start(dir: &Path) -> Start {
        let files = ["c2rust", "c2rust-transpile", "rustfmt"].map(|name| {
            fs::write(dir.join(name), name).unwrap();
            ToolFile::of(name, dir.join(name)).unwrap()
        });
        Start {
            c2rust_version: REQUIRED_VERSION.to_owned(),
            rust_toolchain: "1.99.0".to_owned(),
            system: BTreeMap::new(),
            files: files.to_vec(),
        }
    }

    #[test]
    fn a_file_of_the_tool_that_changed_after_the_start_is_named() {
        let dir = tempfile::tempdir().unwrap();
        let start = start(dir.path());
        assert_eq!(start.changed_files(), [""; 0]);
        fs::write(dir.path().join("c2rust-transpile"), "another program").unwrap();
        fs::remove_file(dir.path().join("rustfmt")).unwrap();
        assert_eq!(start.changed_files(), ["c2rust-transpile", "rustfmt"]);
    }

    #[test]
    fn the_start_records_the_libraries_of_a_program_that_decide_its_output() {
        // The lines that `ldd` prints are read for this test from a program of the host.
        let Some(python) = find_program("python3", Some(OsStr::new(SYSTEM_PATH))) else {
            return;
        };
        for library in loaded_libraries(&python) {
            let name = library.file_name().unwrap().to_string_lossy().into_owned();
            let system = library.starts_with("/usr") || library.starts_with("/lib");
            assert!(
                !system || name.contains("LLVM") || name.contains("clang"),
                "{name}"
            );
        }
        assert!(loaded_libraries(Path::new("/no/such/program")).is_empty());
    }

    #[test]
    fn a_redirected_link_to_the_inputs_is_a_problem() {
        let case = CaseDir::new().unwrap();
        let link = case.workspace().join("inputs");
        std::os::unix::fs::symlink(case.inputs(), &link).unwrap();
        assert_eq!(inputs_link_problems(&case), [""; 0]);
        fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink("scratch/other", &link).unwrap();
        let problems = inputs_link_problems(&case);
        assert!(
            problems[0].contains("does not point to the inputs of the run"),
            "{problems:?}"
        );
        fs::remove_file(&link).unwrap();
        assert_eq!(inputs_link_problems(&case).len(), 1);
        case.finish();
    }

    #[test]
    fn the_record_keeps_the_report_its_texts_and_the_log_of_the_last_call() {
        let dir = tempfile::tempdir().unwrap();
        let report = json!({
            "token": "t", "comparison": {"made": true}, "hand_changes_diff": "+a\n",
            "c_edits_diff": null, "transpile_log": "Transpiling a.c\n",
        });
        write_report(&report, dir.path()).unwrap();
        let read = |name: &str| fs::read_to_string(dir.path().join(name)).unwrap();
        assert_eq!(read("hand_changes.diff"), "+a\n");
        assert_eq!(read("transpile.log"), "Transpiling a.c\n");
        assert!(!dir.path().join("c_edits.diff").exists());
        let kept: Value = serde_json::from_str(&read("report.json")).unwrap();
        assert_eq!(kept, json!({"comparison": {"made": true}}));

        let workspace = tempfile::tempdir().unwrap();
        keep_last_call_log(workspace.path(), dir.path()).unwrap();
        assert!(!dir.path().join(LAST_CALL_LOG.1).exists());
        let log = workspace.path().join(LAST_CALL_LOG.0);
        fs::create_dir_all(log.parent().unwrap()).unwrap();
        fs::write(&log, "panicked\n").unwrap();
        keep_last_call_log(workspace.path(), dir.path()).unwrap();
        assert_eq!(read(LAST_CALL_LOG.1), "panicked\n");
        // A link in the place of the log is not followed.
        fs::remove_file(&log).unwrap();
        fs::remove_file(dir.path().join(LAST_CALL_LOG.1)).unwrap();
        std::os::unix::fs::symlink("/etc/hostname", &log).unwrap();
        keep_last_call_log(workspace.path(), dir.path()).unwrap();
        assert!(!dir.path().join(LAST_CALL_LOG.1).exists());
    }

    /// A report of a run with one failed call, one call with a package, one C edit, one hand
    /// change, and the other files of a run.
    fn report() -> Value {
        json!({
            "calls": [
                {"call": 1, "command": ["tool.py", "--no-libc-define"], "exit_status": 101, "package": null},
                {"call": 2, "command": ["tool.py", "--", "-DA=1"], "exit_status": 0, "package": ["src"]},
            ],
            "comparison": {"made": true, "call": 2, "not_made_because": null},
            "compared_options": {"clang_args": ["-DA=1"]},
            "messages": {"functions_not_translated": 1, "panics": 0},
            "left_out": {"functions_not_translated": ["error: Failed to translate f: g"]},
            "c_edits": {"files": 1, "added": 1, "removed": 1, "files_not_compared_by_lines": 0,
                        "list": [["a.h", 1, 1]]},
            "package_changes": {"files": 1, "added": 3, "removed": 2, "files_not_compared_by_lines": 0,
                                "list": [["src/a.rs", 3, 2]]},
            "other_changes": {"files": 4, "added": 30, "removed": 0, "list": [
                [".gitignore", 7, 0], ["Cargo.lock", 20, 0], ["REPAIRS.md", 2, 0],
                ["c2rust_record/calls.jsonl", 1, 0]]},
        })
    }

    #[test]
    fn the_summary_separates_what_the_new_translation_confirms_from_what_the_tool_recorded() {
        let dir = tempfile::tempdir().unwrap();
        let start = start(dir.path());
        let summary = summary(&start, &[], &[], Ok(&report()));
        assert_eq!(summary["c2rust"]["version"], REQUIRED_VERSION);
        assert_eq!(summary["c2rust"]["sha256"], start.files[0].sha256);
        assert_eq!(
            summary["comparison"],
            "made with a new translation with the options of call 2"
        );
        assert_eq!(
            summary["new_translation"],
            json!({
                "options": {"clang_args": ["-DA=1"]},
                "c2rust_messages": {"functions_not_translated": 1, "panics": 0},
                "left_out_by_c2rust": {"functions_not_translated": ["error: Failed to translate f: g"]},
            })
        );
        assert_eq!(
            summary["calls_as_the_tool_recorded_them_not_verified"],
            json!([
                {"call": 1, "arguments": ["--no-libc-define"], "exit_status": 101, "wrote_package": false},
                {"call": 2, "arguments": ["--", "-DA=1"], "exit_status": 0, "wrote_package": true},
            ])
        );
        assert_eq!(
            summary["c_edits"],
            json!({"files": 1, "added": 1, "removed": 1, "files_not_compared_by_lines": 0})
        );
        assert_eq!(
            summary["rust_hand_changes"],
            json!({"files": 1, "added": 3, "removed": 2, "files_not_compared_by_lines": 0})
        );
        assert_eq!(
            summary["other_files"],
            json!({"files": 1, "added": 2, "removed": 0, "paths": ["REPAIRS.md"]})
        );
        assert_eq!(
            summary["files_not_counted_as_hand_changes"],
            json!([
                {"path": ".gitignore", "lines": 7, "because": "the framework writes it"},
                {"path": "Cargo.lock", "lines": 20, "because": "Cargo writes it at the first build"},
                {"path": "c2rust_record/calls.jsonl", "lines": 1,
                 "because": "the tool appends each call to it"},
            ])
        );
    }

    #[test]
    fn without_a_comparison_the_summary_has_no_count_of_changes() {
        let dir = tempfile::tempdir().unwrap();
        let start = start(dir.path());
        let no_counts = |summary: &Value| {
            assert_eq!(summary["c_edits"], Value::Null);
            assert_eq!(summary["rust_hand_changes"], Value::Null);
            assert!(summary.get("other_files").is_none());
        };

        // The record names no call with a package.
        let mut none = report();
        none["comparison"] = json!({"made": false, "call": null,
            "not_made_because": "no call in c2rust_record/calls.jsonl wrote a package"});
        for field in [
            "compared_options",
            "messages",
            "left_out",
            "c_edits",
            "package_changes",
        ] {
            none[field] = Value::Null;
        }
        let summary_of_none = summary(&start, &[], &[], Ok(&none));
        assert_eq!(
            summary_of_none["comparison"],
            "not made: no call in c2rust_record/calls.jsonl wrote a package"
        );
        assert_eq!(summary_of_none["new_translation"], Value::Null);
        assert_eq!(summary_of_none["all_files"]["paths"], json!(["REPAIRS.md"]));
        no_counts(&summary_of_none);

        // A program of the tool changed: the report of that program does not count.
        let changed = ["c2rust-transpile".to_owned()];
        let problems = ["the link is redirected".to_owned()];
        let summary_of_changed = summary(&start, &problems, &changed, Ok(&report()));
        assert_eq!(
            summary_of_changed["comparison"],
            "not made: c2rust-transpile changed during the run"
        );
        assert_eq!(
            summary_of_changed["files_of_the_tool_changed_during_the_run"],
            json!(["c2rust-transpile"])
        );
        assert_eq!(
            summary_of_changed["problems"],
            json!(["the link is redirected"])
        );
        assert_eq!(summary_of_changed["new_translation"], Value::Null);
        no_counts(&summary_of_changed);

        let before = summary(&start, &[], &changed, Err("a file of the tool changed"));
        assert_eq!(before["comparison"], summary_of_changed["comparison"]);

        let no_report = summary(&start, &[], &[], Err("it ended with exit status: 1"));
        assert_eq!(
            no_report["comparison"],
            "not made: the report was not made: it ended with exit status: 1"
        );
        no_counts(&no_report);
    }
}
