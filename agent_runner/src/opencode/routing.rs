//! OpenRouter provider routing for `openrouter/...` models.
//!
//! OpenRouter serves a model through several upstream providers. A model
//! without a routing suffix uses OpenRouter's default routing. A model written
//! as `openrouter/<model>#<slug>[,<slug>...]` sends its requests only to the
//! listed providers (OpenRouter's `provider.only`): OpenRouter's default
//! routing selects among them, and a request that none of them can serve
//! fails. A slug is a provider (`deepinfra`) or one of its endpoints
//! (`deepinfra/fp8`), as the model's endpoint list on OpenRouter shows it.

const OPENROUTER_PREFIX: &str = "openrouter/";

/// The providers that serve the requests of one OpenRouter model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Route<'a> {
    /// The OpenRouter model id: the OpenCode model id without `openrouter/`.
    pub model_id: &'a str,
    /// The provider slugs, in the order given.
    pub only: Vec<&'a str>,
}

impl Route<'_> {
    /// OpenRouter's `provider` request field for this route: OpenCode sends a
    /// model's `options` with each request of the model.
    pub(crate) fn provider(&self) -> serde_json::Value {
        serde_json::json!({ "only": self.only })
    }
}

/// Removes the routing suffix from the main model and the sub-agent model, so
/// OpenCode and the model registry get plain model ids, and returns the routes.
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

/// Splits `model` into the OpenCode model id and its route, if it has one.
fn split_route(model: &str) -> Result<(&str, Option<Route<'_>>), String> {
    let Some((id, suffix)) = model.split_once('#') else {
        return Ok((model, None));
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
    Ok((id, Some(Route { model_id, only })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_route_parses_the_suffix() {
        assert_eq!(
            split_route("openrouter/deepseek/deepseek-v4-pro"),
            Ok(("openrouter/deepseek/deepseek-v4-pro", None))
        );
        assert_eq!(
            split_route("openrouter/deepseek/deepseek-v4-pro#deepinfra/fp8,fireworks"),
            Ok((
                "openrouter/deepseek/deepseek-v4-pro",
                Some(Route {
                    model_id: "deepseek/deepseek-v4-pro",
                    only: vec!["deepinfra/fp8", "fireworks"],
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
        // No suffix: no route, so OpenRouter's default routing.
        assert_eq!(
            take(Some("openrouter/a/b"), Some("opencode-go/c")),
            Ok((Some("openrouter/a/b"), Some("opencode-go/c"), 0))
        );
        assert_eq!(take(None, None), Ok((None, None, 0)));
    }
}
