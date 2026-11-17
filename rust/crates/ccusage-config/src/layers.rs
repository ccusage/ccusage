use std::collections::BTreeMap;

use schemars::{JsonSchema, schema_for};
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

use crate::config_schema::{
    BlocksSpecificOptions, CodexOptions, DailySpecificOptions, OpenClawOptions, PiOptions,
    PiStoreConfig, SharedOptions, StatuslineSpecificOptions, WeeklySpecificOptions,
};

#[derive(Clone, Debug, Default)]
pub(crate) struct Layer {
    pub(crate) date_error: Option<String>,
    pub(crate) shared: SharedOptions,
    pub(crate) daily: DailySpecificOptions,
    pub(crate) weekly: WeeklySpecificOptions,
    pub(crate) blocks: BlocksSpecificOptions,
    pub(crate) statusline: StatuslineSpecificOptions,
    pub(crate) codex: CodexOptions,
    pub(crate) pi: PiOptions,
    pub(crate) openclaw: OpenClawOptions,
}

pub(crate) struct ConfigFile {
    pub(crate) path: std::path::PathBuf,
    pub(crate) value: Value,
    pub(crate) layers: BTreeMap<String, Layer>,
    pub(crate) warnings: Vec<String>,
    pub(crate) pi_stores: Vec<ccusage_cli::NamedPiStore>,
    pub(crate) pi_stores_error: Option<String>,
}

impl ConfigFile {
    pub(crate) fn parse(value: Value) -> Result<Self, String> {
        Self::parse_with(value, true)
    }

    pub(crate) fn parse_discovered(value: Value) -> Result<Self, String> {
        Self::parse_with(value, false)
    }

    fn parse_with(value: Value, strict: bool) -> Result<Self, String> {
        let root = object(&value, "$")?;
        let mut layers = BTreeMap::new();
        let mut warnings = Vec::new();
        let schema = layer_schema()?;
        for (key, value) in root {
            match key.as_str() {
                "$schema" => {
                    if let Err(error) = decode::<String>(value, "$schema") {
                        if strict {
                            return Err(error);
                        }
                        warnings.push(format!("Invalid ccusage config field: {error}"));
                    }
                }
                "defaults" if !value.is_null() => {
                    layers.insert(
                        key.clone(),
                        layer(value, key, &schema, strict, &mut warnings)?,
                    );
                }
                "commands" if !value.is_null() => {
                    commands(value, key, &schema, strict, &mut layers, &mut warnings)?;
                }
                agent if ccusage_cli::BUILT_IN_AGENT_NAMES.contains(&agent) && !value.is_null() => {
                    let Some(fields) = option_object(value, key, strict, &mut warnings)? else {
                        continue;
                    };
                    for (name, value) in fields {
                        let path = format!("{key}.{name}");
                        match name.as_str() {
                            "defaults" if !value.is_null() => {
                                layers.insert(
                                    path.clone(),
                                    layer(value, &path, &schema, strict, &mut warnings)?,
                                );
                            }
                            "commands" if !value.is_null() => {
                                commands(
                                    value,
                                    &path,
                                    &schema,
                                    strict,
                                    &mut layers,
                                    &mut warnings,
                                )?;
                            }
                            "stores" if agent == "pi" && !value.is_null() => {
                                if strict {
                                    decode::<Vec<PiStoreConfig>>(value, &path)?;
                                }
                                let store_schema =
                                    serde_json::to_value(schema_for!(Vec<PiStoreConfig>))
                                        .map_err(|error| error.to_string())?;
                                unknown_fields(
                                    value,
                                    &store_schema,
                                    &store_schema,
                                    &path,
                                    &mut warnings,
                                );
                            }
                            "defaults" | "commands" | "stores" if value.is_null() => {}
                            _ => warnings.push(format!("Unknown ccusage config key: {path}")),
                        }
                    }
                }
                "defaults" | "commands" => {}
                agent if value.is_null() && ccusage_cli::BUILT_IN_AGENT_NAMES.contains(&agent) => {}
                _ => warnings.push(format!("Unknown ccusage config key: {key}")),
            }
        }
        Ok(Self {
            path: std::path::PathBuf::new(),
            value,
            layers,
            warnings,
            pi_stores: Vec::new(),
            pi_stores_error: None,
        })
    }
}

