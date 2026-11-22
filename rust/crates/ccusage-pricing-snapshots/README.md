# ccusage-pricing-snapshots

Set `CCUSAGE_PRICING_JSON_PATH` to build with the pinned LiteLLM snapshot.
Nix and the dev shell set it automatically. Enable `fetch-litellm-pricing`
for a plain Cargo build that needs to download the locked snapshot.
Run `just gen-models-dev-pricing` to regenerate the models.dev snapshots.

The build script uses `ccusage_core::pricing::encode_pricing_table` to build
the primary `PRICING_TABLE` and the compressed `MODELS_DEV_TABLE` fallback.
Reports read primary rows directly from the binary and inflate the fallback
on its first lookup. The binary also embeds the LiteLLM source rules and the
models.dev catalog rules. The JSON snapshots are exported for tests.

## Build layer

Compiled with the final binary, so refreshing snapshots leaves the cached
foundation and adapter layers intact. Keep `fetch-litellm-pricing` off by
default to keep its HTTPS build dependencies off the normal build path.
