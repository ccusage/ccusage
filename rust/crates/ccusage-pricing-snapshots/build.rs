use std::{fs, path::PathBuf};

use ccusage_core::pricing::{EmbeddedSnapshots, encode_pricing_table, set_embedded_snapshots};
use ccusage_environment::Environment;

use serde_json::Value;

#[path = "src/litellm_filter.rs"]
mod litellm_filter;

const FLAKE_LOCK_JSON: &str = "../../../flake.lock";
const OUT_PRICING_JSON: &str = "litellm-pricing.json.deflate";
const MODELS_DEV_PRICING_JSON: &str = "src/models-dev-pricing.json";
const OUT_MODELS_DEV_PRICING_JSON: &str = "models-dev-pricing.json.deflate";
const MODELS_DEV_CATALOG_RULES_JSON: &str = "src/models-dev-catalog-rules.json";
const OUT_MODELS_DEV_CATALOG_RULES_JSON: &str = "models-dev-catalog-rules.json.deflate";
const OUT_PRICING_TABLE: &str = "pricing-table.bin.deflate";
const OUT_FLOAT_PROBE: &str = "float-probe.rs";
// NOTE(2026-11-20, #2399): serde_json's default float parser misrounds this rate by one ULP and its `float_roundtrip` parser does not. Record how this build script parses it, so a test can check that the runtime parses rates the same way.
const FLOAT_PROBE: &str = "1.4999999999999999e-07";
const PRICING_JSON_PATH_ENV: &str = "CCUSAGE_PRICING_JSON_PATH";
#[cfg(feature = "fetch-litellm-pricing")]
const PRICING_FETCH_TIMEOUT_SECONDS: u64 = 10;

fn main() -> Result<(), String> {
    println!("cargo:rerun-if-env-changed={PRICING_JSON_PATH_ENV}");

    let out_path = out_dir_path(OUT_PRICING_JSON)?;
    let pricing_json = if let Some(path) = Environment::current().var_os(PRICING_JSON_PATH_ENV) {
        let path = PathBuf::from(path);
        println!("cargo:rerun-if-changed={}", path.display());
        fs::read_to_string(path).map_err(|error| {
            format!("read pricing snapshot from CCUSAGE_PRICING_JSON_PATH: {error}")
        })?
    } else {
        println!("cargo:rerun-if-changed={FLAKE_LOCK_JSON}");
        fetch_pricing_json()?
    };
    let models_dev_pricing = embed_snapshot(MODELS_DEV_PRICING_JSON, OUT_MODELS_DEV_PRICING_JSON)?;
    let paid_hosts = litellm_filter::paid_host_providers(&pricing_json, &models_dev_pricing.json)
        .ok_or_else(|| "list paid hosts".to_string())?;
    let pricing_json = litellm_filter::filter_pricing_json(&pricing_json)
        .ok_or_else(|| "filter LiteLLM pricing JSON".to_string())?;
    ensure_reparses_to_itself("filtered LiteLLM pricing", &pricing_json)?;
    write_float_probe()?;

    fs::write(out_path, deflate(pricing_json.as_bytes()))
        .map_err(|error| format!("write build-time pricing snapshot: {error}"))?;

    let litellm_sources = embed_litellm_sources(paid_hosts)?;
    let models_dev_catalog_rules = embed_snapshot(
        MODELS_DEV_CATALOG_RULES_JSON,
        OUT_MODELS_DEV_CATALOG_RULES_JSON,
    )?;

    set_embedded_snapshots(EmbeddedSnapshots {
        litellm_sources: litellm_sources.deflated.leak(),
        models_dev_catalog_rules: models_dev_catalog_rules.deflated.leak(),
        pricing_table: &[],
    });
    let table = encode_pricing_table(&pricing_json, &models_dev_pricing.json);
    fs::write(out_dir_path(OUT_PRICING_TABLE)?, deflate(&table))
        .map_err(|error| format!("write {OUT_PRICING_TABLE}: {error}"))
}

struct Snapshot {
    json: String,
    deflated: Vec<u8>,
}

fn embed_snapshot(source: &str, out_name: &str) -> Result<Snapshot, String> {
    println!("cargo:rerun-if-changed={source}");
    let snapshot = fs::read_to_string(source).map_err(|error| format!("read {source}: {error}"))?;
    let value = serde_json::from_str::<Value>(&snapshot)
        .map_err(|error| format!("parse {source}: {error}"))?;
    let json =
        serde_json::to_string(&value).map_err(|error| format!("serialize {source}: {error}"))?;
    ensure_reparses_to_itself(source, &json)?;
    let deflated = deflate(json.as_bytes());
    fs::write(out_dir_path(out_name)?, &deflated)
        .map_err(|error| format!("write {out_name}: {error}"))?;
    Ok(Snapshot { json, deflated })
}

