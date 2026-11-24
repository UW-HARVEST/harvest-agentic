//! OpenRouter provider routing for `openrouter/...` models.
//!
//! OpenRouter serves a model through several upstream providers. A model
//! without a routing suffix uses OpenRouter's default routing. A model written
//! as `openrouter/<model>#<slug>[,<slug>...]` sends its requests only to the
//! listed providers (OpenRouter's `provider.only`): OpenRouter's default
//! routing selects among them, and a request that none of them can serve
//! fails. A slug is a provider (`deepinfra`) or one of its endpoints
//! (`deepinfra/fp8`), as the model's endpoint list on OpenRouter shows it.
//!
//! All requests of a stage carry one `session_id`, so OpenRouter's sticky
//! routing keeps each model on the provider of its first successful request,
//! across the main agent and the sub-agents. OpenCode sends its own session id
//! in a header, which gives each sub-agent a provider of its own; the body
//! field takes precedence over the header. Stickiness is OpenRouter's: it
//! falls back to another provider when the sticky one fails, and it applies
//! only when the provider's cache reads cost less than its prompt tokens.

use std::path::Path;

const OPENROUTER_PREFIX: &str = "openrouter/";

/// The sticky routing key of a stage: unique per run directory.
pub(crate) fn session_id(case_root: &Path) -> String {
    let name = case_root
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_default();
    format!("harvest-{}", name.trim_start_matches('.'))
}

/// The routing of one OpenRouter model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Route<'a> {
    /// The OpenRouter model id: the OpenCode model id without `openrouter/`.
    pub model_id: &'a str,
    /// The provider slugs of the routing suffix, in the order given. None:
    /// OpenRouter's default routing.
    pub only: Option<Vec<&'a str>>,
}

impl Route<'_> {
    /// The model options that set this route, with `session_id` as the sticky
    /// routing key. OpenCode sends a model's `options` in the body of each
    /// request of the model.
    pub(crate) fn options(&self, session_id: &str) -> serde_json::Map<String, serde_json::Value> {
        let mut options = serde_json::Map::new();
        options.insert("session_id".into(), session_id.into());
        if let Some(only) = &self.only {
            options.insert("provider".into(), serde_json::json!({ "only": only }));
        }
        options
    }
}

/// Removes the routing suffix from the main model and the sub-agent model, so
/// OpenCode and the model registry get plain model ids, and returns the routes
/// of the OpenRouter models.
///
/// OpenCode sets provider options per model id, for every agent that uses the
/// model. A main model and a sub-agent model with the same id must thus have
/// the same route. With the same id and the same route, the sub-agents use the
/// main model.
pub(crate) fn take_routes<'a>(
    model: &mut Option<&'a str>,
    subagent_model: &mut Option<&'a str>,
) -> Result<Vec<Route<'a>>, String> {
    let main = model.map(split_route).transpose()?;
    let sub = subagent_model.map(split_route).transpose()?;
    if let (Some((main_id, main_route)), Some((sub_id, sub_route))) = (&main, &sub)
        && main_id == sub_id
        && main_route != sub_route
    {
        return Err(format!(
            "OpenCode sets the OpenRouter route per model, so the main model and the sub-agent \
             model {main_id} must have the same route: {} and {}",
            model.unwrap_or_default(),
            subagent_model.unwrap_or_default(),
        ));
    }
    *model = main.as_ref().map(|(id, _)| *id);
    *subagent_model = sub
        .as_ref()
        .map(|(id, _)| *id)
        .filter(|sub| Some(*sub) != *model);
    let mut routes: Vec<Route<'a>> = Vec::new();
    for route in main.into_iter().chain(sub).filter_map(|(_, route)| route) {
        if !routes.contains(&route) {
            routes.push(route);
        }
    }
    Ok(routes)
}

