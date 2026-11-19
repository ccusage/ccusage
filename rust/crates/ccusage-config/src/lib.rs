//! `ccusage.json` loading, validation, and the JSON Schema published for editors.
pub mod config;
mod config_schema;
mod layers;

pub use config::ConfigContext;
#[cfg(feature = "schema")]
pub use config_schema::generator::generate_config_schema_json;
