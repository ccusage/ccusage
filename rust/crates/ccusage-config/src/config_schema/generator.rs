use ccusage_cli::{AGENTS, AgentCommandKind, AgentId, AgentReportKind};
use schemars::{
    JsonSchema,
    r#gen::{SchemaGenerator, SchemaSettings},
};
use serde_json::{Map, Value, json};

use super::{
    BlocksSpecificOptions, ClaudeOptions, CodexOptions, DailySpecificOptions, OpenClawOptions,
    PiOptions, PiStoreConfig, SharedOptions, StatuslineSpecificOptions, WeeklySpecificOptions,
};

pub fn generate_config_schema_json() -> serde_json::Result<String> {
    let mut generator = SchemaSettings::draft07()
        .with(|settings| settings.option_add_null_type = false)
        .into_generator();
    let mut shared = option_properties::<SharedOptions>(&mut generator)?;
    shared.insert("all".to_string(), legacy_all_schema());
    let daily = option_properties::<DailySpecificOptions>(&mut generator)?;
    let weekly = option_properties::<WeeklySpecificOptions>(&mut generator)?;
    let blocks = option_properties::<BlocksSpecificOptions>(&mut generator)?;
    let statusline = option_properties::<StatuslineSpecificOptions>(&mut generator)?;
    let claude = option_properties::<ClaudeOptions>(&mut generator)?;
    let codex = option_properties::<CodexOptions>(&mut generator)?;
    let pi = option_properties::<PiOptions>(&mut generator)?;
    let openclaw = option_properties::<OpenClawOptions>(&mut generator)?;
    let pi_stores = serde_json::to_value(Vec::<PiStoreConfig>::json_schema(&mut generator))?;

    let default_values = [
        ("json", json!(false)),
        ("mode", json!("auto")),
        ("debug", json!(false)),
        ("debugSamples", json!(5)),
        ("order", json!("asc")),
        ("breakdown", json!(false)),
        ("offline", json!(false)),
        ("noOffline", json!(false)),
        ("color", json!(false)),
        ("noColor", json!(false)),
        ("all", json!(false)),
        ("compact", json!(false)),
        ("singleThread", json!(false)),
        ("noCost", json!(false)),
    ];
    let shared_defaults = with_defaults(option_section(&[&shared]), &default_values);
    let claude_defaults = with_defaults(option_section(&[&shared, &claude]), &default_values);
    let daily_section = option_section(&[&shared, &claude, &daily]);
    let blocks_section = with_defaults(
        option_section(&[&shared, &claude, &blocks]),
        &[("sessionLength", json!(5.0))],
    );
    let statusline_section = with_defaults(
        option_section(&[&statusline]),
        &[
            ("noOffline", json!(false)),
            ("visualBurnRate", json!("off")),
            ("costSource", json!("auto")),
            ("cache", json!(true)),
            ("noCache", json!(false)),
            ("refreshInterval", json!(1)),
            ("contextLowThreshold", json!(50)),
            ("contextMediumThreshold", json!(80)),
            ("debug", json!(false)),
        ],
    );
    let codex_section = with_defaults(
        option_section(&[&shared, &codex]),
        &[("speed", json!("auto"))],
    );

    let extra_section = |kind| -> serde_json::Result<Value> {
        Ok(match kind {
            AgentCommandKind::Blocks(_) => blocks_section.clone(),
            AgentCommandKind::ClaudeStatusline {
                offline_by_default, ..
            } => with_defaults(
                statusline_section.clone(),
                &[("offline", json!(offline_by_default))],
            ),
            AgentCommandKind::CodexStatusline {
                offline_by_default, ..
            } => codex_statusline_section(&shared, &codex, offline_by_default)?,
        })
    };

    let mut root_commands = AgentReportKind::ALL
        .into_iter()
        .map(|report| {
            let section = match report {
                AgentReportKind::Daily => daily_section.clone(),
                AgentReportKind::Weekly => with_defaults(
                    option_section(&[&shared, &claude, &weekly]),
                    &[("startOfWeek", json!("monday"))],
                ),
                AgentReportKind::Monthly | AgentReportKind::Session => claude_defaults.clone(),
            };
            (report.name().to_string(), section)
        })
        .collect::<Map<_, _>>();
    for command in AgentId::Claude.spec().extra_commands {
        root_commands.insert(command.name.to_string(), extra_section(command.kind)?);
    }

    let mut properties = Map::from_iter([
        (
            "$schema".to_string(),
            json!({
                "description": "JSON Schema URL for validation and autocomplete.", "type": "string"
            }),
        ),
        (
            "defaults".to_string(),
            described(
                with_options(claude_defaults.clone(), &weekly),
                "Default values for all-agent reports and legacy Claude commands.",
            ),
        ),
        (
            "commands".to_string(),
            described(
                object_schema(root_commands),
                "Command-specific configuration for all-agent reports.",
            ),
        ),
    ]);
    for agent in AGENTS {
        let defaults = match agent.name {
            "claude" => option_section(&[&shared, &claude, &daily, &weekly, &blocks, &statusline]),
            "codex" => codex_section.clone(),
            "pi" => option_section(&[&shared, &pi]),
            "openclaw" => option_section(&[&shared, &openclaw]),
            _ => shared_defaults.clone(),
        };
        let mut commands = AgentReportKind::ALL
            .into_iter()
            .map(|report| {
                let section = match (agent.name, report) {
                    ("claude", AgentReportKind::Daily) => daily_section.clone(),
                    ("claude", AgentReportKind::Weekly) => with_defaults(
                        option_section(&[&shared, &claude, &weekly]),
                        &[("startOfWeek", json!("sunday"))],
                    ),
                    ("claude", _) => claude_defaults.clone(),
                    (_, AgentReportKind::Weekly) => with_defaults(
                        with_options(defaults.clone(), &weekly),
                        &[("startOfWeek", json!("monday"))],
                    ),
                    _ => defaults.clone(),
                };
                (report.name().to_string(), section)
            })
            .collect::<Map<_, _>>();
        for command in agent.extra_commands {
            commands.insert(command.name.to_string(), extra_section(command.kind)?);
        }
        let defaults = if agent.name != "claude" {
            with_options(defaults, &weekly)
        } else {
            defaults
        };
        let mut agent_properties = Map::from_iter([
            ("defaults".to_string(), defaults),
            ("commands".to_string(), object_schema(commands)),
        ]);
        if agent.name == "pi" {
            agent_properties.insert(
                "stores".to_string(),
                described(
                    pi_stores.clone(),
                    "Additional named pi-format session stores included in all-agent reports.",
                ),
            );
        }
        let label = match agent.name {
            "copilot" => "GitHub Copilot CLI",
            "hermes" => "Hermes Agent",
            "grok" => "Grok Build CLI",
            "gemini" => "Gemini CLI",
            _ => agent.label,
        };
        properties.insert(
            agent.name.to_string(),
            described(
                object_schema(agent_properties),
                &format!("{label} configuration."),
            ),
        );
    }

    let mut schema = object_schema(properties);
    schema["$schema"] = json!("https://json-schema.org/draft-07/schema#");
    schema["title"] = json!("ccusage Configuration");
    schema["description"] = json!("Configuration file for ccusage");
    schema["definitions"] = serde_json::to_value(generator.definitions())?;
    enrich_schema(&mut schema);
    inline_schema_references(&mut schema);
    wrap_root_schema(&mut schema);
    add_agent_command_references(&mut schema);
    schema["examples"] = json!([example_config()]);
    let mut json = tab_indent_json(&serde_json::to_string_pretty(&schema)?);
    json.push('\n');
    Ok(json)
}

