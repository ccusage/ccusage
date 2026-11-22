# JSON Output

ccusage supports structured JSON output for all report types, making it easy to integrate with other tools, scripts, or applications that need to process usage data programmatically.

## Enabling JSON Output

Add the `--json` (or `-j`) flag to any command:

```bash
# Daily report in JSON format
ccusage daily --json

# Monthly report in JSON format
ccusage monthly --json

# Session report in JSON format
ccusage session --json

# 5-hour blocks report in JSON format
ccusage blocks --json
```

Use `--no-cost` with `--json` when reports should omit cost data:

```bash
ccusage daily --json --no-cost
ccusage blocks --json --no-cost
```

This removes cost fields such as `totalCost`, `costUSD`, `storedCost`, and nested `cost` values while keeping token, model, date, block, burn-rate, and projection fields.

## Unpriced Models

When a model has no known price, its usage is charged at `0`. Table output prints a warning on stderr; JSON output records it in the data instead, so scripts and dashboards can tell "free" from "unknown":

- `totals.unpricedModels` lists the model names, sorted and deduplicated. The key is present only when at least one entry in the report had usage with no price, so `.totals.unpricedModels // []` is safe in `jq`.
- Each affected entry in a summary row's `modelBreakdowns` carries `"missingPricing": true`. Priced entries omit the key.

Both fields describe pricing availability, not amounts, so `--no-cost` keeps them.

```bash
ccusage daily --json | jq '.totals.unpricedModels // []'
```

In the `ccusage claude daily --json` fixture report, `mystery-model-9000` has no price. Its `modelBreakdowns` entry on 2026-01-01:

```json
{
	"cacheCreationTokens": 0,
	"cacheReadTokens": 800,
	"cost": 0.0,
	"inputTokens": 300,
	"missingPricing": true,
	"modelName": "mystery-model-9000",
	"outputTokens": 70
}
```

The report's `totals`:

```json
{
	"totals": {
		"cacheCreationTokens": 2500,
		"cacheReadTokens": 44350,
		"inputTokens": 8207,
		"outputTokens": 3659,
		"totalCost": 0.851326,
		"totalTokens": 58716,
		"unpricedModels": ["mystery-model-9000"]
	}
}
```

Detection follows the [cost mode](/guide/cost-modes): an entry counts as unpriced only when ccusage had to calculate its cost from tokens and found no price. In `display` mode, or in `auto` mode for an entry that carries its own stored cost, nothing is looked up, so nothing is reported. A model can therefore appear in `unpricedModels` and still show a positive cost when some of its entries carried stored costs. An absent key means no unpriced usage was detected under the selected mode, not that every price is current.

Covered: `daily`, `weekly`, `monthly`, and `session` reports for every agent, including `--instances`, `--sections`, and `--by-agent`. Focused `codex` reports set `missingPricing` on the affected entries of each period's `models` map. `blocks` and `session --id` have no `totals` object; Claude's `session --id` still marks unpriced entries of its `modelBreakdowns`.

Update ccusage, or run without `--offline`, once the pricing source lists the model.

## Estimated Prices

Some vendors list a model at $0 for their own free tier. When that model runs through a paid host that publishes no price of its own, such as `kyma/glm-4.7-flash`, the vendor's free tier says nothing about what the host charges. ccusage prices that usage from the closest paid listing of the same model and marks the result as an estimate. If no paid listing exists, the model is reported as [unpriced](#unpriced-models) rather than free. A provider counts as a paid host only when ccusage's pricing data shows that it charges, such as OpenRouter, Novita, Kyma, or a cloud host like Bedrock. The vendor's free tier used directly, a host's own $0 listing, a local runtime such as Ollama, and a provider that the pricing data doesn't know all still bill $0 without an estimate.

- `totals.estimatedModels` lists the estimated model names, sorted and deduplicated. The key is present only when at least one entry used an estimate, so `.totals.estimatedModels // []` is safe in `jq`.
- Each estimated entry in a summary row's `modelBreakdowns` carries `"estimatedPricing": true`. Focused `codex` reports set it on the entry in each period's `models` map instead.
- Table output prints an `INFO` note naming the estimated models on stderr.

Like the unpriced fields, these describe how a price was found, so `--no-cost` keeps them, and they follow the [cost mode](/guide/cost-modes): `display` mode looks up no prices and reports no estimates.

