//! The anonymous daily usage ping, shared by the CLI that sends it and the
//! collector Worker in `apps/telemetry` that validates it.
//!
//! [`Ping`] is the single definition of the wire format. The Worker never
//! restates it: `generate-telemetry-schema` turns it into a JSON Schema module
//! that the Worker validates against and derives its TypeScript type from, and
//! the `telemetry-schema` flake check fails when that module drifts.

use schemars::{JsonSchema, r#gen::SchemaSettings};
use serde::Serialize;

const SHORT_FIELD_PATTERN: &str = r"^[\w .+-]{1,64}$";

/// The one document the CLI sends, at most once per UTC day.
///
/// The collector strips unknown keys rather than rejecting them, so a field can
/// be added here before the Worker is redeployed; removing or tightening one
/// breaks older CLIs that are still sending it.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Ping {
    pub event: PingEvent,
    /// Random 128-bit value as lowercase hex, generated once per install.
    #[schemars(regex(pattern = "^[0-9a-f]{32}$"))]
    pub anonymous_id: String,
    #[schemars(regex = "SHORT_FIELD_PATTERN")]
    pub version: String,
    #[schemars(regex = "SHORT_FIELD_PATTERN")]
    pub os: String,
    #[schemars(regex = "SHORT_FIELD_PATTERN")]
    pub arch: String,
    /// The command as typed, without arguments or option values.
    #[schemars(regex = "SHORT_FIELD_PATTERN")]
    pub command: String,
}

#[derive(Clone, Copy, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PingEvent {
    CliDailyActive,
}

/// Renders [`Ping`]'s JSON Schema as the TypeScript module the Worker imports.
///
/// Subschemas are inlined so the module is one self-contained object, which is
/// what `json-schema-to-ts` needs to derive a type from it.
///
/// # Examples
///
/// ```
/// let module = ccusage_telemetry::generate_schema_module();
/// assert!(module.contains("export const pingJsonSchema"));
/// ```
pub fn generate_schema_module() -> String {
    let generator = SchemaSettings::draft07()
        .with(|settings| {
            settings.inline_subschemas = true;
            settings.option_add_null_type = false;
        })
        .into_generator();
    let schema = generator.into_root_schema_for::<Ping>();
    let json = serde_json::to_string_pretty(&schema).expect("schema serializes");
    format!(
        "// Generated from `rust/crates/ccusage-telemetry` by `just telemetry::generate-schema`.\n\
         // Do not edit; change the Rust `Ping` type instead.\n\
         export const pingJsonSchema = {json} as const;\n"
    )
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    fn schema_value() -> Value {
        let module = generate_schema_module();
        let start = module.find('{').unwrap();
        let end = module.rfind('}').unwrap();
        serde_json::from_str(&module[start..=end]).unwrap()
    }

    #[test]
    fn serializes_to_the_documented_wire_format() {
        let ping = Ping {
            event: PingEvent::CliDailyActive,
            anonymous_id: "0".repeat(32),
            version: "20.0.38".to_string(),
            os: "macos".to_string(),
            arch: "aarch64".to_string(),
            command: "codex monthly".to_string(),
        };

        assert_eq!(
            serde_json::to_value(ping).unwrap(),
            json!({
                "event": "cli_daily_active",
                "anonymousId": "00000000000000000000000000000000",
                "version": "20.0.38",
                "os": "macos",
                "arch": "aarch64",
                "command": "codex monthly",
            })
        );
    }

    #[test]
    fn schema_is_self_contained_and_requires_every_field() {
        let schema = schema_value();

        assert!(schema.get("definitions").is_none());
        assert_eq!(
            schema["required"],
            json!(["anonymousId", "arch", "command", "event", "os", "version"])
        );
        assert_eq!(
            schema["properties"]["event"]["enum"],
            json!(["cli_daily_active"])
        );
        assert_eq!(
            schema["properties"]["command"]["pattern"],
            json!(SHORT_FIELD_PATTERN)
        );
    }
}
