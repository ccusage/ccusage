use std::{collections::BTreeMap, fmt, sync::LazyLock};

use serde::{
    Deserializer,
    de::{DeserializeOwned, Visitor},
    forward_to_deserialize_any,
};
use serde_json::{Map, Value};

use crate::config_schema::{
    BlocksSpecificOptions, ClaudeOptions, CodexOptions, ConfigPricingOverride,
    DailySpecificOptions, OpenClawOptions, PiOptions, PiStoreConfig, SharedOptions,
    StatuslineSpecificOptions, WeeklySpecificOptions,
};

#[derive(Clone, Debug, Default)]
pub(crate) struct Layer {
    pub(crate) date_error: Option<String>,
    pub(crate) shared: SharedOptions,
    pub(crate) daily: DailySpecificOptions,
    pub(crate) weekly: WeeklySpecificOptions,
    pub(crate) blocks: BlocksSpecificOptions,
    pub(crate) statusline: StatuslineSpecificOptions,
    pub(crate) claude: ClaudeOptions,
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
                    layers.insert(key.clone(), layer(value, key, strict, &mut warnings)?);
                }
                "commands" if !value.is_null() => {
                    commands(value, key, strict, &mut layers, &mut warnings)?;
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
                                    layer(value, &path, strict, &mut warnings)?,
                                );
                            }
                            "commands" if !value.is_null() => {
                                commands(value, &path, strict, &mut layers, &mut warnings)?;
                            }
                            "stores" if agent == "pi" && !value.is_null() => {
                                if strict {
                                    decode::<Vec<PiStoreConfig>>(value, &path)?;
                                }
                                if let Some(stores) = value.as_array() {
                                    let known = struct_fields::<PiStoreConfig>();
                                    for (index, store) in stores.iter().enumerate() {
                                        warn_unknown_fields(
                                            store,
                                            known,
                                            &format!("{path}[{index}]"),
                                            &mut warnings,
                                        );
                                    }
                                }
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
            layers.insert(path.clone(), layer(value, &path, strict, warnings)?);
        }
    }
    Ok(())
}

fn layer(
    value: &Value,
    path: &str,
    strict: bool,
    warnings: &mut Vec<String>,
) -> Result<Layer, String> {
    let Some(fields) = option_object(value, path, strict, warnings)? else {
        return Ok(Layer::default());
    };
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
    if let Some(all) = retained.get("all")
        && let Err(error) = decode::<Option<bool>>(all, &format!("{path}.all"))
    {
        if strict {
            return Err(error);
        }
        warnings.push(format!("Invalid ccusage config field: {error}"));
    }
    let layer = Layer {
        date_error,
        shared: decode_options(&mut retained, path, strict, warnings)?,
        daily: decode_options(&mut retained, path, strict, warnings)?,
        weekly: decode_options(&mut retained, path, strict, warnings)?,
        blocks: decode_options(&mut retained, path, strict, warnings)?,
        statusline: decode_options(&mut retained, path, strict, warnings)?,
        claude: decode_options(&mut retained, path, strict, warnings)?,
        codex: decode_options(&mut retained, path, strict, warnings)?,
        pi: decode_options(&mut retained, path, strict, warnings)?,
        openclaw: decode_options(&mut retained, path, strict, warnings)?,
    };
    warn_unknown_layer_fields(fields, path, warnings);
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
    let wrap = |child: &Value| single_field(key.clone(), child.clone());
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

fn single_field(key: String, value: Value) -> Value {
    let mut fields = Map::new();
    fields.insert(key, value);
    Value::Object(fields)
}

fn failing_path(value: &Value, path: &str, rejects: &dyn Fn(&Value) -> bool) -> String {
    if let Value::Object(fields) = value {
        if rejects(&Value::Object(Map::new())) {
            return path.to_string();
        }
        for (key, child) in fields {
            let wrap = |child: Value| single_field(key.clone(), child);
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

fn warn_unknown_layer_fields(fields: &Map<String, Value>, path: &str, warnings: &mut Vec<String>) {
    static LAYER_FIELDS: LazyLock<[&[&str]; 10]> = LazyLock::new(|| {
        [
            struct_fields::<SharedOptions>(),
            struct_fields::<DailySpecificOptions>(),
            struct_fields::<WeeklySpecificOptions>(),
            struct_fields::<BlocksSpecificOptions>(),
            struct_fields::<StatuslineSpecificOptions>(),
            struct_fields::<ClaudeOptions>(),
            struct_fields::<CodexOptions>(),
            struct_fields::<PiOptions>(),
            struct_fields::<OpenClawOptions>(),
            &["all"],
        ]
    });
    for (key, value) in fields {
        if key == "pricingOverrides" {
            if let Some(overrides) = value.as_object() {
                let known = struct_fields::<ConfigPricingOverride>();
                for (model, fields) in overrides {
                    warn_unknown_fields(fields, known, &format!("{path}.{key}.{model}"), warnings);
                }
            }
        } else if !LAYER_FIELDS
            .iter()
            .any(|fields| fields.contains(&key.as_str()))
        {
            warnings.push(format!("Unknown ccusage config key: {path}.{key}"));
        }
    }
}

fn warn_unknown_fields(value: &Value, known: &[&str], path: &str, warnings: &mut Vec<String>) {
    if let Some(fields) = value.as_object() {
        for key in fields.keys().filter(|key| !known.contains(&key.as_str())) {
            warnings.push(format!("Unknown ccusage config key: {path}.{key}"));
        }
    }
}

fn struct_fields<T: DeserializeOwned>() -> &'static [&'static str] {
    match T::deserialize(StructFields) {
        Err(Fields(Some(fields))) => fields,
        _ => unreachable!("config option types must use named fields"),
    }
}

struct StructFields;

#[derive(Debug)]
struct Fields(Option<&'static [&'static str]>);

impl fmt::Display for Fields {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("expected a named config struct")
    }
}

impl std::error::Error for Fields {}

impl serde::de::Error for Fields {
    fn custom<T: fmt::Display>(_: T) -> Self {
        Self(None)
    }
}

impl<'de> Deserializer<'de> for StructFields {
    type Error = Fields;

    fn deserialize_any<V: Visitor<'de>>(self, _: V) -> Result<V::Value, Fields> {
        Err(Fields(None))
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _: &'static str,
        fields: &'static [&'static str],
        _: V,
    ) -> Result<V::Value, Fields> {
        Err(Fields(Some(fields)))
    }

    forward_to_deserialize_any! {
        bool i8 i16 i32 i64 u8 u16 u32 u64 f32 f64 char str string bytes byte_buf
        option unit unit_struct newtype_struct seq tuple tuple_struct map enum identifier ignored_any
    }
}
