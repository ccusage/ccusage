# ccusage-pricing-snapshots

Set `CCUSAGE_PRICING_JSON_PATH` to build with the pinned LiteLLM snapshot.
Nix and the dev shell set it automatically. Enable `fetch-litellm-pricing`
for a plain Cargo build that needs to download the locked snapshot.
Run `just gen-models-dev-pricing` to regenerate the models.dev snapshots.

The build script builds the offline pricing catalog from both snapshots with
`ccusage_core::pricing::encode_pricing_table` and embeds it as
`PRICING_TABLE`, so a report decodes finished rows instead of parsing JSON.
The binary embeds only that table, the LiteLLM source rules and the models.dev
catalog rules; the JSON snapshots are exported for tests.

## Build layer

Compiled with the final binary, so refreshing snapshots leaves the cached
foundation and adapter layers intact. Keep `fetch-litellm-pricing` off by
default to keep its HTTPS build dependencies off the normal build path.
