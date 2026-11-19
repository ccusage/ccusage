# ccusage-core

Shared runtime for the binary and adapters. Keep binary-only code in its own
crate and pricing snapshot data in `ccusage-pricing-snapshots`.

models.dev can list one model in several catalogs. Maintain
[`models-dev-catalog-rules.json`](../ccusage-pricing-snapshots/src/models-dev-catalog-rules.json) for the author, reseller, tier, and billing-unit
facts missing from the live response.

## Build layer

Built in the `foundation` Crane layer. A change here recompiles every adapter.
