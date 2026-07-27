use std::{
    borrow::Cow,
    collections::BTreeMap,
    env,
    sync::{OnceLock, RwLock},
};

const MODEL_ALIASES_ENV: &str = "CCUSAGE_MODEL_ALIASES";

static MODEL_ALIASES: OnceLock<RwLock<BTreeMap<String, String>>> = OnceLock::new();
static TEST_MODEL_ALIASES_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Resolves a model name through `CCUSAGE_MODEL_ALIASES`, including `-fast` variants.
pub fn resolve_model_name(model: &str) -> Cow<'_, str> {
    let aliases = model_aliases();
    let aliases = aliases.read().unwrap_or_else(|error| error.into_inner());
    if let Some(alias) = aliases.get(model).filter(|alias| !alias.is_empty()) {
        return Cow::Owned(alias.clone());
    }
    if let Some(base_model) = model.strip_suffix("-fast")
        && let Some(alias) = aliases.get(base_model).filter(|alias| !alias.is_empty())
    {
        return Cow::Owned(format!("{alias}-fast"));
    }
    Cow::Borrowed(model)
}

fn model_aliases() -> &'static RwLock<BTreeMap<String, String>> {
    MODEL_ALIASES.get_or_init(|| RwLock::new(load_model_aliases_from_env()))
}

fn load_model_aliases_from_env() -> BTreeMap<String, String> {
    env::var(MODEL_ALIASES_ENV)
        .ok()
        .map(|raw| parse_model_aliases(&raw))
        .unwrap_or_default()
}

fn parse_model_aliases(raw: &str) -> BTreeMap<String, String> {
    let trimmed = raw.trim();
    if trimmed.starts_with('{')
        && let Ok(parsed) = serde_json::from_str::<BTreeMap<String, String>>(trimmed)
    {
        return parsed;
    }

    let stripped = trimmed
        .strip_prefix('{')
        .and_then(|inner| inner.strip_suffix('}'))
        .unwrap_or(trimmed);

    stripped
        .split([',', ';', '\n'])
        .filter_map(|pair| {
            let (from, to) = pair.split_once('=')?;
            let from = from.trim();
            let to = to.trim();
            (!from.is_empty() && !to.is_empty()).then(|| (from.to_string(), to.to_string()))
        })
        .collect()
}

#[doc(hidden)]
pub fn set_model_aliases_for_tests<const N: usize>(
    aliases: [(&'static str, &'static str); N],
) -> ModelAliasesGuard {
    let guard = TEST_MODEL_ALIASES_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let lock = model_aliases();
    let mut current = lock.write().unwrap_or_else(|error| error.into_inner());
    let previous = std::mem::replace(
        &mut *current,
        aliases
            .into_iter()
            .map(|(from, to)| (from.to_string(), to.to_string()))
            .collect(),
    );
    ModelAliasesGuard {
        previous,
        _guard: guard,
    }
}

#[doc(hidden)]
pub struct ModelAliasesGuard {
    previous: BTreeMap<String, String>,
    _guard: std::sync::MutexGuard<'static, ()>,
}

impl Drop for ModelAliasesGuard {
    fn drop(&mut self) {
        let lock = model_aliases();
        let mut current = lock.write().unwrap_or_else(|error| error.into_inner());
        *current = std::mem::take(&mut self.previous);
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_model_aliases, resolve_model_name, set_model_aliases_for_tests};

    #[test]
    fn parses_delimited_model_aliases() {
        let aliases = parse_model_aliases(" private-alpha = gpt-5.5, other-alpha=claude-sonnet-4 ");

        assert_eq!(
            aliases.get("private-alpha").map(String::as_str),
            Some("gpt-5.5")
        );
        assert_eq!(
            aliases.get("other-alpha").map(String::as_str),
            Some("claude-sonnet-4")
        );
    }

    #[test]
    fn parses_json_model_aliases() {
        let aliases = parse_model_aliases(r#"{"private-alpha":"gpt-5.5"}"#);

        assert_eq!(
            aliases.get("private-alpha").map(String::as_str),
            Some("gpt-5.5")
        );
    }

    #[test]
    fn falls_back_to_delimited_parsing_when_json_is_malformed() {
        let aliases =
            parse_model_aliases(r#"{private-alpha=gpt-5.5, other-alpha=claude-sonnet-4}"#);

        assert_eq!(
            aliases.get("private-alpha").map(String::as_str),
            Some("gpt-5.5")
        );
        assert_eq!(
            aliases.get("other-alpha").map(String::as_str),
            Some("claude-sonnet-4")
        );
    }

    #[test]
    fn resolves_configured_model_alias() {
        let _aliases = set_model_aliases_for_tests([("private-alpha", "gpt-5.5")]);

        assert_eq!(resolve_model_name("private-alpha"), "gpt-5.5");
        assert_eq!(resolve_model_name("private-alpha-fast"), "gpt-5.5-fast");
        assert_eq!(resolve_model_name("gpt-5"), "gpt-5");
    }
}
