//! The files that the arm of a stage gives its agents (see [`Arm`]). Every stage gets the shared
//! files of the study (see `shared`). A HARVEST stage also gets the rules that all agents, all
//! main agents and all sub-agents read, the known problems of the agent runtime, and `run.md`,
//! the facts of the run. A baseline stage also gets the files that its
//! [`Baseline`](crate::Baseline) lists. A prompt file names another one by its path. The
//! framework does not put the text of one file into another.

use std::fs;
use std::io;
use std::path::Path;

use harvest_core::config::AgentKind;

use crate::{AgentInvocation, AgentPhase, Arm};

/// The directory of the prompt files, as the agents name it: relative to the root of a checkout.
pub const PROMPTS_PATH: &str = "inputs/prompts";

/// The line that starts the list of the files that the reader of a prompt file reads first.
pub const READ_FIRST: &str = "Read these files first:";

/// The prompt files of every HARVEST stage, by their name in `inputs/prompts/`.
const HARVEST_FILES: [(&str, &str); 6] = [
    ("common.md", include_str!("../prompts/common.md")),
    ("main_agent.md", include_str!("../prompts/main_agent.md")),
    ("subagent.md", include_str!("../prompts/subagent.md")),
    ("git_main.md", include_str!("../prompts/git_main.md")),
    (
        "git_subagent.md",
        include_str!("../prompts/git_subagent.md"),
    ),
    (
        "cargo_features.md",
        include_str!("../prompts/cargo_features.md"),
    ),
];

/// The known problems of the agent runtime of the run. Each backend has its text in
/// `prompts/runtime_<agent>.md`: the upstream bug and the behavior that avoids it. Add an item
/// when a backend needs a temporary fix, and remove it when the upstream fix ships.
const RUNTIME_FILE: &str = "runtime.md";
/// The facts of the run, which [`AgentInvocation::run_file`] generates.
const RUN_FILE: &str = "run.md";

/// The limits that `run.md` gives for a model without a registry value. The output limit is also
/// the cap that OpenCode keeps for every model when not every model of the run has a registry
/// output limit (see `opencode::output_cap`).
const DEFAULT_CONTEXT_LIMIT: u64 = 1_000_000;
const DEFAULT_OUTPUT_LIMIT: u64 = 32_000;

/// Experimental and off by default: the prompt file that asks the main agent to record the
/// analysis tools that it wanted in `tool_wishlist.json` of the crate. The tools whose config
/// turns it on pass it as an additional prompt file.
pub fn tool_wishlist() -> AdditionalPrompt {
    AdditionalPrompt {
        path: "tool_wishlist.md".to_owned(),
        purpose: "the record of the analysis tools that you wanted",
        text: include_str!("../prompts/tool_wishlist.md"),
    }
}

fn runtime_problems(agent: AgentKind) -> &'static str {
    match agent {
        // Async sub-agent kill, report-file guard, and the Agent tool's advice
        // against sub-agents.
        AgentKind::Claude => include_str!("../prompts/runtime_claude.md"),
        AgentKind::OpenCode => include_str!("../prompts/runtime_opencode.md"),
    }
}

/// The prompt of the main agent of a HARVEST stage: it names the prompt file of the stage, which
/// holds the task.
pub fn main_prompt(phase: AgentPhase) -> String {
    format!(
        "You are the main agent of the {} stage of a C-to-Rust translation run.\n\
         Read {PROMPTS_PATH}/{}/main.md now, and do the task in it.\n",
        phase.label(),
        phase.prompt_dir()
    )
}

/// A fact of the stage for `run.md`: a section with its heading.
pub struct StageFact<'a> {
    pub heading: &'a str,
    pub text: String,
}

/// A prompt file that the run adds for an option of the stage.
/// [`prepare_agent`](crate::prepare_agent) writes it and `run.md` lists it for the main agent,
/// which reads it and names the files of the sub-agents in their prompts.
pub struct AdditionalPrompt {
    /// Relative to `inputs/prompts/`.
    pub path: String,
    /// What the file is for, in a few words.
    pub purpose: &'static str,
    /// The text of the file, written as it is.
    pub text: &'static str,
}

