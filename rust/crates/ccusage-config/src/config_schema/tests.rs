use std::collections::BTreeSet;

use schemars::r#gen::SchemaSettings;
use serde_json::{Map, Value, json};

use super::{
    SharedOptions, StatuslineSpecificOptions,
    generator::{codex_statusline_section, generate_config_schema_json, option_properties},
};

#[test]
fn published_example_uses_valid_config_keys() {
    let schema = generated_schema();
    let example = &schema["examples"][0];
    let parsed = crate::layers::ConfigFile::parse(example.clone()).unwrap();
    assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);

    let mut warnings = Vec::new();
    unknown_fields(example, &schema, &schema, "$", &mut warnings);
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[test]
fn schema_accepts_week_starts_at_each_weekly_config_layer() {
    let schema = generated_schema();
    for agent in ccusage_cli::AGENTS
        .iter()
        .filter(|agent| agent.name != "codex")
    {
        let config = json!({
            "defaults": { "startOfWeek": "monday" },
            "commands": {
                "weekly": { "startOfWeek": "tuesday" },
                (format!("{} weekly", agent.name)): { "startOfWeek": "wednesday" },
                (format!("{}:weekly", agent.name)): { "startOfWeek": "thursday" },
            },
            (agent.name): {
                "defaults": { "startOfWeek": "friday" },
                "commands": { "weekly": { "startOfWeek": "saturday" } },
            }
        });
        let parsed = crate::layers::ConfigFile::parse(config.clone()).unwrap();
        assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
        let mut warnings = Vec::new();
        unknown_fields(&config, &schema, &schema, "$", &mut warnings);
        assert!(warnings.is_empty(), "{warnings:?}");
    }
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

#[test]
fn schema_option_sets_expose_expected_keys() {
    let schema = generated_schema();
    let shared = [
        "all",
        "breakdown",
        "color",
        "compact",
        "debug",
        "debugSamples",
        "jq",
        "json",
        "mode",
        "noColor",
        "noCost",
        "noOffline",
        "offline",
        "order",
        "pricingOverrides",
        "since",
        "singleThread",
        "timezone",
        "until",
    ];

    assert_schema_properties(
        &schema,
        &["defaults"],
        &with_keys(&shared, &["project", "startOfWeek"]),
    );
    assert_schema_properties(
        &schema,
        &["commands", "daily"],
        &with_keys(&shared, &["instances", "project", "projectAliases"]),
    );
    assert_schema_properties(
        &schema,
        &["commands", "weekly"],
        &with_keys(&shared, &["project", "startOfWeek"]),
    );
    assert_schema_properties(
        &schema,
        &["commands", "blocks"],
        &with_keys(
            &shared,
            &["active", "project", "recent", "sessionLength", "tokenLimit"],
        ),
    );
    assert_schema_properties(
        &schema,
        &["commands", "statusline"],
        &[
            "cache",
            "contextLowThreshold",
            "contextMediumThreshold",
            "costSource",
            "debug",
            "modelLabelAliases",
            "noCache",
            "noOffline",
            "offline",
            "pricingOverrides",
            "refreshInterval",
            "timezone",
            "visualBurnRate",
        ],
    );
    assert_schema_properties(
        &schema,
        &["codex", "defaults"],
        &with_keys(&shared, &["speed"]),
    );
    assert_schema_properties(
        &schema,
        &["pi", "defaults"],
        &with_keys(&shared, &["piPath", "piAgentDir", "startOfWeek"]),
    );
    assert!(schema_property(&schema, &["pi", "stores"]).is_some());
    assert_schema_properties(
        &schema,
        &["openclaw", "defaults"],
        &with_keys(&shared, &["openClawPath", "startOfWeek"]),
    );
    let weekly_defaults = with_keys(&shared, &["startOfWeek"]);
    assert_schema_properties(&schema, &["grok", "defaults"], &weekly_defaults);
    assert_schema_properties(&schema, &["antigravity", "defaults"], &weekly_defaults);
    assert_schema_properties(&schema, &["zcode", "defaults"], &weekly_defaults);
    assert_schema_properties(&schema, &["cline", "defaults"], &weekly_defaults);
    assert_schema_properties(&schema, &["cline", "commands", "weekly"], &weekly_defaults);
}

#[test]
fn agent_configs_expose_only_supported_option_sets() {
    let schema = generated_schema();

    assert!(schema_property(&schema, &["codex", "defaults", "speed"]).is_some());
    assert!(schema_property(&schema, &["opencode", "defaults", "speed"]).is_none());
    assert!(schema_property(&schema, &["amp", "defaults", "speed"]).is_none());
    assert!(schema_property(&schema, &["droid", "defaults", "speed"]).is_none());
    assert!(schema_property(&schema, &["codebuff", "defaults", "speed"]).is_none());
    assert!(schema_property(&schema, &["pi", "defaults", "piPath"]).is_some());
    assert!(schema_property(&schema, &["pi", "stores"]).is_some());
    assert!(schema_property(&schema, &["goose", "defaults", "piPath"]).is_none());
    assert!(schema_property(&schema, &["openclaw", "defaults", "openClawPath"]).is_some());
    assert!(schema_property(&schema, &["kilo", "defaults", "openClawPath"]).is_none());
    assert!(schema_property(&schema, &["gemini", "defaults", "openClawPath"]).is_none());
    assert!(schema_property(&schema, &["antigravity", "defaults", "openClawPath"]).is_none());
    assert!(schema_property(&schema, &["kimi", "defaults", "openClawPath"]).is_none());
    assert!(schema_property(&schema, &["cline", "commands", "weekly"]).is_some());
    assert!(schema_property(&schema, &["qwen", "defaults", "openClawPath"]).is_none());
    assert!(schema_property(&schema, &["grok", "defaults", "grokPath"]).is_none());
    assert!(schema_property(&schema, &["openclaw", "defaults", "grokPath"]).is_none());
}

#[test]
fn schema_exposes_project_in_every_claude_report_section() {
    let schema = generated_schema();
    assert!(schema_property(&schema, &["claude", "defaults", "project"]).is_some());
    for report in ["daily", "weekly", "monthly", "session", "blocks"] {
        assert!(schema_property(&schema, &["commands", report, "project"]).is_some());
        assert!(schema_property(&schema, &["claude", "commands", report, "project"]).is_some());
    }
    assert!(schema_property(&schema, &["codex", "defaults", "project"]).is_none());
    assert!(schema_property(&schema, &["claude", "commands", "statusline", "project"]).is_none());
}

#[test]
fn generated_schema_does_not_accept_null_config_values() {
    let schema = generate_config_schema_json().unwrap();
    let value = serde_json::from_str::<Value>(&schema).unwrap();

    assert!(!schema.contains("\"null\""));
    let limit = schema_property(&value, &["commands", "blocks", "tokenLimit"]).unwrap();
    let types = limit["anyOf"]
        .as_array()
        .unwrap()
        .iter()
        .map(|branch| branch["type"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(types, ["string", "integer"]);
}

#[test]
fn generated_schema_keeps_legacy_root_definition_shape() {
    let schema = generated_schema();

    assert_eq!(
        schema["$ref"].as_str(),
        Some("#/definitions/ccusage-config")
    );
    assert_eq!(
        schema["definitions"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["ccusage-config"]
    );
    assert_properties(
        &schema,
        "ccusage-config",
        &[
            "$schema",
            "amp",
            "claude",
            "cline",
            "codebuff",
            "codex",
            "commands",
            "copilot",
            "defaults",
            "droid",
            "dsh",
            "gemini",
            "antigravity",
            "goose",
            "grok",
            "hermes",
            "kilo",
            "kimi",
            "opencode",
            "openclaw",
            "pi",
            "qwen",
            "zcode",
        ],
    );
    assert!(
        schema["definitions"]["ccusage-config"]["properties"]["defaults"]["properties"].is_object()
    );
}

#[test]
fn schema_allows_cli_config_file_shape() {
    let schema = generated_schema();
    let config = serde_json::json!({
        "$schema": "https://ccusage.com/config-schema.json",
        "defaults": {
            "json": true,
            "compact": true,
            "timezone": "Asia/Tokyo"
        },
        "commands": {
            "daily": {
                "since": "20260101"
            },
            "weekly": {
                "startOfWeek": "monday"
            }
        },
        "claude": {
            "commands": {
                "weekly": {
                    "startOfWeek": "monday"
                },
                "blocks": {
                    "active": true,
                    "tokenLimit": "500000",
                    "sessionLength": 6
                },
                "statusline": {
                    "visualBurnRate": "emoji-text",
                    "costSource": "both",
                    "refreshInterval": 3
                }
            }
        },
        "codex": {
            "commands": {
                "monthly": {
                    "speed": "standard",
                    "since": "20260101"
                }
            }
        },
        "opencode": {
            "commands": {
                "weekly": {
                    "json": true
                }
            }
        },
        "amp": {
            "commands": {
                "daily": {
                    "breakdown": true
                }
            }
        },
        "droid": {
            "commands": {
                "daily": {
                    "json": true
                }
            }
        },
        "codebuff": {
            "commands": {
                "daily": {
                    "json": true
                }
            }
        },
        "pi": {
            "stores": [
                {
                    "name": "omp",
                    "path": "~/.omp/agent/sessions"
                }
            ],
            "commands": {
                "daily": {
                    "piPath": "/tmp/pi-sessions"
                }
            }
        },
        "goose": {
            "commands": {
                "daily": {
                    "json": true
                }
            }
        },
        "openclaw": {
            "commands": {
                "daily": {
                    "openClawPath": "/tmp/openclaw"
                }
            }
        },
        "hermes": {
            "commands": {
                "daily": {
                    "json": true
                }
            }
        },
        "kilo": {
            "commands": {
                "daily": {
                    "json": true
                }
            }
        },
        "copilot": {
            "commands": {
                "session": {
                    "json": true
                }
            }
        },
        "gemini": {
            "commands": {
                "session": {
                    "json": true
                }
            }
        },
        "kimi": {
            "commands": {
                "session": {
                    "json": true
                }
            }
        },
        "qwen": {
            "commands": {
                "session": {
                    "json": true
                }
            }
        },
        "cline": {
            "commands": {
                "weekly": {
                    "json": true
                }
            }
        }
    });

    assert_value_keys_allowed_by_schema(&config, &schema, &schema);
}

#[test]
fn schema_allows_agent_command_keys_with_the_agent_report_options() {
    let schema = generated_schema();
    let config = serde_json::json!({
        "commands": {
            "daily": { "since": "20260101" },
            "codex daily": { "speed": "fast" },
            "codex:monthly": { "speed": "standard" },
            "codex weekly": { "speed": "fast" },
            "codex:weekly": { "mode": "display" },
            "claude blocks": { "tokenLimit": "500000" },
            "claude:statusline": { "refreshInterval": 3 }
        },
        "codex": { "commands": { "weekly": { "speed": "fast" } } }
    });
    let commands = schema
        .pointer("/definitions/ccusage-config/properties/commands/properties")
        .unwrap();
    let source_keys = ccusage_cli::BUILT_IN_AGENT_NAMES.iter().flat_map(|agent| {
        ["daily", "weekly", "monthly", "session"]
            .into_iter()
            .flat_map(move |report| [format!("{agent} {report}"), format!("{agent}:{report}")])
    });

    assert_value_keys_allowed_by_schema(&config, &schema, &schema);
    for key in source_keys {
        assert!(commands.get(&key).is_some(), "{key}");
    }
    for key in ["codex:blocks", "amp statusline", "unknown daily"] {
        assert!(commands.get(key).is_none(), "{key}");
    }
}

#[test]
fn schema_allows_repository_example_config() {
    let schema = generated_schema();
    let config =
        serde_json::from_str::<Value>(include_str!("../../../../../ccusage.example.json")).unwrap();

    assert_value_keys_allowed_by_schema(&config, &schema, &schema);
}

#[test]
fn generated_schema_exposes_cli_defaults() {
    let schema = generated_schema();

    assert_eq!(
        property_default(&schema, &["defaults", "json"]),
        Some(&json!(false))
    );
    assert_eq!(
        property_default(&schema, &["defaults", "mode"]),
        Some(&json!("auto"))
    );
    assert_eq!(
        property_default(&schema, &["defaults", "debugSamples"]),
        Some(&json!(5))
    );
    assert_eq!(
        property_default(&schema, &["defaults", "order"]),
        Some(&json!("asc"))
    );
    assert_eq!(
        property_default(&schema, &["commands", "weekly", "startOfWeek"]),
        Some(&json!("monday"))
    );
    assert_eq!(
        property_default(&schema, &["claude", "commands", "weekly", "startOfWeek"]),
        Some(&json!("sunday"))
    );
    assert_eq!(
        property_default(&schema, &["commands", "blocks", "sessionLength"]),
        Some(&json!(5.0))
    );
    assert_eq!(
        property_default(&schema, &["commands", "statusline", "offline"]),
        Some(&json!(true))
    );
    assert_eq!(
        property_default(&schema, &["commands", "statusline", "visualBurnRate"]),
        Some(&json!("off"))
    );
    assert_eq!(
        property_default(&schema, &["commands", "statusline", "refreshInterval"]),
        Some(&json!(1))
    );
    assert_eq!(
        property_default(&schema, &["codex", "defaults", "speed"]),
        Some(&json!("auto"))
    );
}

#[test]
fn generated_schema_exposes_named_pi_stores() {
    let schema = generated_schema();
    let stores = schema_property(&schema, &["pi", "stores"]).unwrap();

    assert_eq!(stores["type"], json!("array"));
    assert_eq!(
        stores["items"]["properties"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>(),
        ["name", "path"].into_iter().collect::<BTreeSet<_>>()
    );
    assert_eq!(
        stores["items"]["properties"]["name"]["pattern"],
        json!("^[a-z][a-z0-9_-]{0,31}$")
    );
}

#[test]
fn statusline_options_parse_model_label_aliases() {
    let map = serde_json::json!({
        "modelLabelAliases": {
            "arn:aws:bedrock:ap-northeast-1:012345678910:application-inference-profile/abcde12345": "claude-opus-4-6"
        }
    });
    let options = serde_json::from_value::<StatuslineSpecificOptions>(map).unwrap();

    let aliases = options.model_label_aliases.unwrap();
    assert_eq!(
        aliases.get(
            "arn:aws:bedrock:ap-northeast-1:012345678910:application-inference-profile/abcde12345"
        ),
        Some(&"claude-opus-4-6".to_string())
    );
}

#[test]
fn statusline_options_reject_non_string_alias_values() {
    let map = serde_json::json!({
        "modelLabelAliases": {
            "valid": "short",
            "invalid": 123
        }
    });
    assert!(serde_json::from_value::<StatuslineSpecificOptions>(map).is_err());
}

#[test]
fn codex_statusline_schema_exposes_shared_options_with_offline_default() {
    let schema: Value = serde_json::from_str(&generate_config_schema_json().unwrap()).unwrap();
    assert_eq!(
        property_default(&schema, &["codex", "commands", "statusline", "offline"]),
        Some(&json!(true))
    );
    assert!(schema_property(&schema, &["codex", "commands", "statusline", "speed"]).is_some());
    for option in ["breakdown", "compact", "order", "debug", "all", "color"] {
        assert!(
            schema_property(&schema, &["codex", "commands", "statusline", option]).is_none(),
            "{option}"
        );
    }
    for name in ["codex statusline", "codex:statusline"] {
        assert!(schema_property(&schema, &["commands", name]).is_some());
    }
}

#[test]
fn codex_statusline_schema_reports_a_missing_option() {
    let mut generator = SchemaSettings::draft07().into_generator();
    let shared = option_properties::<SharedOptions>(&mut generator).unwrap();
    let error = codex_statusline_section(&shared, &Map::new(), true).unwrap_err();
    assert_eq!(
        error.to_string(),
        "missing object schema for Codex statusline option 'speed'"
    );
}

#[test]
fn snapshots_schema_agent_specific_option_edges() {
    let schema = generated_schema();

    insta::assert_json_snapshot!(json!({
        "rootRef": schema["$ref"],
        "rootProperties": definition_properties(&schema, "ccusage-config"),
        "rootAdditionalProperties": schema["definitions"]["ccusage-config"]["additionalProperties"],
        "defaults": schema_node(&schema, &["defaults"]),
        "rootDaily": schema_node(&schema, &["commands", "daily"]),
        "claudeStatusline": schema_node(&schema, &["claude", "commands", "statusline"]),
        "codexDefaults": schema_node(&schema, &["codex", "defaults"]),
        "opencodeWeekly": schema_node(&schema, &["opencode", "commands", "weekly"]),
        "piDefaults": schema_node(&schema, &["pi", "defaults"]),
        "openclawDefaults": schema_node(&schema, &["openclaw", "defaults"]),
        "grokDefaults": schema_node(&schema, &["grok", "defaults"]),
        "zcodeDefaults": schema_node(&schema, &["zcode", "defaults"]),
        "clineWeekly": schema_node(&schema, &["cline", "commands", "weekly"]),
    }));
}

#[test]
fn token_limit_schema_requires_max_or_positive_integers() {
    let schema = generated_schema();
    let token_limit = schema_property(&schema, &["commands", "blocks", "tokenLimit"]).unwrap();
    assert_eq!(token_limit["anyOf"][1]["minimum"], 1.0);
    assert!(token_limit["anyOf"][0]["pattern"].as_str().is_some());
}

fn generated_schema() -> Value {
    serde_json::from_str(&generate_config_schema_json().unwrap()).unwrap()
}

fn assert_properties(schema: &Value, definition: &str, expected: &[&str]) {
    assert_eq!(
        definition_properties(schema, definition),
        expected.iter().copied().collect::<BTreeSet<_>>(),
        "{definition} properties did not match"
    );
}

fn definition_properties<'a>(schema: &'a Value, definition: &str) -> BTreeSet<&'a str> {
    schema["definitions"][definition]["properties"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect()
}

fn assert_schema_properties(schema: &Value, path: &[&str], expected: &[&str]) {
    assert_eq!(
        schema_properties(schema, path),
        expected.iter().copied().collect::<BTreeSet<_>>(),
        "{path:?} properties did not match"
    );
}

fn schema_properties<'a>(schema: &'a Value, path: &[&str]) -> BTreeSet<&'a str> {
    schema_node(schema, path)["properties"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect()
}

fn property_default<'a>(schema: &'a Value, path: &[&str]) -> Option<&'a Value> {
    schema_property(schema, path).and_then(|property| property.get("default"))
}

fn schema_property<'a>(schema: &'a Value, path: &[&str]) -> Option<&'a Value> {
    let (property, parent_path) = path.split_last().unwrap();
    schema_node(schema, parent_path)["properties"].get(*property)
}