```bash
ccusage daily --json | jq '.totals.estimatedModels // []'
```

In a generated `ccusage codex daily --mode calculate --json` fixture report, one million input and 100,000 output tokens of `kyma/glm-4.7-flash` bill at the paid $0.08 / $0.478 per million token listing. Its `models` entry:

```json
{
	"cacheCreationTokens": 0,
	"cacheReadTokens": 0,
	"estimatedPricing": true,
	"inputTokens": 1000000,
	"isFallback": false,
	"outputTokens": 100000,
	"reasoningOutputTokens": 0,
	"totalTokens": 1100000
}
```

The report's `totals` include `"estimatedModels": ["kyma/glm-4.7-flash"]`.

Unified reports also support JSON-oriented flags for dashboard-style consumers:

```bash
ccusage daily --sections daily,weekly,monthly,session --json
ccusage daily --by-agent --json
```

`--sections` emits each requested unified section from one load. The invoked command's section is always included, and the top-level `totals` object remains the total for that invoked section. JSON keys are emitted with the invoked section first, then remaining sections in `daily`, `weekly`, `monthly`, `session` order, with `totals` last. This excerpt of `ccusage daily --sections daily,weekly,monthly,session --json` keeps one row of each section:

```json
{
	"daily": [
		{
			"agent": "all",
			"cacheCreationTokens": 0,
			"cacheReadTokens": 2000,
			"inputTokens": 1000,
			"metadata": {
				"agents": ["kimi"]
			},
			"modelBreakdowns": [
				{
					"cacheCreationTokens": 0,
					"cacheReadTokens": 2000,
					"cost": 0.00327,
					"inputTokens": 1000,
					"modelName": "kimi-for-coding",
					"outputTokens": 500
				}
			],
			"modelsUsed": ["kimi-for-coding"],
			"outputTokens": 500,
			"period": "2026-04-21",
			"totalCost": 0.00327,
			"totalTokens": 3500
		}
	],
	"weekly": [
		{
			"agent": "all",
			"cacheCreationTokens": 0,
			"cacheReadTokens": 2000,
			"inputTokens": 1000,
			"metadata": {
				"agents": ["kimi"]
			},
			"modelBreakdowns": [
				{
					"cacheCreationTokens": 0,
					"cacheReadTokens": 2000,
					"cost": 0.00327,
					"inputTokens": 1000,
					"modelName": "kimi-for-coding",
					"outputTokens": 500
				}
			],
			"modelsUsed": ["kimi-for-coding"],
			"outputTokens": 500,
			"period": "2026-04-20",
			"totalCost": 0.00327,
			"totalTokens": 3500
		}
	],
	"monthly": [
		{
			"agent": "all",
			"cacheCreationTokens": 0,
			"cacheReadTokens": 2000,
			"inputTokens": 1000,
			"metadata": {
				"agents": ["kimi"]
			},
			"modelBreakdowns": [
				{
					"cacheCreationTokens": 0,
					"cacheReadTokens": 2000,
					"cost": 0.00327,
					"inputTokens": 1000,
					"modelName": "kimi-for-coding",
					"outputTokens": 500
				}
			],
			"modelsUsed": ["kimi-for-coding"],
			"outputTokens": 500,
			"period": "2026-04",
			"totalCost": 0.00327,
			"totalTokens": 3500
		}
	],
	"session": [
		{
			"agent": "claude",
			"cacheCreationTokens": 250,
			"cacheReadTokens": 16000,
			"inputTokens": 2353,
			"metadata": {
				"lastActivity": "2026-01-02T10:00:00.000Z"
			},
			"modelBreakdowns": [
				{
					"cacheCreationTokens": 50,
					"cacheReadTokens": 9000,
					"cost": 0.5098565,
					"inputTokens": 1453,
					"modelName": "claude-sonnet-4-20250514",
					"outputTokens": 454,
					"storedCost": 0.5
				},
				{
					"cacheCreationTokens": 200,
					"cacheReadTokens": 7000,
					"cost": 0.05775,
					"inputTokens": 900,
					"modelName": "claude-opus-4-20250514",
					"outputTokens": 400
				}
			],
			"modelsUsed": ["claude-opus-4-20250514", "claude-sonnet-4-20250514"],
			"outputTokens": 854,
			"period": "sess-beta-1",
			"totalCost": 0.5676065,
			"totalTokens": 19457
		}
	],
	"totals": {
		"cacheCreationTokens": 17765,
		"cacheReadTokens": 1309867,
		"inputTokens": 1347024,
		"outputTokens": 58670,
		"totalCost": 28.146135925000006,
		"totalTokens": 2736743,
		"unpricedModels": [
			"GLM-5.3",
			"[openclaw] openclaw-mystery-1",
			"[openclaw] unknown",
			"[pi] pi-mystery-1",
			"[pi] pi-mystery-2",
			"amp-mystery-1",
			"antigravity-model-77",
			"antigravity-mystery-1",
			"claude-sonnet-4-20250514",
			"cline-mystery-1",
			"codebuff-mystery-1",
			"codex-mystery-1",
			"copilot-mystery-1",
			"droid-mystery-1",
			"dsh-mystery-1",
			"gemini-mystery-1",
			"goose-mystery-1",
			"gpt-unknown",
			"grok-mystery-1",
			"grok-unknown",
			"hermes-mystery-1",
			"kilo-mystery-1",
			"kimi-mystery-1",
			"model_placeholder_m499",
			"mystery-model-9000",
			"opencode-mystery-1",
			"qwen-mystery-1",
			"unknown",
			"zcode-mystery-1"
		]
	}
}
```

