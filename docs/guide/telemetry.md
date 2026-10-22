# Telemetry

ccusage collects anonymous usage data so the project knows roughly how many people use it, on which platforms, and which commands matter. It is on by default, sends at most one small ping per day, and can be turned off with one command.

## Fully Transparent

Every part of the pipeline is public, so you never have to take this page's word for it:

- **Client source** - the code that decides what to send and when lives in [`rust/crates/ccusage/src/telemetry.rs`](https://github.com/ryoppippi/ccusage/blob/main/rust/crates/ccusage/src/telemetry.rs).
- **Server source** - the collector that receives the ping is a small Cloudflare Worker in [`apps/telemetry`](https://github.com/ryoppippi/ccusage/tree/main/apps/telemetry), in the same repository. You can read exactly what it validates and stores.
- **One definition of the payload** - the ping's format is defined once, as the Rust [`Ping`](https://github.com/ryoppippi/ccusage/blob/main/rust/crates/ccusage-telemetry/src/lib.rs) type. The Worker's validation schema is generated from it, and CI fails if the two ever disagree, so the CLI cannot quietly start sending a field the collector does not document.
- **Builds and npm publishing** - release binaries are built and published to npm by the public [release workflow](https://github.com/ryoppippi/ccusage/blob/main/.github/workflows/release.yaml) on GitHub Actions, never from a developer machine. Every package is published with [npm provenance](https://docs.npmjs.com/generating-provenance-statements), so the npm page for each version links to the exact commit and workflow run that produced it.

Builds you compile yourself with `cargo build` contain no collector address and never send anything; only official release builds do.

## What Is Sent

The first ccusage run of each UTC day sends one JSON document:

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

| Field         | Meaning                                                                   |
| ------------- | ------------------------------------------------------------------------- |
| `event`       | Always `cli_daily_active`                                                 |
| `anonymousId` | Random 128-bit value generated on your machine, not derived from anything |
| `version`     | ccusage version                                                           |
| `os`, `arch`  | Operating system and CPU architecture                                     |
| `command`     | The command as typed, such as `daily`, `codex monthly`, or `statusline`   |

That is the complete list. ccusage never sends anything read from your agent logs: no token counts, costs, model names, project names, paths, session IDs, or prompts. Arguments and option values are not sent either.

The ping runs in the background. It never changes report output, never writes to stdout, and a slow or failed request is dropped without a retry.

## What Is Stored

The collector stores the six fields above and nothing else: no IP address, headers, or request metadata.

- **Individual pings** are kept only as long as Cloudflare Workers Analytics Engine retains them, then expire.
- **Daily totals** are computed once a day and kept long-term: the number of distinct users that day and over the last 7 and 30 days, plus user counts per OS and architecture, version, and command. These totals contain no anonymous IDs.

## First-Run Notice

The first time telemetry is active, ccusage prints a short notice to stderr explaining what is collected and how to opt out. `--json` output on stdout is unaffected. The `statusline` command does not print the notice, because Claude Code reads its output; the notice appears on your next interactive command instead.

## Opting Out

Any of these turns telemetry off:

```bash
# Store the preference for this user
ccusage telemetry disable

# Or set an environment variable
export CCUSAGE_TELEMETRY_DISABLED=1
export DO_NOT_TRACK=1
```

Telemetry is also off automatically when the `CI` environment variable is set.

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
