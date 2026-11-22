use std::collections::BTreeSet;

use serde_json::Value;

use ccusage_cli::NamedPiStore;

use super::config_error;
use crate::config_schema::{NAMED_PI_STORE_NAME_PATTERN, PiStoreConfig};

pub(super) fn parse_named_pi_stores(
    value: &Value,
) -> std::result::Result<Vec<NamedPiStore>, String> {
    let Some(stores_value) = value
        .as_object()
        .and_then(|root| root.get("pi"))
        .and_then(Value::as_object)
        .and_then(|pi| pi.get("stores"))
    else {
        return Ok(Vec::new());
    };
    let stores = crate::layers::decode::<Vec<PiStoreConfig>>(stores_value, "pi.stores")
        .map_err(config_error)?;

    let mut names = BTreeSet::new();
    let mut parsed = Vec::with_capacity(stores.len());
    for (index, store) in stores.iter().enumerate() {
        let name = &store.name;
        let path = &store.path;
        if path.trim().is_empty() {
            return Err(config_error(format!(
                "pi.stores[{index}] ('{name}'): path must be a non-empty string"
            )));
        }
        validate_named_pi_store_name(name).map_err(|error| match error {
            NamedPiStoreNameError::Pattern => config_error(format!(
                "pi.stores[{index}].name must match {NAMED_PI_STORE_NAME_PATTERN}"
            )),
            NamedPiStoreNameError::Reserved => config_error(format!(
                "pi.stores name '{name}' collides with a built-in agent"
            )),
        })?;
        if !names.insert(name.clone()) {
            return Err(config_error(format!("duplicate pi.stores name '{name}'")));
        }
        parsed.push(NamedPiStore {
            name: name.clone(),
            path: path.clone(),
        });
    }
    Ok(parsed)
}

#[derive(Debug, Eq, PartialEq)]
pub(super) enum NamedPiStoreNameError {
    Pattern,
    Reserved,
}

pub(super) fn validate_named_pi_store_name(
    name: &str,
) -> std::result::Result<(), NamedPiStoreNameError> {
    if !matches_named_pi_store_name_pattern(name) {
        return Err(NamedPiStoreNameError::Pattern);
    }
    if reserved_named_pi_store_names().contains(&name) {
        return Err(NamedPiStoreNameError::Reserved);
    }
    Ok(())
}

pub(super) fn reserved_named_pi_store_names() -> Vec<&'static str> {
    std::iter::once("all")
        .chain(ccusage_cli::BUILT_IN_AGENT_NAMES.iter().copied())
        .collect()
}

fn matches_named_pi_store_name_pattern(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_lowercase() {
        return false;
    }
    let rest_len = chars
        .by_ref()
        .try_fold(0usize, |count, ch| {
            (ch.is_ascii_lowercase() || ch.is_ascii_digit() || matches!(ch, '_' | '-'))
                .then_some(count + 1)
        })
        .unwrap_or(usize::MAX);
    rest_len <= 31
}