`--by-agent` adds an `agents` array to daily, weekly, and monthly rows, one entry per source with its own tokens, cost, and `modelBreakdowns`; session rows are already per-agent. A row of `ccusage daily --by-agent --json`:

```json
{
	"daily": [
		{
			"agent": "all",
			"agents": [
				{
					"agent": "kimi",
					"cacheCreationTokens": 0,
					"cacheReadTokens": 2000,
					"inputTokens": 1000,
					"modelBreakdowns": [
						{
							"cacheCreationTokens": 0,
							"cacheReadTokens": 2000,
							"cost": 0.00327,
							"inputTokens": 1000,
							"modelName": "kimi-for-coding",
							"outputTokens": 500
						}
					],
					"modelsUsed": ["kimi-for-coding"],
					"outputTokens": 500,
					"totalCost": 0.00327,
					"totalTokens": 3500
				}
			],
			"cacheCreationTokens": 0,
			"cacheReadTokens": 2000,
			"inputTokens": 1000,
			"metadata": {
				"agents": ["kimi"]
			},
			"modelBreakdowns": [
				{
					"cacheCreationTokens": 0,
					"cacheReadTokens": 2000,
					"cost": 0.00327,
					"inputTokens": 1000,
					"modelName": "kimi-for-coding",
					"outputTokens": 500
				}
			],
			"modelsUsed": ["kimi-for-coding"],
			"outputTokens": 500,
			"period": "2026-04-21",
			"totalCost": 0.00327,
			"totalTokens": 3500
		}
	]
}
```

## Stored Costs