impl AgentInvocation<'_> {
    /// The prompt of the main agent.
    pub(crate) fn prompt(&self) -> String {
        match self.arm {
            Arm::Harvest { .. } => main_prompt(self.phase),
            Arm::Baseline(baseline) => baseline.prompt.clone(),
        }
    }

    /// Writes what the arm gives every stage. Every arm: into `inputs/prompts/`, the shared
    /// files of the study. HARVEST: the rules of every stage, the known problems of the agent
    /// runtime, the additional prompt files of the run and `run.md`, and the skill that
    /// `cargo_features.md` describes. A baseline: the prompt files, the skills and the workspace
    /// files that the [`Baseline`](crate::Baseline) lists.
    pub(crate) fn write_files(
        &self,
        limits: &RegistryLimits,
        shared_limit: bool,
    ) -> io::Result<()> {
        let write = |dir: &Path, path: &str, text: &str| {
            let path = dir.join(path);
            fs::create_dir_all(path.parent().expect("a file has a parent"))?;
            fs::write(path, text)
        };
        let prompts = self.case.prompts();
        for (name, text) in crate::shared::FILES {
            write(&prompts, name, text)?;
        }
        match self.arm {
            Arm::Harvest {
                additional_prompts, ..
            } => {
                for (name, text) in HARVEST_FILES {
                    write(&prompts, name, text)?;
                }
                write(&prompts, RUNTIME_FILE, runtime_problems(self.agent))?;
                for file in additional_prompts {
                    write(&prompts, &file.path, file.text)?;
                }
                write(&prompts, RUN_FILE, &self.run_file(limits, shared_limit))?;
                self.case.write_skill("cargo_features")?;
            }
            Arm::Baseline(baseline) => {
                for (path, text) in &baseline.prompt_files {
                    // A file of an arm must not take the place of a shared file: every arm
                    // gets the same text.
                    if crate::shared::FILES.iter().any(|(name, _)| name == path) {
                        return Err(io::Error::other(format!(
                            "the {} arm gives the prompt file {path}, which is a shared file",
                            baseline.profile
                        )));
                    }
                    write(&prompts, path, text)?;
                }
                for skill in &baseline.skills {
                    self.case.write_skill(skill)?;
                }
                for (path, text) in &baseline.workspace_files {
                    write(&self.case.workspace(), path, text)?;
                }
            }
        }
        Ok(())
    }

    /// The text of `run.md`, which a HARVEST stage gets: the stage, the agent runtime, the models
    /// and their limits, the Rust toolchain, the machine resources, the additional prompt files
    /// of the run, and the facts of the stage. Facts only: the prompt files say how to use them.
    /// `shared_limit`: the agent commands run under the shared memory limit of `agent_env`.
    pub(crate) fn run_file(&self, limits: &RegistryLimits, shared_limit: bool) -> String {
        let runtime = match self.agent {
            AgentKind::Claude => "Claude Code",
            AgentKind::OpenCode => "OpenCode",
        };
        let model = |model: Option<&str>| match model {
            Some(model) => format!("model `{model}`"),
            None => "the default model of the agent runtime".to_owned(),
        };
        // OpenCode keeps its default output cap for every model when not every model of the run
        // has a registry output limit.
        let runtime_cap =
            self.agent == AgentKind::OpenCode && crate::opencode::output_cap(limits).is_none();
        let subagents = match self.subagent_model {
            Some(sub) => format!(
                "{}\n{}",
                model(Some(sub)),
                limit_lines(limits.subagent.as_ref(), runtime_cap)
            ),
            None => "the same model and the same limits as the main agent\n".to_owned(),
        };
        let toolchain = self.rust_toolchain;
        let mut lines = vec![
            "# Facts of this run".to_owned(),
            String::new(),
            "The framework wrote this file for this run. It holds facts, not rules. The other prompt"
                .to_owned(),
            "files tell how to use them.".to_owned(),
            String::new(),
            "## Stage".to_owned(),
            String::new(),
            format!("- Stage: {}", self.phase.label()),
            format!("- Agent runtime: {runtime}"),
            format!(
                "- Run directory: `{}`. The working directory of the main agent is its \
                 `workspace/`.",
                self.case.root().display()
            ),
            String::new(),
            "## Models".to_owned(),
            String::new(),
            format!(
                "- Main agent: {}\n{}- Sub-agents: {}",
                model(self.model),
                limit_lines(limits.main.as_ref(), runtime_cap),
                subagents.trim_end()
            ),
            String::new(),
            "## Rust toolchain".to_owned(),
            String::new(),
            format!(
                "- Required Rust toolchain: `{}`",
                toolchain.required_version
            ),
            format!("- Detected rustc: `{}`", toolchain.rustc_version),
            format!("- Detected cargo: `{}`", toolchain.cargo_version),
            String::new(),
            "## Machine resources".to_owned(),
            String::new(),
            machine_resources(shared_limit),
        ];
        if let Arm::Harvest {
            additional_prompts,
            stage_facts,
        } = self.arm
        {
            lines.extend([
                String::new(),
                "## Additional prompt files for the main agent".to_owned(),
                String::new(),
                "A sub-agent reads one of these files only when its task message names the file."
                    .to_owned(),
                String::new(),
            ]);
            if additional_prompts.is_empty() {
                lines.push("None.".to_owned());
            }
            for file in additional_prompts {
                lines.push(format!(
                    "- `{PROMPTS_PATH}/{}`: {}",
                    file.path, file.purpose
                ));
            }
            for fact in stage_facts {
                lines.push(String::new());
                lines.push(format!("## {}", fact.heading));
                lines.push(String::new());
                lines.push(fact.text.trim_end().to_owned());
            }
        }
        lines.join("\n") + "\n"
    }
}

