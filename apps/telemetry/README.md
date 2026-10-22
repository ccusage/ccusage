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

## Recipes

The Worker is built and deployed with Cloudflare's [`cf` CLI](https://github.com/cloudflare/cf) (beta), configured in `cloudflare.config.ts`.

```bash
just telemetry::dev     # run locally with cf dev
just telemetry::test    # Node tests
just telemetry::deploy  # deploy to Cloudflare
```

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
