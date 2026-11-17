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
    pub(crate) value: Value,
    pub(crate) layers: BTreeMap<String, Layer>,
    pub(crate) warnings: Vec<String>,
    pub(crate) pi_stores: Vec<ccusage_cli::NamedPiStore>,
}

impl ConfigFile {
    pub(crate) fn parse(value: Value) -> Result<Self, String> {
        let root = object(&value, "$")?;
        let mut layers = BTreeMap::new();
        let mut warnings = Vec::new();
        let schema = layer_schema()?;
        for (key, value) in root {
            match key.as_str() {
                "$schema" => {
                    decode::<String>(value, "$schema")?;
                }
                "defaults" if !value.is_null() => {
                    layers.insert(key.clone(), layer(value, key, &schema, &mut warnings)?);
                }
                "commands" if !value.is_null() => {
                    commands(value, key, &schema, &mut layers, &mut warnings)?;
                }
                agent if ccusage_cli::BUILT_IN_AGENT_NAMES.contains(&agent) && !value.is_null() => {
                    for (name, value) in object(value, key)? {
                        let path = format!("{key}.{name}");
                        match name.as_str() {
                            "defaults" if !value.is_null() => {
                                layers.insert(
                                    path.clone(),
                                    layer(value, &path, &schema, &mut warnings)?,
                                );
                            }
                            "commands" if !value.is_null() => {
                                commands(value, &path, &schema, &mut layers, &mut warnings)?;
                            }
                            "stores" if agent == "pi" && !value.is_null() => {
                                decode::<Vec<PiStoreConfig>>(value, &path)?;
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
            value,
            layers,
            warnings,
            pi_stores: Vec::new(),
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
    layers: &mut BTreeMap<String, Layer>,
    warnings: &mut Vec<String>,
) -> Result<(), String> {
    for (name, value) in object(value, path)? {
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
            layers.insert(path.clone(), layer(value, &path, schema, warnings)?);
        }
    }
    Ok(())
}

fn layer(
    value: &Value,
    path: &str,
    schema: &Value,
    warnings: &mut Vec<String>,
) -> Result<Layer, String> {
    object(value, path)?;
    let layer = Layer {
        shared: decode(value, path)?,
        daily: decode(value, path)?,
        weekly: decode(value, path)?,
        blocks: decode(value, path)?,
        statusline: decode(value, path)?,
        codex: decode(value, path)?,
        pi: decode(value, path)?,
        openclaw: decode(value, path)?,
    };
    unknown_fields(value, schema, schema, path, warnings);
    Ok(layer)
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
        for (key, child) in fields {
            let wrap = |child: Value| Value::Object(Map::from_iter([(key.clone(), child)]));
            if rejects(&wrap(child.clone())) {
                return failing_path(child, &format!("{path}.{key}"), &|child| {
                    rejects(&wrap(child.clone()))
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
