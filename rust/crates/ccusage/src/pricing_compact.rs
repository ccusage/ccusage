/// Projects litellm pricing JSON to only the fields the parser reads.
///
/// When `keep_all` is false, only models matching [`is_embedded_model`] are
/// retained (for the build-time embedded blob). When `keep_all` is true, all
/// models are kept (for runtime cache projection).
///
/// # Returns
/// `Some(projected_json)` on success, `None` if the input is not a valid JSON
/// object.
fn compact_litellm(json: &str, keep_all: bool) -> Option<String> {
    let serde_json::Value::Object(raw) = serde_json::from_str::<serde_json::Value>(json).ok()?
    else {
        return None;
    };
    let mut compact = serde_json::Map::new();
    for (model, pricing) in raw {
        if !keep_all && !is_embedded_model(&model) {
            continue;
        }
        let serde_json::Value::Object(pricing) = pricing else {
            continue;
        };
        let mut fields = serde_json::Map::new();
        for (source, target) in [
            ("input_cost_per_token", "i"),
            ("output_cost_per_token", "o"),
            ("cache_creation_input_token_cost", "cc"),
            ("cache_read_input_token_cost", "cr"),
            ("input_cost_per_token_above_200k_tokens", "ia"),
            ("output_cost_per_token_above_200k_tokens", "oa"),
            ("cache_creation_input_token_cost_above_200k_tokens", "cca"),
            ("cache_read_input_token_cost_above_200k_tokens", "cra"),
            ("max_input_tokens", "ctx"),
        ] {
            let Some(value) = pricing.get(source) else {
                continue;
            };
            if !value.is_null() {
                fields.insert(target.to_string(), value.clone());
            }
        }
        if let Some(fast) = pricing
            .get("provider_specific_entry")
            .and_then(serde_json::Value::as_object)
            .and_then(|entry| entry.get("fast"))
            .filter(|value| !value.is_null())
        {
            fields.insert("fast".to_string(), fast.clone());
        }
        if fields.contains_key("i") && fields.contains_key("o") {
            compact.insert(model, serde_json::Value::Object(fields));
        }
    }
    serde_json::to_string(&serde_json::Value::Object(compact)).ok()
}

/// Whether the model should be included in the build-time embedded blob.
///
/// The embedded blob is deliberately smaller than the full litellm dataset — it
/// only contains models that the build-time snapshot historically covered. The
/// runtime projector (`keep_all = true`) does **not** call this filter.
fn is_embedded_model(model: &str) -> bool {
    model.starts_with("claude-")
        || model.starts_with("anthropic.")
        || model.starts_with("anthropic/")
        || model.starts_with("us.anthropic.")
        || model.starts_with("eu.anthropic.")
        || model.starts_with("global.anthropic.")
        || model.starts_with("jp.anthropic.")
        || model.starts_with("au.anthropic.")
        || model.starts_with("gpt-")
        || model.starts_with("openai/")
        || model.starts_with("azure/")
        || model.starts_with("zai/")
        || model.starts_with("openrouter/openai/")
}