/// The `run.md` section on the CPUs and the memory of the machine, measured now.
fn machine_resources(shared_limit: bool) -> String {
    let cpus = std::thread::available_parallelism().map_or(1, |n| n.get());
    let meminfo = fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let gib = |key: &str| {
        meminfo
            .lines()
            .find_map(|line| line.strip_prefix(key)?.strip_prefix(':'))
            .and_then(|rest| rest.split_whitespace().next()?.parse::<u64>().ok())
            .map_or_else(
                || "unknown".to_owned(),
                |kib| format!("{:.1} GiB", kib as f64 / (1024.0 * 1024.0)),
            )
    };
    format!(
        "These values were measured when the stage started. Other runs can use the same machine \
         at the same time, so the free part changes during the stage.\n\n\
         - CPUs available to the agent commands: {cpus}\n\
         - Memory: {} in total, {} available at the start of the stage; swap: {}\n\
         - {}",
        gib("MemTotal"),
        gib("MemAvailable"),
        gib("SwapTotal"),
        if shared_limit {
            format!(
                "All harvest agent commands on this machine share one limit: {}. The commands \
                 of other runs count against it too.",
                crate::agent_env::shared_limits()
            )
        } else {
            "The agent commands run without a shared memory limit.".to_owned()
        }
    )
}

/// The two limit lines of a model in `run.md`: the registry values, or the defaults. With
/// `runtime_cap`, the agent runtime caps the output of every model at the default.
fn limit_lines(limits: Option<&ModelLimits>, runtime_cap: bool) -> String {
    let no_value = " (default: the agent runtime gives no value)";
    let cap = " (the default cap of the agent runtime: not every model of this run has an output \
               limit)";
    let (context, context_note) = match limits {
        Some(limits) => (limits.context, ""),
        None => (DEFAULT_CONTEXT_LIMIT, no_value),
    };
    let (output, output_note) = match limits.and_then(|limits| limits.output) {
        Some(output) if runtime_cap && output > DEFAULT_OUTPUT_LIMIT => (DEFAULT_OUTPUT_LIMIT, cap),
        Some(output) => (output, ""),
        None => (DEFAULT_OUTPUT_LIMIT, no_value),
    };
    format!(
        "  - context window: {context} tokens{context_note}\n  \
         - output limit of one response: {output} tokens{output_note}\n"
    )
}

/// A model's context window and output limit, from a backend's model registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ModelLimits {
    pub(crate) context: u64,
    pub(crate) output: Option<u64>,
}

