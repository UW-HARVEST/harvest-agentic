//! The run's OpenCode project: config, agent definition, permissions, providers and plugins.

use std::fs;
use std::path::Path;
use tracing::warn;

use super::registry::{parse_opencode_model, run_opencode_models};
use crate::{AgentInvocation, CaseDir, agent_env, command_stdout};

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
const OPENCODE_COMPACTION_PLUGIN: &str = include_str!("compaction_recovery.js");

/// Project-local OpenCode plugin that rewrites an empty sub-agent `task`
/// result into an explicit failure report. Written for every OpenCode run
/// (unlike the compaction plugin, this needs no per-phase substitution). See
/// the plugin source for the mechanism and the upstream issues.
const OPENCODE_RESILIENCE_PLUGIN: &str = include_str!("resilience.js");

const OPENCODE_LOCAL_PERMISSIONS: &[(&str, &str)] = &[
    ("bash", "allow"),
    ("read", "allow"),
    ("edit", "allow"),
    ("write", "allow"),
    ("glob", "allow"),
    ("grep", "allow"),
    ("todowrite", "allow"),
    ("lsp", "allow"),
    ("webfetch", "deny"),
    ("websearch", "deny"),
    ("skill", "deny"),
];

/// The agents that may run as sub-agents: OpenCode's built-in sub-agents. The
/// main agent's `task` permission allows only these. `task.ts` (v1.18.32)
/// starts any agent named in `subagent_type`, primary agents included, and
/// one without a configured model would run on the main model.
const OPENCODE_SUBAGENTS: &[&str] = &["general", "explore"];

/// The main agent's `task` permission, as frontmatter lines: deny every
/// agent, then allow [`OPENCODE_SUBAGENTS`]. OpenCode applies the last
/// matching rule, and hides the `task` tool when the last `task` rule is a
/// deny of `*`, so the deny comes first. A deny holds under
/// `--dangerously-skip-permissions`, which approves only what no rule denies.
fn opencode_task_permission() -> String {
    let mut lines = String::from("  task:\n    \"*\": deny\n");
    for name in OPENCODE_SUBAGENTS {
        lines.push_str(&format!("    {name}: allow\n"));
    }
    lines
}

/// What the run's OpenCode project config carries besides its fixed settings.
#[derive(Default)]
struct OpenCodeProjectOptions<'a> {
    model: Option<&'a str>,
    subagent_model: Option<&'a str>,
    /// The custom-provider definitions of the run's models (see
    /// `custom_provider_stages`).
    staged_providers: &'a [(String, serde_json::Value)],
    /// The shell for tool calls (see `agent_env`).
    shell: Option<&'a Path>,
}

/// Writes the run's OpenCode project into the workspace of the invocation's case.
pub(crate) fn write_project(
    invocation: &AgentInvocation<'_>,
) -> Result<(), Box<dyn std::error::Error>> {
    write_opencode_agent(
        invocation.case,
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
        invocation.subagent_model,
        agent_env::install(invocation.case.root()).as_deref(),
    )
}

