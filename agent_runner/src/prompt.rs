//! The placeholders that the prompts and the framework-written files of a stage share, filled
//! in by [`PreparedAgent::render`](crate::PreparedAgent::render).

use harvest_core::config::AgentKind;

use crate::AgentInvocation;

/// A model's context window and output limit, from a backend's model registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ModelLimits {
    pub(crate) context: u64,
    pub(crate) output: Option<u64>,
}

/// The registry limits of the run's models, read once per stage: the prompt
/// and the backend both use them. Only OpenCode has a registry (see
/// `opencode::load_registry_limits`); Claude Code leaves this empty.
#[derive(Debug, Default)]
pub(crate) struct RegistryLimits {
    pub(crate) main: Option<ModelLimits>,
    pub(crate) subagent: Option<ModelLimits>,
}

impl AgentInvocation<'_> {
    /// `text` with the shared placeholders filled in: `{SUBAGENT_RULES}` (the rules of
    /// [`SUBAGENT_RULES_FILE`] under a `##` heading, for a task file that carries them),
    /// `{AGENT_BUG_WORKAROUNDS}`, `{GIT_DISCIPLINE}`, `{GIT_SUBAGENT}`,
    /// `{RUST_TOOLCHAIN_CONTEXT}`, `{CASE_LAYOUT}`, `{MODEL_LIMITS}` and `{CARGO_FEATURES}`.
    pub(crate) fn render(&self, text: &str, limits: &RegistryLimits) -> String {
        let model_limits = render_model_limits(self.subagent_model, limits);
        let subagent_rules = format!("## Rules for every sub-agent\n\n{}", subagent_rules());
        // First, because its text has placeholders of its own.
        text.replace("{SUBAGENT_RULES}", &subagent_rules)
            .replace("{AGENT_BUG_WORKAROUNDS}", agent_bug_workarounds(self.agent))
            .replace("{GIT_DISCIPLINE}", git_discipline())
            .replace("{GIT_SUBAGENT}", GIT_SUBAGENT_BODY.trim_end())
            .replace(
                "{RUST_TOOLCHAIN_CONTEXT}",
                &self.rust_toolchain.prompt_block,
            )
            .replace("{CASE_LAYOUT}", self.case.layout())
            .replace("{MODEL_LIMITS}", &model_limits)
            .replace("{CARGO_FEATURES}", cargo_features())
    }

    /// The text of [`SUBAGENT_RULES_FILE`]: the rules that a sub-agent without a task file of
    /// its own reads first.
    pub(crate) fn subagent_rules_file(&self, limits: &RegistryLimits) -> String {
        let text = format!(
            "# Rules for every sub-agent\n\nYou are a sub-agent of this run. These rules apply \
             to all of your work. Your task message gives the task.\n\n{}\n",
            subagent_rules()
        );
        self.render(&text, limits)
    }
}

/// The file in `inputs/prompts/` that every stage gets: the directories of the run, the
/// toolchain, the git rules of a sub-agent and the agent bug workarounds. The main agent names it
/// in the prompt of a sub-agent, and so copies no rules. A task file of a role carries the same
/// rules through `{SUBAGENT_RULES}`.
pub const SUBAGENT_RULES_FILE: &str = "SUBAGENT_RULES.md";

const SUBAGENT_RULES_BODY: &str = include_str!("subagent_rules.md");
const GIT_SUBAGENT_BODY: &str = include_str!("git_subagent.md");

fn subagent_rules() -> &'static str {
    SUBAGENT_RULES_BODY.trim_end()
}

/// The text for the `{MODEL_LIMITS}` placeholder of the main prompts: the main
/// model's registry limits, and with a sub-agent model the "Sub-agent model"
/// section (the model, and its registry limits). Data only: the prompts say
/// how to use it.
fn render_model_limits(subagent_model: Option<&str>, limits: &RegistryLimits) -> String {
    let mut sections = Vec::new();
    if let Some(main) = &limits.main {
        sections.push(limits_section("### Model limits", None, Some(main)));
    }
    if let Some(sub) = subagent_model {
        sections.push(limits_section(
            "### Sub-agent model",
            Some(sub),
            limits.subagent.as_ref(),
        ));
    }
    sections.join("\n\n")
}

/// A heading and its `key: value` lines: the model, if given, then the
/// registry limits, if known.
fn limits_section(heading: &str, model: Option<&str>, limits: Option<&ModelLimits>) -> String {
    let mut lines = vec![heading.to_string()];
    lines.extend(model.map(|model| format!("model: {model}")));
    if let Some(limits) = limits {
        lines.push(format!("context_limit: {}", limits.context));
        lines.extend(
            limits
                .output
                .map(|output| format!("output_limit: {output}")),
        );
    }
    lines.join("\n")
}

/// Agent-specific temporary bug workarounds, injected into every prompt and,
/// through the sub-agent rules, into what every sub-agent reads. Each
/// backend's text lives in `agent_bug_workarounds/<agent>.md` and states the
/// known upstream bug and the behavior that avoids it. Add a file when a
/// backend needs a temporary fix, and remove it when the upstream fix ships.
fn agent_bug_workarounds(agent: AgentKind) -> &'static str {
    match agent {
        // Async sub-agent kill, report-file guard, and the Agent tool's advice
        // against sub-agents.
        AgentKind::Claude => include_str!("agent_bug_workarounds/claude.md").trim_end(),
        AgentKind::OpenCode => include_str!("agent_bug_workarounds/opencode.md").trim_end(),
    }
}

const GIT_DISCIPLINE_BODY: &str = include_str!("git_discipline.md");

