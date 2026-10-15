//! Model limits from the OpenCode model registry (`opencode models --verbose`).

use std::process::Command;
use tracing::info;

use crate::AgentInvocation;
use crate::prompt::{ModelLimits, RegistryLimits};

/// The registry limits of the run's models: one lookup per model, read once
/// per stage. A model left to the CLI default has no entry.
pub(crate) fn load_registry_limits(
    invocation: &AgentInvocation<'_>,
) -> Result<RegistryLimits, Box<dyn std::error::Error>> {
    Ok(RegistryLimits {
        main: invocation
            .model
            .map(load_opencode_model_limits)
            .transpose()?,
        subagent: invocation
            .subagent_model
            .map(load_opencode_model_limits)
            .transpose()?,
    })
}

/// OPENCODE_EXPERIMENTAL_OUTPUT_TOKEN_MAX (opencode#29363 workaround).
/// OpenCode caps each model at `min(limit.output, cap)`, and a model with
/// no output limit at the cap itself. So the cap is the larger limit, set
/// only when every model has one; else `None` keeps the 32k default.
pub(super) fn output_cap(limits: &RegistryLimits) -> Option<u64> {
    let main = limits.main.as_ref()?.output?;
    match &limits.subagent {
        None => Some(main),
        Some(subagent) => Some(main.max(subagent.output?)),
    }
}

fn load_opencode_model_limits(model: &str) -> Result<ModelLimits, Box<dyn std::error::Error>> {
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

pub(super) fn parse_opencode_model(
    model: &str,
) -> Result<(String, String), Box<dyn std::error::Error>> {
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

pub(super) fn run_opencode_models(
    provider: Option<&str>,
) -> Result<String, Box<dyn std::error::Error>> {
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
) -> Option<ModelLimits> {
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
                            let output = registry_output_limit(limit);
                            return Some(ModelLimits { context, output });
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
            let output = registry_output_limit(limit);
            return Some(ModelLimits { context, output });
        }
    }

    None
}

/// The `limit.output` of a registry entry. OpenCode stores a missing output
/// limit as 0, so 0 means none.
fn registry_output_limit(limit: &serde_json::Value) -> Option<u64> {
    limit
        .get("output")
        .and_then(|v| v.as_u64())
        .filter(|&output| output > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits(context: u64, output: Option<u64>) -> ModelLimits {
        ModelLimits { context, output }
    }

    #[test]
    fn output_cap_needs_the_output_limit_of_every_model() {
        let cap = |main, subagent| output_cap(&RegistryLimits { main, subagent });
        assert_eq!(cap(Some(limits(1, Some(128_000))), None), Some(128_000));
        // The larger limit; OpenCode's min() keeps each model at its own.
        assert_eq!(
            cap(Some(limits(1, Some(32_000))), Some(limits(1, Some(65_536)))),
            Some(65_536)
        );
        // A model without an output limit would get the cap itself.
        assert_eq!(
            cap(Some(limits(1, Some(65_536))), Some(limits(1, None))),
            None
        );
        assert_eq!(
            cap(Some(limits(1, None)), Some(limits(1, Some(65_536)))),
            None
        );
        // The main model left to the CLI default has no registry entry.
        assert_eq!(cap(None, Some(limits(1, Some(65_536)))), None);
        assert_eq!(cap(None, None), None);
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
    fn registry_output_limit_zero_means_none() {
        let sample = r#"{"id": "m", "providerID": "p", "limit": {"context": 8192, "output": 0}}"#;
        assert_eq!(
            extract_model_limits_from_output(sample, "p", "m"),
            Some(limits(8192, None))
        );
    }
}
