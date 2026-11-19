# ccusage-pricing-snapshots

Set `CCUSAGE_PRICING_JSON_PATH` to build with the pinned LiteLLM snapshot.
Nix and the dev shell set it automatically. Enable `fetch-litellm-pricing`
for a plain Cargo build that needs to download the locked snapshot.
Run `just gen-models-dev-pricing` to regenerate the models.dev snapshots.

## Build layer

Compiled with the final binary, so refreshing snapshots leaves the cached
foundation and adapter layers intact. Keep `fetch-litellm-pricing` off by
default to keep its HTTPS build dependencies off the normal build path.