pub(super) fn option_properties<T: JsonSchema>(
    generator: &mut SchemaGenerator,
) -> serde_json::Result<Map<String, Value>> {
    let mut schema = T::json_schema(generator).into_object();
    std::mem::take(&mut schema.object().properties)
        .into_iter()
        .map(|(key, schema)| Ok((key, serde_json::to_value(schema)?)))
        .collect()
}

fn object_schema(properties: Map<String, Value>) -> Value {
    json!({ "type": "object", "properties": properties })
}

fn option_section(sections: &[&Map<String, Value>]) -> Value {
    object_schema(
        sections
            .iter()
            .flat_map(|section| section.iter())
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
    )
}

fn described(mut schema: Value, description: &str) -> Value {
    schema["description"] = json!(description);
    schema
}

fn with_options(mut section: Value, options: &Map<String, Value>) -> Value {
    section["properties"]
        .as_object_mut()
        .expect("option section has properties")
        .extend(options.clone());
    section
}

fn with_defaults(mut schema: Value, defaults: &[(&str, Value)]) -> Value {
    for (key, default) in defaults {
        schema["properties"][key]["default"] = default.clone();
    }
    schema
}

pub(super) fn codex_statusline_section(
    shared: &Map<String, Value>,
    codex: &Map<String, Value>,
    offline_by_default: bool,
) -> serde_json::Result<Value> {
    let properties = [
        "json",
        "jq",
        "mode",
        "noCost",
        "since",
        "until",
        "timezone",
        "offline",
        "noOffline",
        "singleThread",
        "pricingOverrides",
    ]
    .into_iter()
    .map(|key| (key, shared))
    .chain([("speed", codex)])
    .map(|(key, options)| {
        let mut schema = options
            .get(key)
            .and_then(Value::as_object)
            .cloned()
            .ok_or_else(|| {
                <serde_json::Error as serde::de::Error>::custom(format!(
                    "missing object schema for Codex statusline option '{key}'"
                ))
            })?;
        schema.remove("description");
        if key == "offline" {
            schema.insert("default".to_string(), json!(offline_by_default));
        }
        Ok((key.to_string(), Value::Object(schema)))
    })
    .collect::<serde_json::Result<_>>()?;
    Ok(object_schema(properties))
}