In `auto` [cost mode](/guide/cost-modes), an entry's `cost` can add together costs stored in the agent's logs (such as Claude's `costUSD`) and costs calculated from the pricing table. Each `modelBreakdowns` entry where at least one record used its stored cost carries `storedCost`, the part of `cost` that came from stored values; `cost - storedCost` is the calculated part. Entries without stored costs omit the key, and `calculate` and `display` modes never emit it.

```json
{
	"cacheCreationTokens": 0,
	"cacheReadTokens": 0,
	"cost": 0.01,
	"inputTokens": 100,
	"modelName": "claude-sonnet-4-20250514",
	"outputTokens": 50,
	"storedCost": 0.01
}
```

## Cache Write TTL Split

[Claude Code](/guide/claude/) records whether each cache write used the 5-minute or the 1-hour cache TTL, and the two tiers are billed at different rates. Each `modelBreakdowns` entry where at least one record reported that split carries `cacheCreation1hTokens`, the part of `cacheCreationTokens` written with the 1-hour TTL; the rest of `cacheCreationTokens` is 5-minute writes plus writes from records that reported no split, which ccusage prices at the 5-minute rate when it calculates cost. Entries whose records never reported a split, including every other agent, omit the key.

```json
{
	"cacheCreation1hTokens": 400,
	"cacheCreationTokens": 600,
	"cacheReadTokens": 9000,
	"cost": 0.09675,
	"inputTokens": 500,
	"modelName": "claude-opus-4-20250514",
	"outputTokens": 800
}
```

## JSON Structure

There are two families of shapes. The unified reports, `ccusage daily`, `weekly`, `monthly`, and `session`, use one row shape keyed by `period`. A focused report such as `ccusage claude daily` uses its source's shape, keyed by `date`, `week`, `month`, or `sessionId`. Most report shapes include a `totals` object; `blocks` and `session --id` do not, and a focused Qwen report with no rows emits `totals: null`.

The samples below are real output from the test fixtures that pin ccusage's report formats. The examples are excerpts that keep whole rows; omitted rows still contribute to the displayed `totals`. Most objects use alphabetical key order; multi-section reports place the invoked section first and `totals` last.

### Unified Daily, Weekly, and Monthly Reports

`ccusage daily --json` emits a `daily` array; `weekly` and `monthly` emit `weekly` and `monthly` arrays with the same row fields. `period` is the date, the first day of the week, or the month (`YYYY-MM`). `agent` is `"all"`, and `metadata.agents` lists the sources that contributed to the row:

```json
{
	"daily": [
		{
			"agent": "all",
			"cacheCreationTokens": 0,
			"cacheReadTokens": 2000,
			"inputTokens": 1000,
			"metadata": {
				"agents": ["kimi"]
			},
			"modelBreakdowns": [
				{
					"cacheCreationTokens": 0,
					"cacheReadTokens": 2000,
					"cost": 0.00327,
					"inputTokens": 1000,
					"modelName": "kimi-for-coding",
					"outputTokens": 500
				}
			],
			"modelsUsed": ["kimi-for-coding"],
			"outputTokens": 500,
			"period": "2026-04-21",
			"totalCost": 0.00327,
			"totalTokens": 3500
		}
	],
	"totals": {
		"cacheCreationTokens": 17765,
		"cacheReadTokens": 1309867,
		"inputTokens": 1347024,
		"outputTokens": 58670,
		"totalCost": 28.146135925000006,
		"totalTokens": 2736743,
		"unpricedModels": [
			"GLM-5.3",
			"[openclaw] openclaw-mystery-1",
			"[openclaw] unknown",
			"[pi] pi-mystery-1",
			"[pi] pi-mystery-2",
			"amp-mystery-1",
			"antigravity-model-77",
			"antigravity-mystery-1",
			"claude-sonnet-4-20250514",
			"cline-mystery-1",
			"codebuff-mystery-1",
			"codex-mystery-1",
			"copilot-mystery-1",
			"droid-mystery-1",
			"dsh-mystery-1",
			"gemini-mystery-1",
			"goose-mystery-1",
			"gpt-unknown",
			"grok-mystery-1",
			"grok-unknown",
			"hermes-mystery-1",
			"kilo-mystery-1",
			"kimi-mystery-1",
			"model_placeholder_m499",
			"mystery-model-9000",
			"opencode-mystery-1",
			"qwen-mystery-1",
			"unknown",
			"zcode-mystery-1"
		]
	}
}
```

### Session Reports

`ccusage session --json` emits a `session` array. Each row is one session of one source: `agent` names the source and `period` holds the session ID. When present, `metadata.lastActivity` is an RFC 3339 timestamp. Some sources add more `metadata`, such as `projectPath`, `credits`, or `reasoningOutputTokens`.

```json
{
	"session": [
		{
			"agent": "claude",
			"cacheCreationTokens": 250,
			"cacheReadTokens": 16000,
			"inputTokens": 2353,
			"metadata": {
				"lastActivity": "2026-01-02T10:00:00.000Z"
			},
			"modelBreakdowns": [
				{
					"cacheCreationTokens": 50,
					"cacheReadTokens": 9000,
					"cost": 0.5098565,
					"inputTokens": 1453,
					"modelName": "claude-sonnet-4-20250514",
					"outputTokens": 454,
					"storedCost": 0.5
				},
				{
					"cacheCreationTokens": 200,
					"cacheReadTokens": 7000,
					"cost": 0.05775,
					"inputTokens": 900,
					"modelName": "claude-opus-4-20250514",
					"outputTokens": 400
				}
			],
			"modelsUsed": ["claude-opus-4-20250514", "claude-sonnet-4-20250514"],
			"outputTokens": 854,
			"period": "sess-beta-1",
			"totalCost": 0.5676065,
			"totalTokens": 19457
		}
	],
	"totals": {
		"cacheCreationTokens": 17865,
		"cacheReadTokens": 1311467,
		"inputTokens": 1348649,
		"outputTokens": 58942,
		"totalCost": 28.93028592500002,
		"totalTokens": 2740360,
		"unpricedModels": [
			"GLM-5.3",
			"[openclaw] openclaw-mystery-1",
			"[openclaw] unknown",
			"[pi] pi-mystery-1",
			"[pi] pi-mystery-2",
			"amp-mystery-1",
			"antigravity-model-77",
			"antigravity-mystery-1",
			"claude-sonnet-4-20250514",
			"cline-mystery-1",
			"codebuff-mystery-1",
			"codex-mystery-1",
			"copilot-mystery-1",
			"droid-mystery-1",
			"dsh-mystery-1",
			"gemini-mystery-1",
			"goose-mystery-1",
			"gpt-unknown",
			"grok-mystery-1",
			"grok-unknown",
			"hermes-mystery-1",
			"kilo-mystery-1",
			"kimi-mystery-1",
			"model_placeholder_m499",
			"mystery-model-9000",
			"opencode-mystery-1",
			"qwen-mystery-1",
			"unknown",
			"zcode-mystery-1"
		]
	}
}
```

This excerpt shows one session; `totals` covers the complete report.

`ccusage claude session --json` emits a `sessions` array instead, with `sessionId`, `projectPath`, `firstActivity`, and `lastActivity` at the top level of each row:

```json
{
	"sessions": [
		{
			"cacheCreationTokens": 250,
			"cacheReadTokens": 16000,
			"firstActivity": "2025-12-31T23:50:00.000Z",
			"inputTokens": 2353,
			"lastActivity": "2026-01-02T10:00:00.000Z",
			"modelBreakdowns": [
				{
					"cacheCreationTokens": 50,
					"cacheReadTokens": 9000,
					"cost": 0.5098565,
					"inputTokens": 1453,
					"modelName": "claude-sonnet-4-20250514",
					"outputTokens": 454,
					"storedCost": 0.5
				},
				{
					"cacheCreationTokens": 200,
					"cacheReadTokens": 7000,
					"cost": 0.05775,
					"inputTokens": 900,
					"modelName": "claude-opus-4-20250514",
					"outputTokens": 400
				}
			],
			"modelsUsed": ["claude-opus-4-20250514", "claude-sonnet-4-20250514"],
			"outputTokens": 854,
			"projectPath": "-Users-dev-beta",
			"sessionId": "sess-beta-1",
			"totalCost": 0.5676065,
			"totalTokens": 19457
		}
	],
	"totals": {
		"cacheCreationTokens": 2500,
		"cacheReadTokens": 44350,
		"inputTokens": 8207,
		"outputTokens": 3659,
		"totalCost": 0.851326,
		"totalTokens": 58716,
		"unpricedModels": ["mystery-model-9000"]
	}
}
```

This excerpt shows one session; `totals` covers the complete report.

### Claude Code Daily, Weekly, and Monthly Reports

`ccusage claude daily`, `weekly`, and `monthly` use the same row fields as the unified reports, without `agent` and `metadata`, and with `date`, `week`, or `month` in place of `period`.

### Project-Grouped Daily Reports

`ccusage claude daily --instances --json` replaces the `daily` array with a `projects` object keyed by project, and adds `project` to each row:

```json
{
	"projects": {
		"-Users-dev-alpha": [
			{
				"cacheCreationTokens": 900,
				"cacheReadTokens": 13100,
				"date": "2025-12-30",
				"inputTokens": 1550,
				"modelBreakdowns": [
					{
						"cacheCreation1hTokens": 400,
						"cacheCreationTokens": 600,
						"cacheReadTokens": 9000,
						"cost": 0.09675,
						"inputTokens": 500,
						"modelName": "claude-opus-4-20250514",
						"outputTokens": 800
					},
					{
						"cacheCreationTokens": 300,
						"cacheReadTokens": 4000,
						"cost": 0.0012,
						"inputTokens": 1000,
						"modelName": "claude-sonnet-4-20250514",
						"outputTokens": 200,
						"storedCost": 0.05
					},
					{
						"cacheCreationTokens": 0,
						"cacheReadTokens": 100,
						"cost": 0.00016,
						"inputTokens": 50,
						"modelName": "claude-haiku-4-5-20251001",
						"outputTokens": 20
					}
				],
				"modelsUsed": [
					"claude-sonnet-4-20250514",
					"claude-opus-4-20250514",
					"claude-haiku-4-5-20251001"
				],
				"outputTokens": 1020,
				"project": "-Users-dev-alpha",
				"totalCost": 0.14690999999999999,
				"totalTokens": 16570
			}
		]
	},
	"totals": {
		"cacheCreationTokens": 2500,
		"cacheReadTokens": 44350,
		"inputTokens": 8207,
		"outputTokens": 3659,
		"totalCost": 0.851326,
		"totalTokens": 58716,
		"unpricedModels": ["mystery-model-9000"]
	}
}
```

#### Usage

```bash
# Standard aggregated output
ccusage claude daily --json

# Project-grouped output
ccusage claude daily --instances --json

# Filter to specific project
ccusage claude daily --project my-frontend-app --json
```

### Blocks Reports

`ccusage blocks --json` emits a `blocks` array with one entry per 5-hour block, including gaps. See [Blocks](/guide/blocks-reports#json-output) for a sample.

## Field Descriptions

### Common Fields

- `inputTokens`: Number of input tokens consumed
- `outputTokens`: Number of output tokens generated
- `cacheCreationTokens`: Tokens used for cache creation
- `cacheReadTokens`: Tokens read from cache
- `totalTokens`: All tokens, including any unexplained remainder in the source's recorded total
- `totalCost`: Estimated cost in US dollars
- `modelsUsed`: Names of the models used
- `modelBreakdowns`: One entry per model with `modelName`, the four token counts, and `cost`, plus [`storedCost`](#stored-costs), [`cacheCreation1hTokens`](#cache-write-ttl-split), [`missingPricing`](#unpriced-models), and [`estimatedPricing`](#estimated-prices) when they apply

An unexplained remainder appears only in `totalTokens` and is excluded from [token-based cost calculations](/guide/cost-modes#token-based-calculation). The total can therefore exceed the sum of the displayed token categories.

### Report-Specific Fields

#### Unified Reports

- `period`: Date (`YYYY-MM-DD`), first day of the week, month (`YYYY-MM`), or session ID
- `agent`: `"all"` for daily, weekly, and monthly rows; the source name for session rows
- `metadata.agents`: Sources that contributed to a daily, weekly, or monthly row
- `metadata.lastActivity`: RFC 3339 timestamp of a session's last entry
- `agents`: Per-source rows, added by `--by-agent`
- Top-level `notes`: A list of messages about report scope. When a Claude project filter is active, the note explains that it applies only to Claude Code usage.

#### Claude Code Reports

- `date`, `week`, `month`: The row's period
- `project`: Project of a `--instances` row
- `sessionId`, `projectPath`: Session identity in `ccusage claude session`
- `firstActivity`, `lastActivity`: RFC 3339 timestamps of the session's first and last entries

#### Blocks Reports

- `id`, `startTime`, `endTime`: Block identity and its 5-hour window
- `actualEndTime`: Timestamp of the block's last entry
- `isActive`, `isGap`: Whether the block is running, or a gap between blocks
- `entries`: Number of usage entries in the block
- `tokenCounts`: `inputTokens`, `outputTokens`, `cacheCreationInputTokens`, and `cacheReadInputTokens`
- `costUSD`: Estimated cost of the block
- `models`: Models used in the block
- `burnRate`: `tokensPerMinute`, `tokensPerMinuteForIndicator`, and `costPerHour` of the active block, otherwise `null`
- `projection`: `totalTokens`, `totalCost`, and `remainingMinutes` the active block reaches at its current burn rate, otherwise `null`
- `tokenLimitStatus`: `limit`, `projectedUsage`, `percentUsed`, and `status` of the active block when `--token-limit` is given
- `usageLimitResetTime`: When Claude Code reported that its usage limit resets

## Filtering with JSON Output

All filtering options work with JSON output:

```bash
# Filter by date range
ccusage daily --json --since 20260510 --until 20260516

# Different cost calculation modes
ccusage monthly --json --mode calculate
ccusage session --json --mode display

# Sort order
ccusage daily --json --order asc

# Hide cost fields
ccusage daily --json --no-cost

# Project analysis
ccusage claude daily --json --instances                    # Group by project
ccusage claude daily --json --project my-project           # Filter to project
ccusage claude daily --json --instances --project my-app   # Combined usage
```

Unified and Claude summary rows include `modelBreakdowns` regardless of `--breakdown`; that flag only changes table output. Focused Codex reports instead use a `models` map and `costUSD` for each period.

## Integration Examples

### Using with jq

You can pipe JSON output to jq for advanced filtering and formatting:

```bash
# Get total cost for the last 7 days
ccusage daily --json --last 7 | jq '.totals.totalCost'

# List all unique models used
ccusage session --json | jq -r '.session[].modelsUsed[]' | sort -u

# Find the most expensive session
ccusage session --json | jq -r '.session | max_by(.totalCost) | "\(.agent) \(.period)"'

# Get daily costs as CSV
ccusage daily --json | jq -r '.daily[] | [.period, .totalCost] | @csv'

# Analyze project costs
ccusage claude daily --instances --json | jq -r '.projects | to_entries[] | [.key, (.value | map(.totalCost) | add)] | @csv'

# Find most expensive project
ccusage claude daily --instances --json | jq -r '.projects | to_entries | map({project: .key, total: (.value | map(.totalCost) | add)}) | max_by(.total) | .project'

# Get usage by project for specific date
ccusage claude daily --instances --json | jq '.projects | to_entries[] | {project: .key, usage: (.value[] | select(.date == "2026-05-16"))}'
```

`ccusage <command> --jq '<filter>'` passes the report to `jq`; the `jq` executable must be on your PATH.

### Using with Python

```python
import json
import subprocess

# Get daily usage data
result = subprocess.run(['ccusage', 'daily', '--json'], capture_output=True, text=True)
data = json.loads(result.stdout)

# Process the data
for day in data['daily']:
    print(f"Date: {day['period']}, Cost: ${day['totalCost']:.2f}")

total_cost = data['totals']['totalCost']
print(f"Total cost: ${total_cost:.2f}")

# Project analysis example
result = subprocess.run(['ccusage', 'claude', 'daily', '--instances', '--json'], capture_output=True, text=True)
project_data = json.loads(result.stdout)

for project_name, daily_entries in project_data['projects'].items():
    project_total = sum(day['totalCost'] for day in daily_entries)
    print(f"Project {project_name}: ${project_total:.2f}")

# Find highest spending project
project_totals = {
    project: sum(day['totalCost'] for day in days)
    for project, days in project_data['projects'].items()
}
if project_totals:
    top_project = max(project_totals, key=project_totals.get)
    print(f"Highest spending project: {top_project} (${project_totals[top_project]:.2f})")
```

### Using with Node.js

```javascript
import { execSync } from 'node:child_process';

// Get session usage data
const output = execSync('ccusage session --json', { encoding: 'utf-8' });
const data = JSON.parse(output);

// Find sessions over $10
const expensiveSessions = data.session.filter((session) => session.totalCost > 10);
console.log(`Found ${expensiveSessions.length} expensive sessions`);

expensiveSessions.forEach((session) => {
	console.log(`${session.agent} ${session.period}: $${session.totalCost.toFixed(2)}`);
});

// Project analysis example
const projectOutput = execSync('ccusage claude daily --instances --json', { encoding: 'utf-8' });
const projectData = JSON.parse(projectOutput);

// Calculate total cost per project
const projectCosts = Object.entries(projectData.projects).map(([name, days]) => ({
	name,
	totalCost: days.reduce((sum, day) => sum + day.totalCost, 0),
	totalTokens: days.reduce((sum, day) => sum + day.totalTokens, 0),
}));

// Sort by cost descending
projectCosts.sort((a, b) => b.totalCost - a.totalCost);

console.log('Project Usage Summary:');
projectCosts.forEach((project) => {
	console.log(
		`${project.name}: $${project.totalCost.toFixed(2)} (${project.totalTokens.toLocaleString()} tokens)`,
	);
});
```

## Programmatic Usage

JSON output is designed for programmatic consumption:

- **Consistent structure**: Token and cost fields are always present (with 0 or empty values when not applicable), except that `--no-cost` removes cost fields. A few `modelBreakdowns` keys appear only when they apply: [`missingPricing`](#unpriced-models), [`storedCost`](#stored-costs), and [`cacheCreation1hTokens`](#cache-write-ttl-split)
- **Standard types**: Numbers for metrics, strings for identifiers, arrays for lists
- **ISO timestamps**: Standardized date/time formats for reliable parsing