/// Writes the OpenCode project config and agent definition into the workspace of `case`.
fn write_opencode_agent(
    case: &CaseDir,
    config: OpenCodeAgentConfig<'_>,
    model: Option<&str>,
    subagent_model: Option<&str>,
    shell: Option<&Path>,
) -> Result<(), Box<dyn std::error::Error>> {
    let work_dir = case.workspace();
    let agents_dir = work_dir.join(".opencode/agents");
    fs::create_dir_all(&agents_dir)?;

    let models: Vec<&str> = model.into_iter().chain(subagent_model).collect();
    let staged = custom_provider_stages(&models)?;
    fs::write(
        work_dir.join(".opencode/opencode.json"),
        opencode_project_config(
            case.root(),
            &OpenCodeProjectOptions {
                model,
                subagent_model,
                staged_providers: &staged,
                shell,
            },
        ),
    )?;

    let mut permissions = String::new();
    for (tool, policy) in OPENCODE_LOCAL_PERMISSIONS {
        permissions.push_str(&format!("  {tool}: {policy}\n"));
    }
    permissions.push_str(&opencode_task_permission());

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

/// Project-level OpenCode config written alongside the agent definitions.
/// `external_directory` defaults to "ask", and task sub-agent sessions do not
/// inherit `--dangerously-skip-permissions`, so in a headless run any
/// sub-agent tool call that touches a path outside the project directory
/// blocks forever on the unanswerable permission prompt. Scoping external
/// access to this run's temp directory and denying everything else makes "ask"
/// unreachable.
///
/// `snapshot` is disabled. OpenCode snapshots a git work tree twice per agent
/// step to back interactive undo/revert. It is the heaviest path-lookup load
/// and coincided with `path_is_under` kernel Oops. A headless run does not need it anyways.
///
/// `shell` runs the agent's commands under memory limits (see `agent_env`).
/// OpenCode uses it for tool calls, sub-agents included.
///
/// The sub-agent model becomes the model of [`OPENCODE_SUBAGENTS`]. The `task`
/// tool runs a sub-agent on its agent's configured model and falls back to the
/// caller's model only when none is configured (`task.ts`, v1.18.32); the tool
/// has no model parameter, so the main agent cannot pick another one.
fn opencode_project_config(case_root: &Path, options: &OpenCodeProjectOptions<'_>) -> String {
    let tempdir_pattern = format!("{}/**", case_root.display());
    let models: Vec<&str> = options
        .model
        .into_iter()
        .chain(options.subagent_model)
        .collect();
    let provider_block = match opencode_provider_config(&models, options.staged_providers) {
        Some(value) => format!(
            ",\n  \"provider\": {}",
            serde_json::to_string(&value).expect("provider block serializes to JSON"),
        ),
        None => String::new(),
    };
    let agent_block = options
        .subagent_model
        .map(|sub| {
            let agents: serde_json::Map<String, serde_json::Value> = OPENCODE_SUBAGENTS
                .iter()
                .map(|name| (name.to_string(), serde_json::json!({ "model": sub })))
                .collect();
            format!(
                ",\n  \"agent\": {}",
                serde_json::to_string(&agents).expect("agent block serializes to JSON"),
            )
        })
        .unwrap_or_default();
    let shell_line = options
        .shell
        .map(|path| {
            format!(
                "\n  \"shell\": {},",
                serde_json::to_string(&path.display().to_string())
                    .expect("path serializes to JSON")
            )
        })
        .unwrap_or_default();
    format!(
        r#"{{
  "$schema": "https://opencode.ai/config.json",
  "snapshot": false,{}
  "permission": {{
    "external_directory": {{
      "*": "deny",
      {}: "allow"
    }}
  }}{}{}
}}
"#,
        shell_line,
        serde_json::to_string(&tempdir_pattern).expect("path pattern serializes to JSON"),
        provider_block,
        agent_block,
    )
}