fn embed_litellm_sources(paid_hosts: Vec<String>) -> Result<Snapshot, String> {
    const SOURCE: &str = "src/litellm-sources.json";
    println!("cargo:rerun-if-changed={SOURCE}");
    let snapshot = fs::read_to_string(SOURCE).map_err(|error| format!("read {SOURCE}: {error}"))?;
    let mut value = serde_json::from_str::<Value>(&snapshot)
        .map_err(|error| format!("parse {SOURCE}: {error}"))?;
    value["paidHostProviders"] = Value::from(paid_hosts);
    let json =
        serde_json::to_string(&value).map_err(|error| format!("serialize {SOURCE}: {error}"))?;
    let deflated = deflate(json.as_bytes());
    fs::write(out_dir_path("litellm-sources.json.deflate")?, &deflated)
        .map_err(|error| format!("write litellm-sources.json.deflate: {error}"))?;
    Ok(Snapshot { json, deflated })
}

// NOTE(2026-11-20, #2399): The table is encoded from rewritten JSON while fetched pricing is parsed from upstream text. When parsing the rewritten JSON and writing it again gives back the same text, every number in it is the value the parser read from the upstream text.
fn ensure_reparses_to_itself(name: &str, json: &str) -> Result<(), String> {
    let value =
        serde_json::from_str::<Value>(json).map_err(|error| format!("parse {name}: {error}"))?;
    let rewritten =
        serde_json::to_string(&value).map_err(|error| format!("serialize {name}: {error}"))?;
    if rewritten == json {
        Ok(())
    } else {
        Err(format!(
            "{name} changes when parsed and written again, so a rate would shift"
        ))
    }
}

fn write_float_probe() -> Result<(), String> {
    let bits = serde_json::from_str::<f64>(FLOAT_PROBE)
        .map_err(|error| format!("parse {FLOAT_PROBE}: {error}"))?
        .to_bits();
    fs::write(
        out_dir_path(OUT_FLOAT_PROBE)?,
        format!(
            "const FLOAT_PROBE: &str = {FLOAT_PROBE:?};\nconst FLOAT_PROBE_BITS: u64 = {bits};\n"
        ),
    )
    .map_err(|error| format!("write {OUT_FLOAT_PROBE}: {error}"))
}

fn deflate(bytes: &[u8]) -> Vec<u8> {
    miniz_oxide::deflate::compress_to_vec(bytes, 10)
}

fn out_dir_path(file_name: &str) -> Result<PathBuf, String> {
    let out_dir = Environment::current()
        .var_os("OUT_DIR")
        .ok_or_else(|| "OUT_DIR is set by cargo".to_string())?;
    Ok(PathBuf::from(out_dir).join(file_name))
}

#[cfg(not(feature = "fetch-litellm-pricing"))]
fn fetch_pricing_json() -> Result<String, String> {
    Err(format!(
        "no LiteLLM pricing snapshot available: set {PRICING_JSON_PATH_ENV} to a \
         model_prices_and_context_window.json, or build with \
         --features fetch-litellm-pricing to download it"
    ))
}

#[cfg(feature = "fetch-litellm-pricing")]
fn fetch_pricing_json() -> Result<String, String> {
    download_pricing_json().map_err(|error| format!("fetch LiteLLM pricing for embed: {error}"))
}

#[cfg(feature = "fetch-litellm-pricing")]
fn download_pricing_json() -> std::io::Result<String> {
    let response = minreq::get(litellm_pricing_url()?)
        .with_timeout(PRICING_FETCH_TIMEOUT_SECONDS)
        .send()
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    if response.status_code != 200 {
        return Err(std::io::Error::other(format!(
            "HTTP {}",
            response.status_code
        )));
    }
    Ok(response
        .as_str()
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error.to_string()))?
        .to_string())
}

#[cfg(feature = "fetch-litellm-pricing")]
fn litellm_pricing_url() -> std::io::Result<String> {
    let flake_lock = fs::read_to_string(FLAKE_LOCK_JSON)?;
    let Value::Object(root) = serde_json::from_str::<Value>(&flake_lock)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error.to_string()))?
    else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "flake.lock must be a JSON object",
        ));
    };
    let locked = root
        .get("nodes")
        .and_then(|nodes| nodes.get("litellm"))
        .and_then(|litellm| litellm.get("locked"))
        .and_then(Value::as_object)
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "flake.lock is missing nodes.litellm.locked",
            )
        })?;
    required_flake_lock_string_field(locked, "url")
}

#[cfg(feature = "fetch-litellm-pricing")]
fn required_flake_lock_string_field(
    object: &serde_json::Map<String, Value>,
    field: &str,
) -> std::io::Result<String> {
    object
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("flake.lock nodes.litellm.locked.{field} must be a string"),
            )
        })
}