fn git_discipline() -> &'static str {
    GIT_DISCIPLINE_BODY.trim_end()
}

const CARGO_FEATURES_BODY: &str = include_str!("cargo_features.md");

/// The rule for the Cargo features of the C build options, and how to use the `cargo_features`
/// skill that [`CaseDir::new`](crate::CaseDir::new) gives every stage. The `{CARGO_FEATURES}`
/// placeholder.
fn cargo_features() -> &'static str {
    CARGO_FEATURES_BODY.trim_end()
}

/// The placeholders (`{UPPER_CASE}`) that `text` has. The tools use it to test that a file for
/// the agents has none left.
pub fn placeholders(text: &str) -> Vec<&str> {
    let mut found = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find('{') {
        rest = &rest[start..];
        let name_len = rest[1..]
            .find(|c: char| !(c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_'))
            .unwrap_or(rest.len() - 1);
        if name_len > 0 && rest[1 + name_len..].starts_with('}') {
            found.push(&rest[..name_len + 2]);
        }
        rest = &rest[1..];
    }
    found
}

const TOOL_WISHLIST_BODY: &str = include_str!("tool_wishlist.md");

/// Fills the `{TOOL_WISHLIST}` placeholder of the translate and verify prompts.
/// Experimental and off by default: when on, the agent records the analysis
/// tools it wanted in `tool_wishlist.json` of the crate; when off, the
/// placeholder becomes empty.
pub fn fill_tool_wishlist(prompt: &str, on: bool) -> String {
    let section = if on {
        TOOL_WISHLIST_BODY.trim_end()
    } else {
        ""
    };
    prompt.replace("{TOOL_WISHLIST}", section)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AgentPhase, CaseDir, RustToolchainContext};
    use std::collections::HashMap;

    #[test]
    #[cfg_attr(miri, ignore)]
    fn render_prompt_fills_shared_placeholders() {
        let case = CaseDir::new().unwrap();
        let toolchain = RustToolchainContext {
            required_version: "1.99.0".to_owned(),
            prompt_block: "TOOLCHAIN".to_owned(),
            test_corpus_root: None,
        };
        let env = HashMap::new();
        let invocation = AgentInvocation {
            phase: AgentPhase::Translate,
            agent: AgentKind::Claude,
            case: &case,
            prompt: "{AGENT_BUG_WORKAROUNDS}\n{GIT_DISCIPLINE}\n{RUST_TOOLCHAIN_CONTEXT}\n\
                     {CASE_LAYOUT}\n{MODEL_LIMITS}\n{CARGO_FEATURES}\n{SUBAGENT_RULES}",
            timeout_secs: 1,
            model: None,
            subagent_model: None,
            no_plan: false,
            teamclaude: true,
            extra_env: &env,
            output_log_path: None,
            rust_toolchain: &toolchain,
        };
        let limits = RegistryLimits::default();
        let prompt = invocation.render(invocation.prompt, &limits);
        assert!(prompt.contains("TOOLCHAIN"));
        assert_eq!(prompt.matches("### Directories of this run").count(), 2);
        assert!(prompt.contains("### Cargo features"));
        assert!(prompt.contains("## Rules for every sub-agent"));
        assert!(prompt.contains("### Git rules for a sub-agent"));
        assert!(!prompt.contains('{'), "{prompt}");
        // The rules file names itself (in the layout), and no prompt tells an agent to copy rules.
        let rules = invocation.subagent_rules_file(&limits);
        assert!(rules.starts_with("# Rules for every sub-agent"));
        assert!(
            rules.contains(SUBAGENT_RULES_FILE) && !rules.contains('{'),
            "{rules}"
        );
        assert!(!prompt.contains("Copy this"), "{prompt}");
        case.finish();
    }

    #[test]
    fn placeholders_are_upper_case_names_in_braces() {
        assert_eq!(
            placeholders("a {X} b {GIT_DISCIPLINE}{} {x} {A1}c { B } `{`"),
            ["{X}", "{GIT_DISCIPLINE}", "{A1}"]
        );
        assert!(placeholders("fn f() { EXPECT_EQ(RunC(kMode, {}), 1); }").is_empty());
    }

    #[test]
    fn tool_wishlist_is_filled_only_when_on() {
        let on = fill_tool_wishlist("a\n{TOOL_WISHLIST}\nb", true);
        assert!(on.contains("tool_wishlist.json"), "{on}");
        assert!(!on.contains("{TOOL_WISHLIST}"));
        assert_eq!(fill_tool_wishlist("a\n{TOOL_WISHLIST}\nb", false), "a\n\nb");
    }

    #[test]
    fn model_limits_list_the_registry_and_subagent_sections() {
        // OpenCode, both models in the registry.
        let both = RegistryLimits {
            main: Some(ModelLimits {
                context: 1_048_576,
                output: Some(128_000),
            }),
            subagent: Some(ModelLimits {
                context: 262_144,
                output: Some(65_536),
            }),
        };
        assert_eq!(
            render_model_limits(Some("opencode-go/mimo-v2.6-flash"), &both),
            "### Model limits\ncontext_limit: 1048576\noutput_limit: 128000\n\n\
             ### Sub-agent model\nmodel: opencode-go/mimo-v2.6-flash\n\
             context_limit: 262144\noutput_limit: 65536"
        );
        // Claude Code: no registry, so only the model.
        let none = RegistryLimits::default();
        assert_eq!(
            render_model_limits(Some("haiku"), &none),
            "### Sub-agent model\nmodel: haiku"
        );
        assert_eq!(render_model_limits(None, &none), "");
    }
}
