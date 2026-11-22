# Telemetry

Official ccusage releases collect minimal usage data so the project can understand adoption and platform support. Collection is on by default after the first-run notice, sends at most one small ping per UTC day, and can be turned off with one command. Agent logs stay on your machine.

## Fully Transparent

Every part of the pipeline is public, so you never have to take this page's word for it:

- **Client source** - the code that decides what to send and when lives in [`rust/crates/ccusage/src/telemetry.rs`](https://github.com/ccusage/ccusage/blob/main/rust/crates/ccusage/src/telemetry.rs).
- **Server source** - the collector that receives the ping is a small Cloudflare Worker in [`apps/telemetry`](https://github.com/ccusage/ccusage/tree/main/apps/telemetry), in the same repository. You can read exactly what it validates and stores.
- **Public statistics** - the [usage dashboard](https://telemetry.ccusage.com/) publishes the daily aggregates used to understand adoption. Its [JSON endpoint](https://telemetry.ccusage.com/v1/stats) is public too; neither exposes installation IDs.
- **One definition of the payload** - the ping's format is defined once, as the Rust [`Ping`](https://github.com/ccusage/ccusage/blob/main/rust/crates/ccusage-telemetry/src/lib.rs) type. The Worker's validation schema is generated from it, and CI fails if the two ever disagree, so the CLI cannot quietly start sending a field the collector does not document.
- **Builds and npm publishing** - release binaries are built and published to npm by the public [release workflow](https://github.com/ccusage/ccusage/blob/main/.github/workflows/release.yaml) on GitHub Actions, never from a developer machine. Every package is published with [npm provenance](https://docs.npmjs.com/generating-provenance-statements), so the npm page for each version links to the exact commit and workflow run that produced it.

Builds you compile yourself with `cargo build` contain no collector address and never send anything; only official release builds do.

## What Is Sent

After the first-run notice, the first eligible ccusage run of each UTC day sends one JSON document:

```json
{
	"event": "cli_daily_active",
	"anonymousId": "4fd4c40cb4140e0e8952ceb2b3502e07",
	"version": "20.0.38",
	"os": "macos",
	"arch": "aarch64",
	"command": "codex monthly"
}
```

| Field         | Meaning                                                                                                                  |
| ------------- | ------------------------------------------------------------------------------------------------------------------------ |
| `event`       | Always `cli_daily_active`                                                                                                |
| `anonymousId` | Random 128-bit value generated on your machine, not derived from anything                                                |
| `version`     | ccusage version                                                                                                          |
| `os`, `arch`  | Operating system and CPU architecture                                                                                    |
| `command`     | The first eligible command that day, such as `daily`, `codex monthly`, or `statusline`; never arguments or option values |

That is the complete list. ccusage never sends anything read from your agent logs: no token counts, costs, model names, project names, paths, session IDs, or prompts. Arguments and option values are not sent either.

The ping runs in the background. It never changes report output, never writes to stdout, and a slow or failed request is dropped without a retry.

## What Is Stored

The collector writes only the six fields above to its dataset. It does not write IP addresses, headers, or request metadata, and Workers Logs are disabled. Cloudflare handles the connection and therefore processes the source IP as part of operating its network. The random persistent ID allows activity from the same configuration environment to be linked across days; this is pseudonymous measurement, not untraceable data.

- **Individual pings** expire after Analytics Engine's three-month retention period. Project maintainers with account access can query them.
- **Daily totals** are computed once a day and kept long-term: the number of distinct users that day and over the last 7 and 30 days, plus user counts per OS and architecture, version, and command. These totals contain no anonymous IDs.

## Public Usage Dashboard

The [public dashboard](https://telemetry.ccusage.com/) shows up to 90 completed UTC days of daily, weekly and monthly active environments, plus platform and version breakdowns for the latest available day. The Worker reads stored KV aggregates; viewing the dashboard does not query individual pings or trigger telemetry. It uses no third-party scripts or page analytics.

An active environment means a distinct random installation ID observed in the relevant window, not a verified person. Multiple devices and reset IDs can increase counts; opt-outs, blocked requests and failed sends can reduce them. These are adoption indicators, not an exact population or a guaranteed lower bound. Command breakdowns are omitted because only the first command of each UTC day is reported.

Daily aggregates are generated after 00:15 UTC. The [aggregate JSON](https://telemetry.ccusage.com/v1/stats) is cached for up to one hour at the edge and five minutes in browsers. Missing days appear as gaps, not zero activity. An empty dashboard means no aggregates are available yet; an error or stale data is shown explicitly.

## First-Run Notice

The first eligible invocation saves its notification state and prints a short notice to stderr explaining what is collected and how to opt out. That invocation does not send a ping; collection can start on a subsequent invocation. `--json` output on stdout is unaffected. The `statusline` command neither prints the notice nor sends anything until another command has shown it. Environments that only use statusline therefore remain unmeasured until that happens.

## Opting Out

Any of these turns telemetry off:

```bash
# Store the preference for this user
ccusage telemetry disable

# Or set an environment variable
export CCUSAGE_TELEMETRY_DISABLED=1
export DO_NOT_TRACK=1
```

Telemetry is also off automatically in CI and when `--offline` is selected. If the settings cannot be read or updated safely, no ping is sent.

Disabling deletes the stored anonymous ID, so enabling it again later starts over as a new anonymous user.

## Checking the Current State

```bash
ccusage telemetry          # same as `ccusage telemetry status`
ccusage telemetry enable
ccusage telemetry disable
```

`ccusage telemetry status` reports whether telemetry is on, and if it is off, which setting turned it off. The preference is stored in `$XDG_CONFIG_HOME/ccusage/telemetry.json`, or `~/.config/ccusage/telemetry.json` when `XDG_CONFIG_HOME` is not set.

To see the exact payload without sending it, set `CCUSAGE_TELEMETRY_DEBUG=1`:

```bash
CCUSAGE_TELEMETRY_DEBUG=1 ccusage daily
```

## Environment Variables

| Variable                     | Effect                                                 |
| ---------------------------- | ------------------------------------------------------ |
| `CCUSAGE_TELEMETRY_DISABLED` | Any value except empty, `0`, or `false` disables it    |
| `DO_NOT_TRACK`               | Same as above, following the `DO_NOT_TRACK` convention |
| `CI`                         | Same as above; set by most CI providers                |
| `CCUSAGE_TELEMETRY_DEBUG`    | Prints the payload to stderr instead of sending it     |

See [Environment Variables](/guide/environment-variables) for every variable ccusage reads.
