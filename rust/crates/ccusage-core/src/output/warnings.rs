use std::collections::BTreeSet;

pub fn print_missing_pricing_warnings_for_models<'a>(
    models: impl IntoIterator<Item = &'a str>,
    offline: bool,
) {
    for warning in missing_pricing_warnings_for_models(models, offline) {
        ccusage_terminal::errln!("{warning}");
    }
}

pub(crate) fn print_estimated_pricing_notes_for_models<'a>(
    models: impl IntoIterator<Item = &'a str>,
) {
    for model in models.into_iter().collect::<BTreeSet<_>>() {
        ccusage_terminal::errln!(
            "INFO  Estimated pricing for {model}: its vendor lists it free, but it ran through a paid host with no price of its own; cost uses the closest paid listing."
        );
    }
}

pub(super) fn missing_pricing_warnings_for_models<'a>(
    models: impl IntoIterator<Item = &'a str>,
    offline: bool,
) -> Vec<String> {
    let models = models.into_iter().collect::<BTreeSet<_>>();

    models
        .into_iter()
        .map(|model| {
            if offline {
                format!(
                    "WARN  Missing embedded pricing for {model}; cost excludes this model. Run without --offline or update ccusage after pricing is added."
                )
            } else {
                format!(
                    "WARN  Missing pricing for {model}; cost excludes this model. Update pricing or run again after LiteLLM has the model."
                )
            }
        })
        .collect()
}
