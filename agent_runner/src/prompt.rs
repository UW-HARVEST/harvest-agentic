//! The prompt files that every stage gets in `inputs/prompts/`: the rules that all agents, all
//! main agents and all sub-agents read, and `run.md`, the facts of the run. A prompt file names
//! another one by its path. The framework does not put the text of one file into another.

use std::fs;
use std::io;

use harvest_core::config::AgentKind;

use crate::{AgentInvocation, AgentPhase};

/// The directory of the prompt files, as the agents name it: relative to the root of a checkout.
pub const PROMPTS_PATH: &str = "inputs/prompts";

/// The prompt files that are the same in every run, by their name in `inputs/prompts/`.
const SHARED_FILES: [(&str, &str); 5] = [
    ("common.md", include_str!("../prompts/common.md")),
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

/// The rules of every main agent. Only a run with a state file gets them: nothing names this file
/// in a run without one.
const MAIN_AGENT_FILE: (&str, &str) = ("main_agent.md", include_str!("../prompts/main_agent.md"));

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

/// The prompt of the main agent of a stage: it names the prompt file of the stage, which holds
/// the task.
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

/// A prompt file that the run adds for an option of the stage. [`prepare_agent`](crate::prepare_agent)
/// writes it and `run.md` lists it for the main agent, which reads it and names the files of the
/// sub-agents in their prompts.
pub struct AdditionalPrompt {
    /// Relative to `inputs/prompts/`.
    pub path: String,
    /// What the file is for, in a few words.
    pub purpose: &'static str,
    /// The text of the file, written as it is.
    pub text: &'static str,
}

impl AgentInvocation<'_> {
    /// Writes the prompt files that every stage gets into `inputs/prompts/`: the shared rules
    /// (with the rules of every main agent only when the run has a state file), the known
    /// problems of the agent runtime, the additional prompt files of the run, and `run.md`, which
    /// lists them.
    pub(crate) fn write_shared_prompts(&self, limits: &RegistryLimits) -> io::Result<()> {
        let dir = self.case.prompts();
        fs::create_dir_all(&dir)?;
        let main_agent = self.plan_files_enabled().then_some(MAIN_AGENT_FILE);
        for (name, text) in SHARED_FILES.into_iter().chain(main_agent) {
            fs::write(dir.join(name), text)?;
        }
        fs::write(dir.join(RUNTIME_FILE), runtime_problems(self.agent))?;
        for file in self.additional_prompts {
            let path = dir.join(&file.path);
            fs::create_dir_all(path.parent().expect("a file has a parent"))?;
            fs::write(path, file.text)?;
        }
        fs::write(dir.join(RUN_FILE), self.run_file(limits))
    }

    /// The text of `run.md`: the stage, the agent runtime, the models and their limits, the Rust
    /// toolchain, the additional prompt files of the run, and the facts of the stage. Facts only:
    /// the prompt files say how to use them.
    pub(crate) fn run_file(&self, limits: &RegistryLimits) -> String {
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
            "Reader: every agent of this run. The framework wrote this file for this run. It holds"
                .to_owned(),
            "facts, not rules. The other prompt files tell how to use them.".to_owned(),
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
            "## Additional prompt files for the main agent".to_owned(),
            String::new(),
            "A sub-agent reads one of these files only when its task message names the file."
                .to_owned(),
            String::new(),
        ];
        if self.additional_prompts.is_empty() {
            lines.push("None.".to_owned());
        }
        for file in self.additional_prompts {
            lines.push(format!(
                "- `{PROMPTS_PATH}/{}`: {}",
                file.path, file.purpose
            ));
        }
        for fact in self.stage_facts {
            lines.push(String::new());
            lines.push(format!("## {}", fact.heading));
            lines.push(String::new());
            lines.push(fact.text.trim_end().to_owned());
        }
        lines.join("\n") + "\n"
    }
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
mod tests {
    use super::*;
    use crate::prompt_check::placeholders;
    use crate::{AgentPhase, CaseDir, RustToolchainContext};
    use std::collections::HashMap;

