//! `benchmark --dry-run`: the stage renders what its agent would receive, and no agent starts.
//!
//! The tests run the benchmark binary on a small bench case. Every agent command on its PATH
//! is a stand-in that leaves a marker and fails when it would start an agent.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// The file that a stand-in agent command writes when it starts.
const STARTED: &str = "agent_started";

/// The key of the provider in the user configuration that the stand-in `opencode` prints, and
/// the value of a variable in the `env` table of the run config. No output can have them.
const SECRETS: [&str; 2] = ["sk-PROVIDER-SECRET", "sk-ENV-SECRET"];

/// A directory to run the benchmark in: the bench case `bench/toy` (a library), the snapshot
/// `snapshot/toy` of a HARVEST translate stage, and the stand-in commands in `bin/`: the agent
/// commands, and a `c2rust` that only tells the version of the study. The stand-in `opencode`
/// answers what the framework asks before a run (the configuration of the user, with a custom
/// provider, and the model registry).
struct Fixture {
    dir: tempfile::TempDir,
    /// The Rust toolchain of the runs.
    rust_toolchain: String,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let write = |path: &str, text: &str| {
            let path = dir.path().join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        };
        // The toolchain contract of the run is the file of the working directory.
        let rustc = Command::new("rustc").arg("--version").output().unwrap();
        let rustc = String::from_utf8(rustc.stdout).unwrap();
        let version = rustc.split_whitespace().nth(1).unwrap().to_owned();
        write(
            "rust-toolchain.toml",
            &format!("[toolchain]\nchannel = \"{version}\"\n"),
        );
        let c_source = [
            (
                "CMakeLists.txt",
                "project(toy C)\nadd_library(toy SHARED toy.c)\n",
            ),
            ("toy.c", "int toy(void) { return 1; }\n"),
        ];
        for (name, text) in c_source {
            write(&format!("bench/toy/test_case/{name}"), text);
            write(&format!("snapshot/toy/.harvest/c_src/{name}"), text);
        }
        write(
            "bench/toy/gtest_suite/manifest.json",
            r#"{"program": {"lib": {"c_target": "toy"}}}"#,
        );
        write(
            "snapshot/toy/Cargo.toml",
            "[package]\nname = \"toy\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        );
        write("snapshot/toy/src/lib.rs", "// the translation\n");
        let manifest = serde_json::json!({
            "schema_version": 2,
            "bench_program": "toy",
            "runs": [{
                "stages": ["translate"], "argv": [], "cwd": "/", "harvest_version": "test",
                "created_unix": 0, "config": null
            }]
        });
        write("snapshot/toy/.harvest/stage.json", &manifest.to_string());
        let started = dir.path().join(STARTED);
        let start =
            |agent: &str| format!("echo {agent} \"$@\" >> '{}'; exit 97", started.display());
        let provider = serde_json::json!({"provider": {"myprov": {
            "npm": "@ai-sdk/openai-compatible",
            "options": {"baseURL": "https://example.invalid/v1", "apiKey": SECRETS[0]},
            "models": {"m1": {}}
        }}});
        let registry =
            r#"{"providerID":"myprov","id":"m1","limit":{"context":100000,"output":8000}}"#;
        let opencode = format!(
            "case \"$1\" in\n  debug) echo '{provider}';;\n  models) echo '{registry}';;\n  \
             *) {};;\nesac",
            start("opencode")
        );
        let commands = [
            ("claude", start("claude")),
            ("teamclaude", start("teamclaude")),
            ("opencode", opencode),
            ("c2rust", "echo 'C2Rust 0.22.1'".to_owned()),
            ("c2rust-transpile", "exit 0".to_owned()),
        ];
        for (name, script) in commands {
            write(&format!("bin/{name}"), &format!("#!/bin/sh\n{script}\n"));
            let path = dir.path().join("bin").join(name);
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        Fixture {
            dir,
            rust_toolchain: version,
        }
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.dir.path().join(relative)
    }

    /// Runs the benchmark with `args` in the directory of the fixture, and checks that no agent
    /// command started.
    fn benchmark(&self, args: &[&str]) -> Output {
        let path = std::env::var("PATH").unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_benchmark"))
            .args(args)
            .current_dir(self.dir.path())
            .env("PATH", format!("{}:{path}", self.path("bin").display()))
            // No user configuration of the host.
            .env("XDG_CONFIG_HOME", self.path("no_config"))
            .output()
            .unwrap();
        assert!(!self.path(STARTED).exists(), "an agent command started");
        output
    }

    /// A dry-run that succeeds. Returns the directory of the program in its output `out`.
    fn dry_run(&self, args: &[&str], out: &str) -> PathBuf {
        let output = self.benchmark(&[args, &["--dry-run", out]].concat());
        assert!(output.status.success(), "{output:?}");
        // Nothing is graded, and the output is not a snapshot.
        let program = self.path(out).join("toy");
        assert!(!self.path(out).join("results.csv").exists());
        assert!(!program.join(".harvest/stage.json").exists());
        assert!(!program.join("Cargo.toml").exists());
        program
    }
}

/// The files below `dir` that have `text`.
fn files_with(dir: &Path, text: &str) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let meta = fs::symlink_metadata(&path).unwrap();
        if meta.is_dir() {
            found.extend(files_with(&path, text));
        } else if meta.is_file()
            && String::from_utf8_lossy(&fs::read(&path).unwrap()).contains(text)
        {
            found.push(path);
        }
    }
    found
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