/// The `provider` section for the run's models: the OpenRouter pins, then the
/// staged custom providers. A pin wins over a staged entry of the same
/// provider (`or_insert_with`): a user shadowing a built-in provider (e.g.
/// openrouter) keeps today's routing behavior.
fn opencode_provider_config(
    models: &[&str],
    staged_providers: &[(String, serde_json::Value)],
) -> Option<serde_json::Value> {
    let pins: serde_json::Map<String, serde_json::Value> = models
        .iter()
        .filter_map(|model| openrouter_provider_pin(model))
        .collect();
    let mut providers = serde_json::Map::new();
    if !pins.is_empty() {
        providers.insert(
            "openrouter".to_string(),
            serde_json::json!({ "models": pins }),
        );
    }
    for (id, definition) in staged_providers {
        providers
            .entry(id.clone())
            .or_insert_with(|| definition.clone());
    }
    (!providers.is_empty()).then_some(serde_json::Value::Object(providers))
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
/// lands on an inferior host. Returns the model's registry id and its
/// provider options, or None for non-OpenRouter models (direct providers route
/// to a single upstream and need no hint).
fn openrouter_provider_pin(model: &str) -> Option<(String, serde_json::Value)> {
    let rest = model.strip_prefix("openrouter/")?;
    // Drop any routing suffix (e.g. ":floor") so the key matches the registry
    // model id, then take the author segment as the first-party provider slug.
    let model_id = rest.split(':').next().unwrap_or(rest);
    let author = model_id.split('/').next().filter(|s| !s.is_empty())?;
    Some((
        model_id.to_string(),
        serde_json::json!({
            "options": {
                "provider": { "only": [author], "allow_fallbacks": false }
            }
        }),
    ))
}

/// The custom-provider definitions of the run's models, for staging into the
/// run's project config.
///
/// `invoke_opencode` points XDG_CONFIG_HOME at an empty directory, which hides
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
///
/// Each provider is looked up once, and `opencode debug config` runs once.
fn custom_provider_stages(models: &[&str]) -> Result<Vec<(String, serde_json::Value)>, String> {
    let provider_ids = provider_ids(models);
    if provider_ids.is_empty() {
        return Ok(Vec::new());
    }
    let resolved = match command_stdout("opencode", &["debug", "config"]).and_then(|output| {
        serde_json::from_str::<serde_json::Value>(&output)
            .map_err(|error| -> Box<dyn std::error::Error> { error.into() })
    }) {
        Ok(value) => value,
        Err(error) => {
            warn!("opencode debug config unreadable, staging nothing: {error}");
            return Ok(Vec::new());
        }
    };
    let mut staged = Vec::new();
    for provider_id in provider_ids {
        if let Some(definition) = resolved.get("provider").and_then(|p| p.get(&provider_id)) {
            staged.push((provider_id, definition.clone()));
        } else if run_opencode_models(Some(&provider_id)).is_err() {
            // A built-in provider resolves without user config; this one does not.
            return Err(format!(
                "OpenCode provider `{provider_id}` is defined nowhere: `opencode models \
                 {provider_id}` fails and `opencode debug config` has no entry for it. Define it \
                 in ~/.config/opencode/opencode.jsonc, or export OPENCODE_CONFIG to a config file \
                 that defines it, and rerun."
            ));
        }
    }
    Ok(staged)
}

/// The distinct providers of `provider/model` ids, in order.
fn provider_ids(models: &[&str]) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    for model in models {
        if let Ok((id, _)) = parse_opencode_model(model)
            && !ids.contains(&id)
        {
            ids.push(id);
        }
    }
    ids
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn task_permission_allows_only_the_subagents() {
        let permission = opencode_task_permission();
        // The deny of every agent comes first: the last matching rule wins,
        // and a last `task` rule that denies `*` would hide the tool.
        assert!(permission.starts_with("  task:\n    \"*\": deny\n"));
        for name in OPENCODE_SUBAGENTS {
            assert!(permission.contains(&format!("    {name}: allow\n")));
        }
        assert!(!permission.trim_end().ends_with("deny"));
    }

    #[test]
    fn opencode_project_config_scopes_external_directory_to_case_dir() {
        let case_root = Path::new("/tmp/.tmpAbc123");
        let raw = opencode_project_config(case_root, &OpenCodeProjectOptions::default());
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
    fn opencode_project_config_disables_snapshot() {
        let case_root = Path::new("/tmp/.tmpAbc123");
        let staged = (
            "harvest-hyak".to_string(),
            serde_json::json!({"npm": "@ai-sdk/openai-compatible"}),
        );
        for (model, staged) in [
            (None, &[][..]),
            (Some("openrouter/xiaomi/mimo-v2.5-pro"), &[][..]),
            (
                Some("harvest-hyak/qwen3.8-27b"),
                std::slice::from_ref(&staged),
            ),
        ] {
            let raw = opencode_project_config(
                case_root,
                &OpenCodeProjectOptions {
                    model,
                    staged_providers: staged,
                    ..Default::default()
                },
            );
            let config: serde_json::Value =
                serde_json::from_str(&raw).expect("project config must be valid JSON");
            assert_eq!(config.get("snapshot"), Some(&serde_json::json!(false)));
        }
    }

    #[test]
    fn opencode_project_config_sets_shell_only_when_given() {
        let case_root = Path::new("/tmp/.tmpAbc123");
        let shell = Path::new("/tmp/.tmpAbc123/agent_env/harvest-shell");
        let with: serde_json::Value = serde_json::from_str(&opencode_project_config(
            case_root,
            &OpenCodeProjectOptions {
                shell: Some(shell),
                ..Default::default()
            },
        ))
        .expect("project config must be valid JSON");
        assert_eq!(
            with.get("shell"),
            Some(&serde_json::json!(
                "/tmp/.tmpAbc123/agent_env/harvest-shell"
            ))
        );
        let without: serde_json::Value =
            serde_json::from_str(&opencode_project_config(case_root, &Default::default())).unwrap();
        assert!(without.get("shell").is_none());
    }

    #[test]
    fn opencode_project_config_pins_openrouter_to_author_endpoint() {
        let case_root = Path::new("/tmp/.tmpAbc123");
        let raw = opencode_project_config(
            case_root,
            &OpenCodeProjectOptions {
                model: Some("openrouter/xiaomi/mimo-v2.5-pro"),
                ..Default::default()
            },
        );
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
        let case_root = Path::new("/tmp/.tmpAbc123");
        let raw = opencode_project_config(
            case_root,
            &OpenCodeProjectOptions {
                model: Some("opencode-go/mimo-v2.5"),
                ..Default::default()
            },
        );
        let config: serde_json::Value =
            serde_json::from_str(&raw).expect("project config must be valid JSON");
        assert!(config.get("provider").is_none());
    }

    #[test]
    fn opencode_project_config_stages_custom_provider() {
        let case_root = Path::new("/tmp/.tmpAbc123");
        let definition = serde_json::json!({
            "npm": "@ai-sdk/openai-compatible",
            "options": { "baseURL": "https://endpoint.example/v1" }
        });
        let raw = opencode_project_config(
            case_root,
            &OpenCodeProjectOptions {
                model: Some("harvest-hyak/qwen3.8-27b"),
                staged_providers: &[("harvest-hyak".to_string(), definition)],
                ..Default::default()
            },
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
        let case_root = Path::new("/tmp/.tmpAbc123");
        let raw = opencode_project_config(
            case_root,
            &OpenCodeProjectOptions {
                model: Some("openrouter/xiaomi/mimo-v2.5-pro"),
                staged_providers: &[(
                    "openrouter".to_string(),
                    serde_json::json!({"npm": "shadowed"}),
                )],
                ..Default::default()
            },
        );
        let config: serde_json::Value =
            serde_json::from_str(&raw).expect("project config must be valid JSON");
        assert!(config.pointer("/provider/openrouter/models").is_some());
        assert!(config.pointer("/provider/openrouter/npm").is_none());
    }

    #[test]
    fn opencode_project_config_sets_subagent_model() {
        let case_root = Path::new("/tmp/.tmpAbc123");
        let raw = opencode_project_config(
            case_root,
            &OpenCodeProjectOptions {
                model: Some("openrouter/xiaomi/mimo-v2.5-pro"),
                subagent_model: Some("openrouter/deepseek/deepseek-v4-flash"),
                ..Default::default()
            },
        );
        let config: serde_json::Value =
            serde_json::from_str(&raw).expect("project config must be valid JSON");
        for name in OPENCODE_SUBAGENTS {
            assert_eq!(
                config.pointer(&format!("/agent/{name}/model")),
                Some(&serde_json::json!("openrouter/deepseek/deepseek-v4-flash"))
            );
        }
        // Both models are pinned to their author's endpoint.
        let pins = config.pointer("/provider/openrouter/models").unwrap();
        assert!(pins.get("xiaomi/mimo-v2.5-pro").is_some());
        assert!(pins.get("deepseek/deepseek-v4-flash").is_some());
        // Without a sub-agent model the sub-agents keep the main model.
        let raw = opencode_project_config(
            case_root,
            &OpenCodeProjectOptions {
                model: Some("opencode-go/mimo-v2.5"),
                ..Default::default()
            },
        );
        let config: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert!(config.get("agent").is_none());
    }

    #[test]
    fn opencode_project_config_pins_openrouter_and_stages_custom_provider() {
        let raw = opencode_project_config(
            Path::new("/tmp/.tmpAbc123"),
            &OpenCodeProjectOptions {
                model: Some("openrouter/xiaomi/mimo-v2.5-pro"),
                subagent_model: Some("harvest-hyak/qwen3.8-27b"),
                staged_providers: &[(
                    "harvest-hyak".to_string(),
                    serde_json::json!({"npm": "@ai-sdk/openai-compatible"}),
                )],
                ..Default::default()
            },
        );
        let config: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert!(
            config
                .pointer("/provider/openrouter/models/xiaomi~1mimo-v2.5-pro")
                .is_some()
        );
        assert_eq!(
            config.pointer("/provider/harvest-hyak/npm"),
            Some(&serde_json::json!("@ai-sdk/openai-compatible"))
        );
    }

    #[test]
    fn provider_ids_are_distinct_and_skip_malformed_models() {
        assert_eq!(
            provider_ids(&["harvest-hyak/a", "harvest-hyak/b", "opencode-go/c", "bare"]),
            ["harvest-hyak", "opencode-go"]
        );
        // No provider: nothing to look up, so no `opencode` call.
        assert!(custom_provider_stages(&["bare"]).unwrap().is_empty());
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn write_opencode_agent_writes_project_config() {
        let case = CaseDir::new().unwrap();
        let dir = case.workspace();
        write_opencode_agent(
            &case,
            OpenCodeAgentConfig {
                name: "harvest-translate",
                description: "test",
                recovery_command: None,
            },
            None,
            None,
            None,
        )
        .unwrap();
        let config = fs::read_to_string(dir.join(".opencode/opencode.json")).unwrap();
        assert_eq!(
            config,
            opencode_project_config(case.root(), &Default::default())
        );
        let agent_md =
            fs::read_to_string(dir.join(".opencode/agents/harvest-translate.md")).unwrap();
        // The body must be EMPTY so `agent.prompt` stays falsy and OpenCode's
        // official default provider system prompt applies. The compaction
        // hint lives in the plugin, not here.
        assert!(agent_md.ends_with("---\n"));
        assert!(!agent_md.contains("compaction"));
        assert!(agent_md.contains(&opencode_task_permission()));
        // No recovery command -> no plugin.
        assert!(!dir.join(".opencode/plugin/compaction-recovery.js").exists());
        case.finish();
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn write_opencode_agent_writes_compaction_recovery_plugin() {
        let case = CaseDir::new().unwrap();
        let dir = case.workspace();
        write_opencode_agent(
            &case,
            OpenCodeAgentConfig {
                name: "harvest-verify",
                description: "test",
                recovery_command: Some("cat PLAN.md HYPOTHESES.md".to_string()),
            },
            None,
            None,
            None,
        )
        .unwrap();
        let plugin =
            fs::read_to_string(dir.join(".opencode/plugin/compaction-recovery.js")).unwrap();
        assert!(plugin.contains("run `cat PLAN.md HYPOTHESES.md` to restore"));
        assert!(!plugin.contains("{RECOVERY_CMD}"));
        assert!(plugin.contains("experimental.chat.messages.transform"));
        assert!(plugin.contains("experimental.session.compacting"));
        // Fallback predicate must not depend solely on the unstable metadata marker.
        assert!(plugin.contains("Continue if you have next steps"));
        case.finish();
    }
}
