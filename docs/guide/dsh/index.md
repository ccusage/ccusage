# DeepSeek Harness Data Source (Experimental)

> DeepSeek Harness support is experimental. Expect changes while ccusage and [DeepSeek Harness](https://github.com/deepseek-ai/deepseek-harness) continue to evolve.

ccusage reads the local session logs that DeepSeek Harness (`dsh`) writes. It supports the standard daily, weekly, monthly, and session reports.

## Usage

```bash
# Daily, weekly, and monthly usage
ccusage dsh daily
ccusage dsh weekly
ccusage dsh monthly

# Usage grouped by DeepSeek Harness session
ccusage dsh session
```

Use `ccusage daily`, `ccusage weekly`, `ccusage monthly`, or `ccusage session` to include detected DeepSeek Harness data in a unified report.

## Data Locations

By default, ccusage reads session logs from:

```text
~/.dsh/sessions/<project>/<session-id>/session.jsonl
~/.dsh/sessions/<project>/<session-id>/session.v<N>.jsonl
```

Either file can also be zstd-compressed with a `.zstd` suffix. A compressed log that is still being written can end in an incomplete frame; ccusage reads the complete part and skips the rest. A log that ends in an incomplete frame and does not change while ccusage reads it, or one with a corrupt frame, is reported in a skipped-input warning, and its complete part still counts (see [Debug Options](/guide/cli-options#debug-options)).

DeepSeek Harness keeps older log generations next to the newest one after it migrates a session to a new format. ccusage reads only the newest generation in each session directory, so the same requests are not counted twice.

## Custom Data Roots

Set `DSH_HOME` to one or more comma-separated roots. Setting it replaces the default `~/.dsh` location. Each root must contain a `sessions/` directory; roots that do not exist are skipped, and a request found in more than one root is counted once.

```bash
DSH_HOME="$HOME/.dsh,/archive/dsh" ccusage dsh daily
```

## Supported Reports

| Command               | Description                             | Related Report                          |
| --------------------- | --------------------------------------- | --------------------------------------- |
| `ccusage dsh daily`   | Group usage by day                      | [Daily Usage](/guide/daily-reports)     |
| `ccusage dsh weekly`  | Group usage by week                     | [Weekly Usage](/guide/weekly-reports)   |
| `ccusage dsh monthly` | Group usage by month                    | [Monthly Usage](/guide/monthly-reports) |
| `ccusage dsh session` | Group usage by DeepSeek Harness session | [Session Usage](/guide/session-reports) |

Session rows are keyed by the session ID from the log header, such as `session-<uuid>`.

## Token and Cost Data

ccusage counts the usage recorded on assistant messages, failed or aborted assistant attempts, and compaction summaries. When a message or attempt has no settled usage, ccusage uses the last usage its stream reported, as DeepSeek Harness itself does. Assistant messages and attempts are attributed to the provider and model of the most recent request; compaction summaries use the provider and model they name. Input, output, cache read, and cache write tokens are recorded separately by DeepSeek Harness, so input tokens do not include cache tokens.

A forked session starts with a copy of its parent's log. ccusage counts only the requests made after the fork point, so a parent and its forks are not counted twice. A fork whose log was cut off before the end of the copied history is skipped, because everything in it belongs to the parent.

When copies of a request disagree, ccusage keeps the copy with the most billable tokens. If the totals match, it prefers the copy with more output tokens.

DeepSeek Harness does not log usage for the requests that generate session titles, so those requests are not included.

DeepSeek Harness does not record costs, so ccusage calculates them from token counts and known model pricing. `auto` and `calculate` produce the same result, and `display` shows `$0.00`. Requests through DeepSeek's own API-key and account providers use DeepSeek pricing, including DeepSeek's peak and off-peak rates. Requests that name another provider look up pricing for that provider's model first, then for the bare model ID, and keep that flat rate even when the bare DeepSeek model ID resolves to DeepSeek's own row. A model without known pricing remains visible with a missing-pricing warning.

For `deepseek-flash`, each request's timestamp selects the peak or off-peak token rates, including cache reads. Peak windows are Monday through Friday, 01:00–04:00 and 06:00–10:00 UTC. All other hours, including weekends, use half the peak rates. The report timezone does not change the rate. DeepSeek retired V4 Flash but still accepts `deepseek-v4-flash` and `deepseek-v4-flash-vision-exp`, so those IDs use V4 Flash rates until 04:00 UTC on September 10, 2026 and then use `deepseek-flash` rates. `deepseek-v4-pro` keeps its existing rates across that cutoff.

ccusage applies peak rates during those windows even on Chinese public holidays. Cost estimates for those hours can therefore exceed the vendor's charges. [DeepSeek pricing](https://api-docs.deepseek.com/quick_start/pricing/)

## Environment Variables

| Variable   | Description                                                                 |
| ---------- | --------------------------------------------------------------------------- |
| `DSH_HOME` | Comma-separated roots containing `sessions/`; replaces the `~/.dsh` default |

## Troubleshooting

::: details No DeepSeek Harness usage data found
Check that `~/.dsh/sessions/` contains session directories with `session.jsonl`, `session.v<N>.jsonl`, or `.zstd`-compressed versions of those files. If your data lives elsewhere, set `DSH_HOME` to the directory that contains `sessions/`.
:::