#[test]
fn a_harvest_translate_stage_is_rendered_and_no_agent_starts() {
    let fixture = Fixture::new();
    let program = fixture.dry_run(&["--agentic=t", "--agent", "claude", "bench"], "out");
    let summary = read(&program.join("DRY_RUN.md"));
    assert!(summary.starts_with("# Dry-run of the translate stage\n"));
    assert!(summary.contains("- Profile: harvest\n- Agent runtime: claude\n"));
    assert!(summary.contains("- `.harvest/prompts/translate/`: the record"));
    assert!(read(&program.join("PROMPT.md")).contains("inputs/prompts/translate/main.md"));
    // The command line of the stand-in TeamClaude on PATH, with the text for the system prompt.
    let hint = read(&program.join("SYSTEM_PROMPT.md"));
    assert!(hint.starts_with("After any context compaction"), "{hint}");
    let argv = read(&program.join("ARGV.txt"));
    assert!(argv.starts_with("teamclaude\nrun\n--\n-p\n"), "{argv}");
    assert!(argv.contains(&format!("--append-system-prompt\n{hint}\n")));
    assert!(!program.join("RESUME_PROMPT.md").exists());

    // The run directory, as the agent would find it.
    let run_dir = program.join("run_dir");
    for file in [
        "inputs/c_src/toy.c",
        "inputs/prompts/run.md",
        "inputs/prompts/translate/main.md",
        "inputs/agent_skills/tab2rust/SKILL.md",
        "workspace/PLAN.md",
        "workspace/.gitignore",
        "workspace/.git/HEAD",
        ".claude/settings.json",
        "agent_env/harvest-shell",
    ] {
        assert!(run_dir.join(file).is_file(), "{file}");
    }
    // Its files name it by its path, and the links of the workspace point into it.
    let run_dir_name = run_dir.to_str().unwrap();
    assert!(read(&run_dir.join("inputs/prompts/run.md")).contains(run_dir_name));
    assert_eq!(
        fs::read_link(run_dir.join("workspace/inputs")).unwrap(),
        run_dir.join("inputs")
    );
    // A person can remove it.
    let inputs = fs::metadata(run_dir.join("inputs/c_src/toy.c")).unwrap();
    assert!(!inputs.permissions().readonly());
    // The record of a run is kept too.
    assert!(program
        .join(".harvest/prompts/translate/given.json")
        .is_file());

    // The output of a dry-run is no input of a later stage.
    let later = fixture.benchmark(&["--agentic=v", "--dry-run", "out", "out_verify"]);
    assert!(!later.status.success());
    let err = String::from_utf8_lossy(&later.stderr);
    assert!(err.contains("nothing to render"), "{err}");
}

#[test]
fn a_baseline_stage_is_rendered_with_only_the_files_of_its_arm() {
    let fixture = Fixture::new();
    let stage = [
        "--agentic=t",
        "--baseline=vanilla",
        "--agent",
        "oc",
        "bench",
    ];
    let program = fixture.dry_run(&stage, "out");
    assert!(read(&program.join("DRY_RUN.md")).contains("- Profile: vanilla\n"));
    let prompt = read(&program.join("PROMPT.md"));
    assert!(prompt.contains("Translate the C project in `inputs/c_src` to Rust."));
    assert!(read(&program.join("ARGV.txt")).starts_with("opencode\nrun\n"));
    assert!(read(&program.join("RESUME_PROMPT.md")).contains("provider failure"));
    assert!(!program.join("SYSTEM_PROMPT.md").exists());
    let run_dir = program.join("run_dir");
    for file in [
        "inputs/c_src/toy.c",
        "inputs/prompts/output_spec.md",
        "workspace/.opencode/opencode.json",
    ] {
        assert!(run_dir.join(file).is_file(), "{file}");
    }
    for entry in [
        "inputs/prompts/translate",
        "inputs/prompts/run.md",
        "inputs/prompts/runtime.md",
        "inputs/agent_skills",
        "workspace/PLAN.md",
    ] {
        assert!(!run_dir.join(entry).exists(), "{entry}");
    }
}