fn schema_node<'a>(schema: &'a Value, path: &[&str]) -> &'a Value {
    let mut node = &schema["definitions"]["ccusage-config"];
    for segment in path {
        node = &node["properties"][*segment];
    }
    node
}

fn with_keys<'a>(base: &[&'a str], extra: &[&'a str]) -> Vec<&'a str> {
    base.iter().chain(extra).copied().collect()
}

fn assert_value_keys_allowed_by_schema(value: &Value, schema: &Value, root: &Value) {
    let Some(value_object) = value.as_object() else {
        return;
    };
    let schema = resolve_schema(schema, root);
    let schema = merge_all_of(schema, root);
    let properties = schema.get("properties").and_then(Value::as_object);
    for (key, child_value) in value_object {
        let child_schema = if let Some(properties) = properties {
            if let Some(child_schema) = properties.get(key) {
                child_schema
            } else if schema
                .get("additionalProperties")
                .is_some_and(|v| !v.is_null() && *v != Value::Bool(false))
            {
                schema.get("additionalProperties").unwrap()
            } else {
                panic!("schema does not allow config key {key}");
            }
        } else if schema
            .get("additionalProperties")
            .is_some_and(|v| !v.is_null() && *v != Value::Bool(false))
        {
            schema.get("additionalProperties").unwrap()
        } else {
            panic!("schema node has no properties: {schema:?}");
        };
        assert_value_keys_allowed_by_schema(child_value, child_schema, root);
    }
}

fn resolve_schema<'a>(schema: &'a Value, root: &'a Value) -> &'a Value {
    let Some(reference) = schema.get("$ref").and_then(Value::as_str) else {
        return schema;
    };
    root.pointer(reference.strip_prefix('#').unwrap()).unwrap()
}

fn merge_all_of(schema: &Value, root: &Value) -> Value {
    let Some(items) = schema.get("allOf").and_then(Value::as_array) else {
        return schema.clone();
    };
    let mut merged = schema.clone();
    let properties = merged
        .as_object_mut()
        .unwrap()
        .entry("properties")
        .or_insert_with(|| Value::Object(Default::default()));
    let properties = properties.as_object_mut().unwrap();
    for item in items {
        let resolved = resolve_schema(item, root);
        for (key, value) in resolved["properties"].as_object().unwrap() {
            properties.insert(key.clone(), value.clone());
        }
    }
    merged
}
