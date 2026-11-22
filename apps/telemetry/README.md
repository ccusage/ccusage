# @ccusage/telemetry

The Cloudflare Worker behind `https://telemetry.ccusage.com/v1/ping`, which receives the anonymous daily ping described in the [Telemetry guide](https://ccusage.com/guide/telemetry). The client side lives in `rust/crates/ccusage/src/telemetry.rs`.

The Worker validates the ping and writes its six fields to the `ccusage_telemetry` [Workers Analytics Engine](https://developers.cloudflare.com/analytics/analytics-engine/) dataset. It stores nothing else: no IP address, headers, or Cloudflare request metadata, and Workers Logs are disabled in `cloudflare.config.ts`.

| Column   | Field         |
| -------- | ------------- |
| `index1` | `anonymousId` |
| `blob1`  | `event`       |
| `blob2`  | `version`     |
| `blob3`  | `os`          |
| `blob4`  | `arch`        |
| `blob5`  | `command`     |

## Daily rollups

Raw pings only live as long as Analytics Engine retains them, so a Cron Trigger at 00:15 UTC aggregates the previous UTC day and writes it to the `ccusage-telemetry-rollups` R2 bucket as `daily/YYYY-MM-DD.json`. These small files are what keeps the long-term trend; rerunning a day replaces its file.

```json
{
	"date": "2026-10-21",
	"generatedAt": "2026-10-22T00:15:03.000Z",
	"dailyActiveUsers": 10,
	"weeklyActiveUsers": 25,
	"monthlyActiveUsers": 42,
	"byPlatform": [{ "os": "macos", "arch": "aarch64", "users": 7 }],
	"byVersion": [{ "version": "20.0.38", "users": 10 }],
	"byCommand": [{ "command": "daily", "users": 6 }]
}
```

The weekly and monthly counts are distinct users over the 7 and 30 days ending that date. They are computed at rollup time because they cannot be rebuilt from daily files once the raw pings expire.

The rollup reads the dataset through an Analytics SQL binding, so it needs no API token. That binding has no local simulation, which means `cf dev` connects to Cloudflare and requires you to be logged in; the Node tests cover the rollup without it.

## Recipes

The Worker is built and deployed with Cloudflare's [`cf` CLI](https://github.com/cloudflare/cf) (beta), configured in `cloudflare.config.ts`.

```bash
just telemetry::dev     # run locally with cf dev
just telemetry::test    # Node tests
just telemetry::deploy  # deploy to Cloudflare
```

## Automated deployment

The `Deploy telemetry Worker` GitHub Actions workflow tests, builds, and deploys the Worker when changes to `apps/telemetry/` or its workspace lockfiles reach `main`. It can also be run manually from `main` with **Actions → Deploy telemetry Worker → Run workflow**. Runs are serialized so two deployments cannot race.

Before the first run, create a GitHub environment named `telemetry-production`, allow deployments from `main` only, and add these environment secrets:

- `CLOUDFLARE_API_TOKEN` — an API token scoped to the Cloudflare account and zone used by this Worker, with permissions to deploy Workers and manage the configured bindings and custom domain.
- `CLOUDFLARE_ACCOUNT_ID` — the target Cloudflare account ID.

Actions reads GitHub environment secrets directly. A 1Password account or service account is not required. Rotate the deployment token by replacing the GitHub environment secret.

The first deployment needs the `ccusage.com` zone available to that account. `cf deploy` provisions supported bindings from `cloudflare.config.ts`; check the Cloudflare deployment output for any account resources that require one-time setup.

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

`GET /` serves the dashboard and `GET /dashboard.js` serves its script without third-party dependencies or page analytics. `GET /v1/stats` reads the last 90 completed UTC days from the private R2 bucket. It validates the stored data and publishes only dates, aggregation timestamps, daily/weekly/monthly active ID counts, and platform/version aggregates. It never returns installation IDs or command breakdowns.

Successful API responses are cached with the Workers Cache API for one hour per edge location and in browsers for five minutes. Query strings share one cache key. Missing files remain gaps; invalid data or an R2 failure returns an uncacheable 503. Cache misses read at most 90 objects in batches of six. The dashboard distinguishes no data, failed loading and stale aggregates, and provides a data table and a JSON download alongside its chart.

There is no public R2 bucket or SQL endpoint. Do not add identifiers or raw pings to the public schema. Local development shows the empty state until the local R2 bucket contains daily aggregate files.