fn legacy_all_schema() -> Value {
    json!({
        "description": "Accepted for compatibility; all detected supported agents are included by default.",
        "type": "boolean",
    })
}

fn example_config() -> Value {
    json!({
        "$schema": "https://ccusage.com/config-schema.json",
        "defaults": {
            "json": false,
            "timezone": "Asia/Tokyo",
            "pricingOverrides": {
                "[pi] gpt-5.4": {
                    "inputCostPerToken": 0.0000025,
                    "outputCostPerToken": 0.000015,
                    "cacheReadInputTokenCost": 0.00000025
                }
            }
        },
        "claude": {
            "defaults": {
                "mode": "auto"
            },
            "commands": {
                "daily": {
                    "instances": true
                },
                "blocks": {
                    "tokenLimit": "500000"
                }
            }
        },
        "codex": {
            "defaults": {
                "speed": "auto"
            }
        },
        "pi": {
            "stores": [
                {
                    "name": "omp",
                    "path": "~/.omp/agent/sessions"
                }
            ]
        },
        "gemini": {
            "defaults": {
                "offline": true
            }
        }
    })
}

fn tab_indent_json(json: &str) -> String {
    json.lines()
        .map(|line| {
            let spaces = line
                .as_bytes()
                .iter()
                .take_while(|byte| **byte == b' ')
                .count();
            let mut formatted = "\t".repeat(spaces / 2);
            formatted.push_str(&line[spaces..]);
            formatted
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn enrich_schema(value: &mut Value) {
    match value {
        Value::Object(map) => {
            if let Some(description) = map.get("description").cloned() {
                map.entry("markdownDescription".to_string())
                    .or_insert(description);
            }
            if map.contains_key("properties") {
                map.entry("additionalProperties".to_string())
                    .or_insert(Value::Bool(false));
            }
            for child in map.values_mut() {
                enrich_schema(child);
            }
        }
        Value::Array(values) => {
            for child in values {
                enrich_schema(child);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

fn inline_schema_references(schema: &mut Value) {
    let definitions = schema
        .get("definitions")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    inline_schema_value(schema, &definitions);
}

fn inline_schema_value(value: &mut Value, definitions: &Map<String, Value>) {
    match value {
        Value::Object(map) => {
            inline_ref(map, definitions);
            inline_all_of(map, definitions);
            for child in map.values_mut() {
                inline_schema_value(child, definitions);
            }
        }
        Value::Array(values) => {
            for child in values {
                inline_schema_value(child, definitions);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

fn inline_ref(map: &mut Map<String, Value>, definitions: &Map<String, Value>) {
    let Some(reference) = map.remove("$ref") else {
        return;
    };
    let Some(reference) = reference.as_str() else {
        return;
    };
    let Some(definition_name) = reference.strip_prefix("#/definitions/") else {
        return;
    };
    let Some(Value::Object(definition)) = definitions.get(definition_name).cloned() else {
        return;
    };

    let existing = std::mem::take(map);
    for (key, value) in definition {
        map.insert(key, value);
    }
    for (key, value) in existing {
        map.insert(key, value);
    }
}

fn inline_all_of(map: &mut Map<String, Value>, definitions: &Map<String, Value>) {
    let Some(Value::Array(items)) = map.remove("allOf") else {
        return;
    };
    for mut item in items {
        inline_schema_value(&mut item, definitions);
        let Value::Object(item) = item else {
            continue;
        };
        merge_schema_object(map, item);
    }
}

fn merge_schema_object(target: &mut Map<String, Value>, source: Map<String, Value>) {
    for (key, value) in source {
        if key == "properties" {
            let target_properties = target
                .entry(key)
                .or_insert_with(|| Value::Object(Map::new()));
            if let (Some(target), Value::Object(source)) =
                (target_properties.as_object_mut(), value)
            {
                target.extend(source);
            }
            continue;
        }
        target.entry(key).or_insert(value);
    }
}

fn wrap_root_schema(schema: &mut Value) {
    let Value::Object(root) = schema else {
        return;
    };
    root.remove("definitions");
    let mut definitions = Map::new();
    let mut root_definition = Map::new();
    for key in [
        "additionalProperties",
        "description",
        "markdownDescription",
        "properties",
        "type",
    ] {
        if let Some(value) = root.remove(key) {
            root_definition.insert(key.to_string(), value);
        }
    }
    definitions.insert("ccusage-config".to_string(), Value::Object(root_definition));
    root.insert(
        "$ref".to_string(),
        Value::String("#/definitions/ccusage-config".to_string()),
    );
    root.insert("definitions".to_string(), Value::Object(definitions));
}

fn add_agent_command_references(schema: &mut Value) {
    let Some(config) = schema.pointer("/definitions/ccusage-config/properties") else {
        return;
    };
    let keys = config
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(agent, agent_schema)| {
            agent_schema
                .pointer("/properties/commands/properties")
                .and_then(Value::as_object)
                .map(|reports| (agent, reports))
        })
        .flat_map(|(agent, reports)| {
            reports.keys().flat_map(move |report| {
                let reference = json!({
                    "$ref": format!(
                        "#/definitions/ccusage-config/properties/{agent}/properties/commands/properties/{report}"
                    )
                });
                [
                    (format!("{agent} {report}"), reference.clone()),
                    (format!("{agent}:{report}"), reference),
                ]
            })
        })
        .collect::<Vec<_>>();
    if let Some(commands) = schema
        .pointer_mut("/definitions/ccusage-config/properties/commands/properties")
        .and_then(Value::as_object_mut)
    {
        commands.extend(keys);
    }
}