/// The registry limits of the run's models, read once per stage: `run.md`
/// and the backend both use them. Only OpenCode has a registry (see
/// `opencode::load_registry_limits`); Claude Code leaves this empty.
#[derive(Debug, Default)]
pub(crate) struct RegistryLimits {
    pub(crate) main: Option<ModelLimits>,
    pub(crate) subagent: Option<ModelLimits>,
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::prompt_check::placeholders;
    use crate::{AgentPhase, CaseDir, RustToolchainContext};
    use std::collections::HashMap;

    /// A HARVEST invocation of the verify stage in a new case directory, for a check of what it
    /// writes. With `options`, the run has an additional prompt file of the stage and the tool
    /// wishlist.
    pub(crate) fn with_invocation(
        agent: AgentKind,
        subagent_model: Option<&str>,
        options: bool,
        check: impl Fn(&AgentInvocation<'_>),
    ) {
        let additional = [
            AdditionalPrompt {
                path: "verify/method.md".to_owned(),
                purpose: "the test method",
                text: "# Method\n",
            },
            tool_wishlist(),
        ];
        let facts = [StageFact {
            heading: "External test suite",
            text: "- `inputs/suite/`\n".to_owned(),
        }];
        let case = CaseDir::new().unwrap();
        let toolchain = RustToolchainContext::for_version("1.99.0");
        let env = HashMap::new();
        check(&AgentInvocation {
            phase: AgentPhase::Verify,
            agent,
            case: &case,
            arm: Arm::Harvest {
                additional_prompts: if options { &additional } else { &[] },
                stage_facts: &facts,
            },
            timeout_secs: 1,
            model: Some("provider/main"),
            subagent_model,
            teamclaude: true,
            extra_env: &env,
            output_log_path: None,
            prompt_record_dir: None,
            rust_toolchain: &toolchain,
        });
        case.finish();
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn harvest_prompt_files_are_consistent() {
        for agent in [AgentKind::Claude, AgentKind::OpenCode] {
            with_invocation(agent, None, true, |invocation| {
                invocation
                    .write_files(&RegistryLimits::default(), true)
                    .unwrap();
                let inputs = invocation.case.inputs();
                // The additional prompt files are written where run.md names them.
                assert_eq!(crate::prompt_check::problems(&inputs).unwrap(), [""; 0]);
                let files = crate::prompt_check::load(&inputs).unwrap();
                let read = |entry: &str| -> Vec<String> {
                    crate::prompt_check::must_read(&files, entry)
                        .into_iter()
                        .collect()
                };
                // Every agent reads the facts of the environment and the anti-cheating rules,
                // which are shared files of the study.
                let every_agent = ["anti_cheating.md", "environment.md", "run.md", "runtime.md"];
                let with = |own: [&'static str; 3]| -> Vec<&'static str> {
                    let mut all = [&own[..], &every_agent].concat();
                    all.sort();
                    all
                };
                assert_eq!(
                    read("main_agent.md"),
                    with(["common.md", "git_main.md", "main_agent.md"])
                );
                assert_eq!(
                    read("subagent.md"),
                    with(["common.md", "git_subagent.md", "subagent.md"])
                );
                // A sub-agent without a role file starts at subagent.md, which lists all.
                assert_eq!(
                    crate::prompt_check::not_listed(&files, "subagent.md"),
                    [""; 0]
                );
                // The skill comes with the prompt file that describes it.
                assert!(files.contains_key("cargo_features.md"));
                assert!(
                    inputs
                        .join("agent_skills/cargo_features/SKILL.md")
                        .is_file()
                );
                assert_eq!(files["verify/method.md"], "# Method\n");
                assert!(files["tool_wishlist.md"].contains("tool_wishlist.json"));
                let runtime = match agent {
                    AgentKind::Claude => "Claude Code",
                    AgentKind::OpenCode => "OpenCode",
                };
                assert!(files["runtime.md"].contains(runtime));
                // A stage reads the output specification where its own files name it.
                assert!(files.contains_key("output_spec.md"));
                assert_eq!(
                    invocation.prompt(),
                    "You are the main agent of the verification stage of a C-to-Rust translation \
                     run.\nRead inputs/prompts/verify/main.md now, and do the task in it.\n"
                );
            });
        }
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn a_run_without_options_lists_no_additional_file() {
        with_invocation(AgentKind::Claude, None, false, |off| {
            off.write_files(&RegistryLimits::default(), true).unwrap();
            assert!(!off.case.prompts().join("tool_wishlist.md").exists());
            let run = off.run_file(&RegistryLimits::default(), true);
            assert!(run.contains("## Additional prompt files for the main agent"));
            assert!(run.contains("\nNone.\n"), "{run}");
        });
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn run_file_gives_the_facts_of_the_run() {
        // Claude Code: no registry, so the default limits, and marked as such.
        with_invocation(AgentKind::Claude, None, true, |invocation| {
            let run = invocation.run_file(&RegistryLimits::default(), true);
            for line in [
                "- Stage: verification\n",
                "- Agent runtime: Claude Code\n",
                "- Main agent: model `provider/main`\n\
                 \x20 - context window: 1000000 tokens (default: the agent runtime gives no value)\n\
                 \x20 - output limit of one response: 32000 tokens (default: the agent runtime gives no value)\n\
                 - Sub-agents: the same model and the same limits as the main agent\n",
                "- Required Rust toolchain: `1.99.0`\n",
                "\n## Machine resources\n\n",
                "- CPUs available to the agent commands: ",
                " in total, ",
                "share one limit: 5 GiB of memory and 10 GiB of swap (`harvest.slice`).",
                "- `inputs/prompts/verify/method.md`: the test method\n",
                "- `inputs/prompts/tool_wishlist.md`: ",
                "\n## External test suite\n\n- `inputs/suite/`\n",
            ] {
                assert!(run.contains(line), "{line:?} not in:\n{run}");
            }
            assert!(run.contains(&invocation.case.root().display().to_string()));
            // Without the shared limit, run.md does not claim one.
            let unlimited = invocation.run_file(&RegistryLimits::default(), false);
            assert!(unlimited.contains("- The agent commands run without a shared memory limit."));
            assert!(!unlimited.contains("harvest.slice"));
            assert_eq!(placeholders(&run), [""; 0]);
        });
        // OpenCode, both models with a registry output limit: the registry values hold.
        with_invocation(
            AgentKind::OpenCode,
            Some("provider/sub"),
            true,
            |invocation| {
                let limits = RegistryLimits {
                    main: Some(ModelLimits {
                        context: 1_048_576,
                        output: Some(128_000),
                    }),
                    subagent: Some(ModelLimits {
                        context: 262_144,
                        output: Some(65_536),
                    }),
                };
                let run = invocation.run_file(&limits, true);
                let models = "- Main agent: model `provider/main`\n\
                 \x20 - context window: 1048576 tokens\n\
                 \x20 - output limit of one response: 128000 tokens\n\
                 - Sub-agents: model `provider/sub`\n\
                 \x20 - context window: 262144 tokens\n\
                 \x20 - output limit of one response: 65536 tokens\n";
                assert!(run.contains(models), "{run}");
            },
        );
        // OpenCode, the sub-agent model without an output limit: OpenCode keeps its default cap
        // for every model, so run.md gives the cap, not the larger registry value.
        with_invocation(
            AgentKind::OpenCode,
            Some("provider/sub"),
            true,
            |invocation| {
                let limits = RegistryLimits {
                    main: Some(ModelLimits {
                        context: 1_048_576,
                        output: Some(128_000),
                    }),
                    subagent: Some(ModelLimits {
                        context: 262_144,
                        output: None,
                    }),
                };
                let run = invocation.run_file(&limits, true);
                let models = "- Main agent: model `provider/main`\n\
                 \x20 - context window: 1048576 tokens\n\
                 \x20 - output limit of one response: 32000 tokens (the default cap of the agent \
                 runtime: not every model of this run has an output limit)\n\
                 - Sub-agents: model `provider/sub`\n\
                 \x20 - context window: 262144 tokens\n\
                 \x20 - output limit of one response: 32000 tokens (default: the agent runtime gives no value)\n";
                assert!(run.contains(models), "{run}");
            },
        );
    }
}
