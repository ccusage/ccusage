//! `ccusage.json` loading, validation, and the JSON Schema published for editors.
//!
//! Split from `ccusage-core` because only the binary reads configuration, and the
//! schema derives pull `schemars` in: keeping them here means neither reaches the
//! critical path of `ccusage-core` or any adapter.
pub mod config;
pub mod config_schema;

pub use config::ConfigContext;
pub use config_schema::generate_config_schema_json;
