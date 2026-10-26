# DeepSeek Harness (DSH) Data Source

ccusage reads local [DeepSeek Harness](https://github.com/deepseek-ai/deepseek-harness) session logs and includes DSH in focused and unified reports.

## Usage

```sh
# Daily, weekly, monthly, and session reports
ccusage dsh daily
ccusage dsh weekly
ccusage dsh monthly
ccusage dsh session

# Include DSH in an all-source report
ccusage daily
```

These commands support the usual [report options](/guide/cli-options), including `--json`, `--since`, `--until`, and `--timezone`.

## Data Location

The default DSH data directory is `~/.dsh`. Set `DSH_HOME` to use a different root:

```sh
DSH_HOME="$HOME/.dsh" ccusage dsh daily
```

Session logs are discovered under `sessions/<encoded-cwd>/<session-id>/` and may use any of these filenames, optionally compressed with `.zstd`:

```text
session.jsonl
session.v3.jsonl
session.v4.jsonl
```

For example, a log may be at `~/.dsh/sessions/<encoded-cwd>/<session-id>/session.v4.jsonl.zstd`.

## Related Reports

| Command               | Description            | Related guide                           |
| --------------------- | ---------------------- | --------------------------------------- |
| `ccusage dsh daily`   | Group usage by day     | [Daily Usage](/guide/daily-reports)     |
| `ccusage dsh weekly`  | Group usage by week    | [Weekly Usage](/guide/weekly-reports)   |
| `ccusage dsh monthly` | Group usage by month   | [Monthly Usage](/guide/monthly-reports) |
| `ccusage dsh session` | Group usage by session | [Session Usage](/guide/session-reports) |

## Environment Variables

| Variable   | Description                                      |
| ---------- | ------------------------------------------------ |
| `DSH_HOME` | Override the DSH data root; defaults to `~/.dsh` |

See [Environment Variables](/guide/environment-variables) for configuration shared by other sources.

## Troubleshooting

::: details No DSH usage data found
Check that session logs exist under `$DSH_HOME/sessions/` or `~/.dsh/sessions/`. Set `DSH_HOME` if DSH stores logs under a different root.
:::
