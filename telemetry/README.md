# @ccusage/telemetry

The Cloudflare Worker behind `https://telemetry.ccusage.com/v1/ping`, which receives the anonymous daily ping described in the [Telemetry guide](https://ccusage.com/guide/telemetry). The client side lives in `rust/crates/ccusage/src/telemetry.rs`.

The Worker validates the ping and writes its six fields to the `ccusage_telemetry` [Workers Analytics Engine](https://developers.cloudflare.com/analytics/analytics-engine/) dataset. It stores nothing else: no IP address, headers, or Cloudflare request metadata, and Workers Logs are disabled in `void.config.ts`.

| Column   | Field         |
| -------- | ------------- |
| `index1` | `anonymousId` |
| `blob1`  | `event`       |
| `blob2`  | `version`     |
| `blob3`  | `os`          |
| `blob4`  | `arch`        |
| `blob5`  | `command`     |

## Daily rollups

Raw pings only live as long as Analytics Engine retains them, so a Cron Trigger at 00:15 UTC recomputes the previous three completed UTC days and writes them to the `ccusage-telemetry-rollups` KV namespace as `daily/YYYY-MM-DD.json`. These archives preserve the long-term trend; rerunning a day replaces its file. The public snapshot is rebuilt from the last 90 archive keys in one bulk KV read, overlaid with the current run's results so publication does not depend on immediate read-after-write consistency. A corrupt public snapshot is never used as an input.

```json
{
	"date": "2026-10-21",
	"generatedAt": "2026-10-22T00:15:03.000Z",
	"dailyActiveUsers": 10,
	"weeklyActiveUsers": 25,
	"monthlyActiveUsers": 42,
	"samplingObserved": false,
	"byPlatform": [{ "os": "macos", "arch": "aarch64", "users": 7 }],
	"byVersion": [{ "version": "20.0.38", "users": 10 }],
	"byCommand": [{ "command": "daily", "users": 6 }]
}
```

The weekly and monthly counts are distinct users over the 7 and 30 days ending that date. They are computed at rollup time because they cannot be rebuilt from daily files once the raw pings expire.

Platform, version and command queries retain the top 20 groups per day. The limit applies in SQL, before results reach the Worker, and keeps the 90-day public projection well below the KV value-size limit even with high-cardinality valid input. Active-ID totals are queried separately and are not truncated; no synthetic "other" count is produced by summing distinct groups.

Public projection also selects the top 20 platform and version groups from every archive, including older unbounded archives. It preserves the stored archives, active-ID totals and sampling metadata.

Each new rollup records `samplingObserved` from `APPROX_MAX(sampleInterval)` in its six Analytics SQL queries. The installation ID is reused across days and is the dimension being counted; it is not a unique index per event. Sampling intervals are diagnostic metadata, not a multiplier for distinct IDs or an error estimate. Older archives without the metadata remain valid. The dashboard warns about observed sampling and inconsistent nested-window counts without clamping the returned values.

The three-day lookback recovers recent missed runs; longer outages remain visible as gaps. Rejected scheduled promises appear in Cloudflare's Cron Trigger Past Events status. The `Check telemetry rollup` GitHub workflow checks the public snapshot daily at 02:23 UTC and fails when yesterday's aggregate is missing. It can also be dispatched manually and requires no Cloudflare credentials. Request logging stays disabled.

The rollup reads the dataset through an Analytics SQL binding, so it needs no API token. Keep this account on Workers Free: KV operations fail at their free quota rather than billing for overages. No R2 subscription is required. That binding has no local simulation, so the scheduled rollup cannot run in local development; the tests cover the rollup without it.

## Recipes

The Worker is a [Void](https://void.cloud/guide/) app deployed straight to the ccusage Cloudflare account; it does not use the Void platform. `void.config.ts` holds the Worker settings and passes its `cloudflare` block, including the Analytics Engine, Analytics SQL and KV bindings, Workers Cache and the custom domain, through to the generated Cloudflare config. File conventions map the app:

- `routes/v1/ping.ts` and `routes/v1/stats.ts` are the API, as Hono handlers.
- `crons/rollup.ts` is the daily rollup. Void requires its cron expression to be a literal, so a test checks it against `ROLLUP_CRON`, which the edge cache lifetime uses.
- `pages/index.island.svelte` server-renders the dashboard from the loader in `pages/index.server.ts`. Only the chart and the theme toggle in `src/components/` hydrate as islands; other components keep their styles in their own `<style>` blocks.
- `src/bindings.d.ts` types the bindings so type checking does not depend on Void's generated `.void/` directory.

```bash
just telemetry::dev     # run locally with vite dev
just telemetry::test    # Vitest tests
just telemetry::deploy  # deploy to Cloudflare with void deploy
```

## Automated deployment

The `Deploy telemetry Worker` GitHub Actions workflow tests, builds, and deploys the Worker when changes to `telemetry/` or its workspace lockfiles reach `main`. It can also be run manually from `main` with **Actions → Deploy telemetry Worker → Run workflow**. Runs are serialized so two deployments cannot race.

Before the first run, create a GitHub environment named `telemetry-production`, allow deployments from `main` only, and add these environment secrets:

- `CLOUDFLARE_API_TOKEN` — an API token scoped to the Cloudflare account and zone used by this Worker, with permissions to deploy Workers and read the configured bindings and custom domain.
- `CLOUDFLARE_ACCOUNT_ID` — the target Cloudflare account ID.

Actions reads GitHub environment secrets directly. A 1Password account or service account is not required. Rotate the deployment token by replacing the GitHub environment secret.

The first deployment needs the `ccusage.com` zone and the KV namespace referenced in `void.config.ts` in that account. Enable Analytics Engine and create the KV namespace before deploying. No R2 subscription is needed. `void deploy` records the linked Worker in `void.lock.json`; commit it so CI deploys to the same Worker. The `VERSION` version metadata binding lets Void check a new version before it receives traffic, because `workers.dev` is off.

## Querying

Analytics Engine is queried with SQL over its [HTTP API](https://developers.cloudflare.com/analytics/analytics-engine/sql-api/). Each anonymous ID sends at most one ping per UTC day, so distinct IDs per day are daily active users:

```sql
SELECT toStartOfDay(timestamp) AS day, count(DISTINCT index1) AS users
FROM ccusage_telemetry
WHERE timestamp > NOW() - INTERVAL '30' DAY
GROUP BY day
ORDER BY day
```

Monthly active users, and the platform split:

```sql
SELECT count(DISTINCT index1) AS users
FROM ccusage_telemetry
WHERE timestamp > NOW() - INTERVAL '30' DAY;

SELECT blob3 AS os, blob4 AS arch, count(DISTINCT index1) AS users
FROM ccusage_telemetry
WHERE timestamp > NOW() - INTERVAL '30' DAY
GROUP BY os, arch
ORDER BY users DESC;
```

## Public dashboard

`GET /` serves the dashboard, rendered on the server from the same KV snapshot and cached with the same headers and tag as the API, so the cron purge refreshes both. Only the chart and theme toggle ship JavaScript. TanStack Charts and the documentation site's logo and font are bundled as same-origin static assets. The page uses no CDN scripts or page analytics. `GET /v1/stats` reads a single cached snapshot of the last 90 completed UTC days from the private KV namespace. It validates the stored data and publishes only dates, aggregation timestamps, daily/weekly/monthly active ID counts, sampling metadata, and platform/version aggregates. It never returns installation IDs or command breakdowns.

Successful API responses are cached with Workers Cache under the `public-stats` cache tag, and each scheduled run purges that tag after publishing the snapshot. A snapshot that includes the previous UTC day stays at the edge until the next scheduled rollup; one that does not, including the empty state, stays for one minute so a stale read cannot hide a new aggregate. Browsers cache responses for at most five minutes. Missing days remain gaps; invalid data or a KV failure returns an uncacheable 503. Public cache misses read one KV value. A scheduled run uses 18 SQL queries, three archive writes, one bulk read of 90 archive keys and one public snapshot write. Bulk reads count as one Worker operation but consume one KV read per key. The dashboard distinguishes no data, failed loading, stale aggregates and sampling warnings, and provides a data table and a JSON download alongside its chart. Platform and version breakdowns show the latest day's split as a segmented bar above the ranked list, and the last 30 days as normalized stacked bars. The five largest groups of the latest day keep fixed colors, validated for color-vision deficiencies in both themes; the rest, including groups beyond the top 20, fold into a gray Other, which is the daily active count minus the named groups because each installation reports one platform and one version per day. Every chart has a table view.

There is no public KV namespace or SQL endpoint. Do not add identifiers or raw pings to the public schema. Local development shows the empty state until the local KV namespace contains a public snapshot; `vite dev` prints the local explorer API, whose KV endpoints can write one.