fn object<'a>(value: &'a Value, path: &str) -> Result<&'a Map<String, Value>, String> {
    value
        .as_object()
        .ok_or_else(|| format!("{path}: expected an object"))
}

fn commands(
    value: &Value,
    path: &str,
    schema: &Value,
    strict: bool,
    layers: &mut BTreeMap<String, Layer>,
    warnings: &mut Vec<String>,
) -> Result<(), String> {
    let Some(fields) = option_object(value, path, strict, warnings)? else {
        return Ok(());
    };
    for (name, value) in fields {
        let path = format!("{path}.{name}");
        let known = matches!(
            name.as_str(),
            "daily" | "weekly" | "monthly" | "session" | "blocks" | "statusline"
        ) || name.split_once([' ', ':']).is_some_and(|(agent, report)| {
            ccusage_cli::AgentId::from_name(agent).is_some_and(|agent| {
                ccusage_cli::AgentReportKind::from_name(report).is_some()
                    || agent.spec().accepts(report)
            })
        });
        if !known {
            warnings.push(format!("Unknown ccusage config key: {path}"));
        } else if !value.is_null() {
            layers.insert(path.clone(), layer(value, &path, schema, strict, warnings)?);
        }
    }
    Ok(())
}

fn layer(
    value: &Value,
    path: &str,
    schema: &Value,
    strict: bool,
    warnings: &mut Vec<String>,
) -> Result<Layer, String> {
    if option_object(value, path, strict, warnings)?.is_none() {
        return Ok(Layer::default());
    }
    let mut retained = value.clone();
    let mut date_error = None;
    if !strict {
        for field in ["since", "until"] {
            if let Some(value) = retained.get(field)
                && let Err(error) = decode::<Option<crate::config_schema::ConfigDateBound>>(
                    value,
                    &format!("{path}.{field}"),
                )
            {
                if date_error.is_none() {
                    date_error = Some(error.clone());
                }
                warnings.push(format!("Invalid ccusage config field: {error}"));
                if let Some(fields) = retained.as_object_mut() {
                    fields.remove(field);
                }
            }
        }
    }
    let layer = Layer {
        date_error,
        shared: decode_options(&mut retained, path, strict, warnings)?,
        daily: decode_options(&mut retained, path, strict, warnings)?,
        weekly: decode_options(&mut retained, path, strict, warnings)?,
        blocks: decode_options(&mut retained, path, strict, warnings)?,
        statusline: decode_options(&mut retained, path, strict, warnings)?,
        codex: decode_options(&mut retained, path, strict, warnings)?,
        pi: decode_options(&mut retained, path, strict, warnings)?,
        openclaw: decode_options(&mut retained, path, strict, warnings)?,
    };
    unknown_fields(value, schema, schema, path, warnings);
    Ok(layer)
}

fn option_object<'a>(
    value: &'a Value,
    path: &str,
    strict: bool,
    warnings: &mut Vec<String>,
) -> Result<Option<&'a Map<String, Value>>, String> {
    match object(value, path) {
        Ok(fields) => Ok(Some(fields)),
        Err(error) if strict => Err(error),
        Err(error) => {
            warnings.push(format!("Invalid ccusage config field: {error}"));
            Ok(None)
        }
    }
}

fn decode_options<T: DeserializeOwned>(
    retained: &mut Value,
    path: &str,
    strict: bool,
    warnings: &mut Vec<String>,
) -> Result<T, String> {
    loop {
        match decode(retained, path) {
            Ok(options) => return Ok(options),
            Err(error) => {
                if strict {
                    return Err(error);
                }
                if !prune_invalid(retained, &|candidate| {
                    serde_json::from_value::<T>(candidate.clone()).is_err()
                }) {
                    return Err(error);
                }
                warnings.push(format!("Invalid ccusage config field: {error}"));
            }
        }
    }
}