/// Splits `model` into the OpenCode model id and its route. Only an OpenRouter
/// model has a route.
fn split_route(model: &str) -> Result<(&str, Option<Route<'_>>), String> {
    let Some((id, suffix)) = model.split_once('#') else {
        let route = model
            .strip_prefix(OPENROUTER_PREFIX)
            .filter(|model_id| !model_id.is_empty())
            .map(|model_id| Route {
                model_id,
                only: None,
            });
        return Ok((model, route));
    };
    let model_id = id
        .strip_prefix(OPENROUTER_PREFIX)
        .filter(|model_id| !model_id.is_empty())
        .ok_or_else(|| {
            format!(
                "a routing suffix (`#<provider>,...`) applies only to `openrouter/<model>` \
                 models, got: {model}"
            )
        })?;
    let only: Vec<&str> = suffix.split(',').collect();
    if only
        .iter()
        .any(|slug| slug.is_empty() || slug.contains(|c: char| c.is_whitespace() || c == '#'))
    {
        return Err(format!(
            "the routing suffix of {model} must be a comma-separated list of OpenRouter provider \
             slugs, for example `#deepinfra/fp8,fireworks`"
        ));
    }
    Ok((
        id,
        Some(Route {
            model_id,
            only: Some(only),
        }),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_route_parses_the_suffix() {
        assert_eq!(
            split_route("openrouter/deepseek/deepseek-v4-pro"),
            Ok((
                "openrouter/deepseek/deepseek-v4-pro",
                Some(Route {
                    model_id: "deepseek/deepseek-v4-pro",
                    only: None,
                })
            ))
        );
        // Not an OpenRouter model: no route.
        assert_eq!(
            split_route("opencode-go/mimo-v2.5"),
            Ok(("opencode-go/mimo-v2.5", None))
        );
        assert_eq!(
            split_route("openrouter/deepseek/deepseek-v4-pro#deepinfra/fp8,fireworks"),
            Ok((
                "openrouter/deepseek/deepseek-v4-pro",
                Some(Route {
                    model_id: "deepseek/deepseek-v4-pro",
                    only: Some(vec!["deepinfra/fp8", "fireworks"]),
                })
            ))
        );
        // OpenRouter's own `:` variants stay in the model id.
        assert_eq!(
            split_route("openrouter/a/b:free#chutes")
                .unwrap()
                .1
                .unwrap()
                .model_id,
            "a/b:free"
        );
    }

    #[test]
    fn split_route_rejects_bad_suffixes() {
        for model in [
            "opencode-go/mimo-v2.5#xiaomi",
            "openrouter/#deepinfra",
            "openrouter/a/b#",
            "openrouter/a/b#deepinfra,",
            "openrouter/a/b#deep infra",
            "openrouter/a/b#x#y",
        ] {
            assert!(split_route(model).is_err(), "{model}");
        }
    }

    /// The models and the number of routes after [`take_routes`].
    fn take<'a>(
        mut model: Option<&'a str>,
        mut sub: Option<&'a str>,
    ) -> Result<(Option<&'a str>, Option<&'a str>, usize), String> {
        let routes = take_routes(&mut model, &mut sub)?;
        Ok((model, sub, routes.len()))
    }

    #[test]
    fn session_id_names_the_run_directory() {
        assert_eq!(
            session_id(Path::new("/var/tmp/.tmpi90pgi")),
            "harvest-tmpi90pgi"
        );
    }

    #[test]
    fn options_carry_the_session_and_the_providers() {
        let default = Route {
            model_id: "a/b",
            only: None,
        };
        let json = |route: &Route<'_>| serde_json::Value::Object(route.options("s"));
        assert_eq!(json(&default), serde_json::json!({ "session_id": "s" }));
        let only = Route {
            model_id: "a/b",
            only: Some(vec!["x", "y/fp8"]),
        };
        assert_eq!(
            json(&only),
            serde_json::json!({ "session_id": "s", "provider": { "only": ["x", "y/fp8"] } })
        );
    }

    #[test]
    fn take_routes_strips_suffixes_and_checks_shared_ids() {
        assert_eq!(
            take(Some("openrouter/a/b#x"), Some("openrouter/c/d#y")),
            Ok((Some("openrouter/a/b"), Some("openrouter/c/d"), 2))
        );
        // Same id, same route: the sub-agents use the main model.
        assert_eq!(
            take(Some("openrouter/a/b#x"), Some("openrouter/a/b#x")),
            Ok((Some("openrouter/a/b"), None, 1))
        );
        // Same id, different routes: OpenCode cannot express it.
        assert!(take(Some("openrouter/a/b#x"), Some("openrouter/a/b#y")).is_err());
        assert!(take(Some("openrouter/a/b#x"), Some("openrouter/a/b")).is_err());
        // No suffix: a route with OpenRouter's default routing; other providers: no route.
        assert_eq!(
            take(Some("openrouter/a/b"), Some("opencode-go/c")),
            Ok((Some("openrouter/a/b"), Some("opencode-go/c"), 1))
        );
        assert_eq!(take(None, None), Ok((None, None, 0)));
    }
}