    /// An invocation in a new case directory, for a check of what it writes. With `options`, the
    /// run has an additional prompt file of the stage and the tool wishlist.
    fn with_invocation(
        agent: AgentKind,
        subagent_model: Option<&str>,
        options: bool,
        check: impl Fn(&AgentInvocation<'_>),
    ) {
        let case = CaseDir::new().unwrap();
        let toolchain = RustToolchainContext::for_version("1.99.0");
        let env = HashMap::new();
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
        check(&AgentInvocation {
            phase: AgentPhase::Verify,
            agent,
            case: &case,
            prompt: "",
            timeout_secs: 1,
            model: Some("provider/main"),
            subagent_model,
            no_plan: false,
            teamclaude: true,
            extra_env: &env,
            output_log_path: None,
            rust_toolchain: &toolchain,
            additional_prompts: if options { &additional } else { &[] },
            stage_facts: &facts,
        });
        case.finish();
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn shared_prompt_files_are_consistent() {
        for agent in [AgentKind::Claude, AgentKind::OpenCode] {
            with_invocation(agent, None, true, |invocation| {
                invocation
                    .write_shared_prompts(&RegistryLimits::default())
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
                assert_eq!(
                    read("main_agent.md"),
                    [
                        "common.md",
                        "git_main.md",
                        "main_agent.md",
                        "run.md",
                        "runtime.md"
                    ]
                );
                assert_eq!(
                    read("subagent.md"),
                    [
                        "common.md",
                        "git_subagent.md",
                        "run.md",
                        "runtime.md",
                        "subagent.md"
                    ]
                );
                assert!(files.contains_key("cargo_features.md"));
                assert_eq!(files["verify/method.md"], "# Method\n");
                assert!(files["tool_wishlist.md"].contains("tool_wishlist.json"));
                let runtime = match agent {
                    AgentKind::Claude => "Claude Code",
                    AgentKind::OpenCode => "OpenCode",
                };
                assert!(files["runtime.md"].contains(runtime));
            });
        }
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn a_run_without_options_lists_no_additional_file() {
        with_invocation(AgentKind::Claude, None, false, |off| {
            off.write_shared_prompts(&RegistryLimits::default())
                .unwrap();
            assert!(!off.case.prompts().join("tool_wishlist.md").exists());
            let run = off.run_file(&RegistryLimits::default());
            assert!(run.contains("## Additional prompt files for the main agent"));
            assert!(run.contains("\nNone.\n"), "{run}");
        });
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn a_run_without_state_file_gets_no_rules_of_a_main_agent() {
        let case = CaseDir::new().unwrap();
        let toolchain = RustToolchainContext::for_version("1.99.0");
        let env = HashMap::new();
        let invocation = AgentInvocation {
            phase: AgentPhase::Translate,
            agent: AgentKind::Claude,
            case: &case,
            prompt: "",
            timeout_secs: 1,
            model: None,
            subagent_model: None,
            no_plan: true,
            teamclaude: true,
            extra_env: &env,
            output_log_path: None,
            rust_toolchain: &toolchain,
            additional_prompts: &[],
            stage_facts: &[],
        };
        invocation
            .write_shared_prompts(&RegistryLimits::default())
            .unwrap();
        let prompts = case.prompts();
        assert!(!prompts.join("main_agent.md").exists());
        assert!(prompts.join("subagent.md").exists() && prompts.join("common.md").exists());
        assert_eq!(
            crate::prompt_check::problems(&case.inputs()).unwrap(),
            [""; 0]
        );
        case.finish();
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn run_file_gives_the_facts_of_the_run() {
        // Claude Code: no registry, so the default limits, and marked as such.
        with_invocation(AgentKind::Claude, None, true, |invocation| {
            let run = invocation.run_file(&RegistryLimits::default());
            for line in [
                "- Stage: verification\n",
                "- Agent runtime: Claude Code\n",
                "- Main agent: model `provider/main`\n\
                 \x20 - context window: 1000000 tokens (default: the agent runtime gives no value)\n\
                 \x20 - output limit of one response: 32000 tokens (default: the agent runtime gives no value)\n\
                 - Sub-agents: the same model and the same limits as the main agent\n",
                "- Required Rust toolchain: `1.99.0`\n",
                "- `inputs/prompts/verify/method.md`: the test method\n",
                "- `inputs/prompts/tool_wishlist.md`: ",
                "\n## External test suite\n\n- `inputs/suite/`\n",
            ] {
                assert!(run.contains(line), "{line:?} not in:\n{run}");
            }
            assert!(run.contains(&invocation.case.root().display().to_string()));
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
                let run = invocation.run_file(&limits);
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
                let run = invocation.run_file(&limits);
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

    #[test]
    fn main_prompt_names_the_prompt_file_of_the_stage() {
        assert_eq!(
            main_prompt(AgentPhase::Verify),
            "You are the main agent of the verification stage of a C-to-Rust translation run.\n\
             Read inputs/prompts/verify/main.md now, and do the task in it.\n"
        );
    }
}