fn prune_invalid(value: &mut Value, rejects: &dyn Fn(&Value) -> bool) -> bool {
    let Value::Object(fields) = value else {
        return false;
    };
    let invalid = fields
        .iter()
        .find(|(key, child)| {
            rejects(&Value::Object(Map::from_iter([(
                (*key).clone(),
                (*child).clone(),
            )])))
        })
        .map(|(key, _)| key.clone());
    let Some(key) = invalid else {
        return false;
    };
    let wrap = |child: &Value| Value::Object(Map::from_iter([(key.clone(), child.clone())]));
    if let Some(child) = fields.get_mut(&key)
        && prune_invalid(child, &|child| rejects(&wrap(child)))
    {
        return true;
    }
    fields.remove(&key);
    true
}

pub(crate) fn decode<T: DeserializeOwned>(value: &Value, path: &str) -> Result<T, String> {
    serde_json::from_value(value.clone()).map_err(|error| {
        let path = failing_path(value, path, &|candidate| {
            serde_json::from_value::<T>(candidate.clone()).is_err()
        });
        format!("{path}: {error}")
    })
}

fn failing_path(value: &Value, path: &str, rejects: &dyn Fn(&Value) -> bool) -> String {
    if let Value::Object(fields) = value {
        if rejects(&Value::Object(Map::new())) {
            return path.to_string();
        }
        for (key, child) in fields {
            let wrap = |child: Value| Value::Object(Map::from_iter([(key.clone(), child)]));
            if rejects(&wrap(child.clone())) {
                return failing_path(child, &format!("{path}.{key}"), &|child| {
                    rejects(&wrap(child.clone()))
                });
            }
        }
    }
    if let Value::Array(values) = value {
        for (index, child) in values.iter().enumerate() {
            if rejects(&Value::Array(vec![child.clone()])) {
                return failing_path(child, &format!("{path}[{index}]"), &|child| {
                    rejects(&Value::Array(vec![child.clone()]))
                });
            }
        }
    }
    path.to_string()
}

fn layer_schema() -> Result<Value, String> {
    let mut generator = schemars::r#gen::SchemaGenerator::default();
    let mut properties = serde_json::Map::new();
    macro_rules! add {
        ($($ty:ty),*) => { $(
            let schema = serde_json::to_value(<$ty>::json_schema(&mut generator)).map_err(|error| error.to_string())?;
            let fields = schema.get("properties").and_then(Value::as_object).ok_or_else(|| "option schema has no properties".to_string())?;
            properties.extend(fields.clone());
        )* };
    }
    add!(
        SharedOptions,
        DailySpecificOptions,
        WeeklySpecificOptions,
        BlocksSpecificOptions,
        StatuslineSpecificOptions,
        CodexOptions,
        PiOptions,
        OpenClawOptions
    );
    Ok(serde_json::json!({ "properties": properties, "definitions": generator.definitions() }))
}

fn unknown_fields(
    value: &Value,
    schema: &Value,
    root: &Value,
    path: &str,
    warnings: &mut Vec<String>,
) {
    if let Some(reference) = schema.get("$ref").and_then(Value::as_str)
        && let Some(schema) = reference
            .strip_prefix('#')
            .and_then(|path| root.pointer(path))
    {
        unknown_fields(value, schema, root, path, warnings);
        return;
    }
    if let (Some(fields), Some(properties)) = (
        value.as_object(),
        schema.get("properties").and_then(Value::as_object),
    ) {
        for (key, value) in fields {
            let path = format!("{path}.{key}");
            match properties.get(key) {
                Some(schema) => unknown_fields(value, schema, root, &path, warnings),
                None => warnings.push(format!("Unknown ccusage config key: {path}")),
            }
        }
    } else if let (Some(fields), Some(additional)) = (
        value.as_object(),
        schema
            .get("additionalProperties")
            .filter(|schema| schema.is_object()),
    ) {
        for (key, value) in fields {
            unknown_fields(value, additional, root, &format!("{path}.{key}"), warnings);
        }
    } else if let (Some(values), Some(items)) = (value.as_array(), schema.get("items")) {
        for (index, value) in values.iter().enumerate() {
            unknown_fields(value, items, root, &format!("{path}[{index}]"), warnings);
        }
    }
}
