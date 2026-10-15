//! The placeholders that every main prompt shares, filled in by `invoke_agent`.

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
/// `opencode::load_registry_limits`); other backends leave this empty.
#[derive(Debug, Default)]
pub(crate) struct RegistryLimits {
    pub(crate) main: Option<ModelLimits>,
    pub(crate) subagent: Option<ModelLimits>,
}

impl AgentInvocation<'_> {
    /// The prompt with the placeholders that all main prompts share filled in:
    /// `{AGENT_BUG_WORKAROUNDS}`, `{GIT_DISCIPLINE}`, `{RUST_TOOLCHAIN_CONTEXT}`,
    /// `{CASE_LAYOUT}` and `{MODEL_LIMITS}`.
    pub(crate) fn render_prompt(&self, limits: &RegistryLimits) -> String {
        let model_limits = render_model_limits(self.subagent_model, limits);
        self.prompt
            .replace("{AGENT_BUG_WORKAROUNDS}", agent_bug_workarounds(self.agent))
            .replace("{GIT_DISCIPLINE}", git_discipline())
            .replace(
                "{RUST_TOOLCHAIN_CONTEXT}",
                &self.rust_toolchain.prompt_block,
            )
            .replace("{CASE_LAYOUT}", self.case.layout())
            .replace("{MODEL_LIMITS}", &model_limits)
    }
}

/// The text for the `{MODEL_LIMITS}` placeholder of the main prompts: the main
/// model's registry limits, and with a sub-agent model the "Sub-agent model"
/// section (the model, and its registry limits). Data only: the prompts say
/// how to use it.
fn render_model_limits(subagent_model: Option<&str>, limits: &RegistryLimits) -> String {
    let mut sections = Vec::new();
    if let Some(main) = &limits.main {
        sections.push(limits_section(
            "### Registry context limits",
            None,
            Some(main),
        ));
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

/// Agent-specific temporary bug workarounds, injected into every prompt.
/// Prompt templates carry an `{AGENT_BUG_WORKAROUNDS}` placeholder, and
/// prompt-building tools substitute this text for it. Each backend's text
/// lives in `agent_bug_workarounds/<agent>.md` and states the known upstream
/// bug and the behavior that avoids it. The main agent must copy it into
/// every sub-agent prompt, because sub-agents do not see this prompt. Add a
/// file when a backend needs a temporary fix, and remove it when the upstream
/// fix ships.
fn agent_bug_workarounds(agent: AgentKind) -> &'static str {
    match agent {
        AgentKind::Kiro => "",
        // Async sub-agents are killed when a headless `claude -p` session ends.
        AgentKind::Claude => include_str!("agent_bug_workarounds/claude.md").trim_end(),
        AgentKind::OpenCode => include_str!("agent_bug_workarounds/opencode.md").trim_end(),
    }
}

const GIT_DISCIPLINE_BODY: &str = include_str!("git_discipline.md");

fn git_discipline() -> &'static str {
    GIT_DISCIPLINE_BODY.trim_end()
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
            required_version: "1.94.1".to_owned(),
            prompt_block: "TOOLCHAIN".to_owned(),
            test_corpus_root: None,
        };
        let env = HashMap::new();
        let invocation = AgentInvocation {
            phase: AgentPhase::Translate,
            agent: AgentKind::Claude,
            case: &case,
            prompt: "{AGENT_BUG_WORKAROUNDS}\n{GIT_DISCIPLINE}\n{RUST_TOOLCHAIN_CONTEXT}\n\
                     {CASE_LAYOUT}\n{MODEL_LIMITS}",
            timeout_secs: 1,
            model: None,
            subagent_model: None,
            no_plan: false,
            extra_env: &env,
            output_log_path: None,
            rust_toolchain: &toolchain,
        };
        let prompt = invocation.render_prompt(&RegistryLimits::default());
        assert!(prompt.contains("TOOLCHAIN"));
        assert!(prompt.contains("### Directories of this run"));
        assert!(!prompt.contains('{'), "{prompt}");
        case.finish();
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
            "### Registry context limits\ncontext_limit: 1048576\noutput_limit: 128000\n\n\
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