#[test]
fn a_later_stage_is_rendered_from_a_snapshot() {
    let fixture = Fixture::new();
    let stage = ["--agentic=v", "--agent", "claude", "snapshot"];
    let program = fixture.dry_run(&stage, "out");
    assert!(read(&program.join("DRY_RUN.md")).starts_with("# Dry-run of the verify stage\n"));
    assert!(read(&program.join("PROMPT.md")).contains("inputs/prompts/verify/main.md"));
    let run_dir = program.join("run_dir");
    // The crate of the snapshot, and what the stage adds for its agent.
    assert_eq!(
        read(&run_dir.join("workspace/src/lib.rs")),
        "// the translation\n"
    );
    for file in [
        "workspace/HYPOTHESES.md",
        "inputs/prompts/verify/main.md",
        "inputs/c_src/toy.c",
    ] {
        assert!(run_dir.join(file).is_file(), "{file}");
    }
    // A baseline does not render a stage on the translation of another arm.
    let actor = [
        "--agentic=v",
        "--baseline=actor",
        "--dry-run",
        "snapshot",
        "x",
    ];
    let refused = fixture.benchmark(&actor);
    assert!(!refused.status.success());
    let err = String::from_utf8_lossy(&refused.stderr);
    assert!(
        err.contains("resumes only from a snapshot that only runs with --baseline=actor made"),
        "{err}"
    );
}

#[test]
fn a_dry_run_of_two_stages_is_refused_before_anything_is_written() {
    let fixture = Fixture::new();
    let refused = fixture.benchmark(&["--agentic=t,v", "--dry-run", "bench", "out"]);
    assert!(!refused.status.success());
    let err = String::from_utf8_lossy(&refused.stderr);
    assert!(
        err.contains("a dry-run renders one stage, and this run has 2: translate, verify."),
        "{err}"
    );
    assert!(!fixture.path("out").exists());
}

#[test]
fn an_opencode_stage_with_a_model_is_rendered_without_a_secret() {
    let fixture = Fixture::new();
    let env = format!("tools.translate_agentic.env.MY_API_KEY={}", SECRETS[1]);
    let stage = [
        "--agentic=t",
        "--agent",
        "oc",
        "--model",
        "myprov/m1",
        "-c",
        &env,
        "bench",
    ];
    let program = fixture.dry_run(&stage, "out");
    // The limits of the model are those of the registry, as in a run.
    let run = read(&program.join("run_dir/inputs/prompts/run.md"));
    assert!(
        run.contains("model `myprov/m1`\n  - context window: 100000 tokens\n"),
        "{run}"
    );
    assert!(read(&program.join("ARGV.txt")).ends_with("--model\nmyprov/m1\n"));
    // The project has the provider of the user, without its key.
    let project = read(&program.join("run_dir/workspace/.opencode/opencode.json"));
    let project: serde_json::Value = serde_json::from_str(&project).unwrap();
    assert_eq!(
        project["provider"]["myprov"]["options"],
        serde_json::json!({"baseURL": "https://example.invalid/v1", "apiKey": "<masked>"})
    );
    // The variables of the agent process: the values of the framework, and only the name of
    // the one of the run config.
    let env = read(&program.join("ENV.txt"));
    let run_dir = program.join("run_dir");
    let expected = format!(
        "PWD={0}/workspace\nRUSTUP_TOOLCHAIN={1}\nTMPDIR={0}/tmp\nXDG_CONFIG_HOME={0}/xdg-config\n\
         OPENCODE_EXPERIMENTAL_OUTPUT_TOKEN_MAX=8000\n",
        run_dir.display(),
        fixture.rust_toolchain
    );
    assert_eq!(env, format!("{expected}MY_API_KEY=<masked>\n"));
    let summary = read(&program.join("DRY_RUN.md"));
    assert!(
        summary.contains("are replaced with `<masked>`"),
        "{summary}"
    );
    assert!(summary.contains("\n## Not shown here\n"), "{summary}");
    for secret in SECRETS {
        assert_eq!(
            files_with(&fixture.path("out"), secret),
            [""; 0].map(PathBuf::from)
        );
    }
}

#[test]
fn the_c2rust_arm_is_rendered_with_its_start_record() {
    let fixture = Fixture::new();
    let stage = [
        "--agentic=t",
        "--baseline=c2rust",
        "--agent",
        "claude",
        "bench",
    ];
    let program = fixture.dry_run(&stage, "out");
    assert!(read(&program.join("DRY_RUN.md")).contains("- Profile: c2rust\n"));
    assert!(program
        .join("run_dir/inputs/agent_skills/c2rust/SKILL.md")
        .is_file());
    // The check of the start ran on the stand-in, and the hook before the agent kept its
    // record, which is not a file for the agent.
    let start = read(&program.join(".harvest/prompts/translate/c2rust/start.json"));
    let start: serde_json::Value = serde_json::from_str(&start).unwrap();
    assert_eq!(start["c2rust_version"], "C2Rust 0.22.1");
    assert_eq!(
        start["files"][0]["path"],
        fixture.path("bin/c2rust").to_str().unwrap()
    );
    assert!(!program.join("run_dir/workspace/c2rust").exists());
}
