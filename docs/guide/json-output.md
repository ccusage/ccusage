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

This removes cost fields such as `totalCost`, `costUSD`, and nested `cost` values while keeping token, model, date, block, burn-rate, and projection fields.

Unified reports also support JSON-oriented flags for dashboard-style consumers:

```bash
ccusage daily --sections daily,monthly,session --by-agent --json
```

`--sections` emits each requested unified section from one load. The invoked command's section is always included, and the top-level `totals` object remains the total for that invoked section. JSON keys are emitted with the invoked section first, then remaining sections in `daily`, `weekly`, `monthly`, `session` order, with `totals` last. `--by-agent` adds an `agents` array to daily, weekly, and monthly rows; session rows are already per-agent.

```json
{
	"daily": [
		{
			"agent": "all",
			"agents": [
				{
					"agent": "claude",
					"cacheCreationTokens": 20,
					"cacheReadTokens": 30,
					"inputTokens": 500,
					"modelBreakdowns": [
						{
							"cacheCreationTokens": 20,
							"cacheReadTokens": 30,
							"cost": 0.02,
							"inputTokens": 500,
							"modelName": "claude-sonnet-4-20250514",
							"outputTokens": 100
						}
					],
					"modelsUsed": ["claude-sonnet-4-20250514"],
					"outputTokens": 100,
					"totalCost": 0.02,
					"totalTokens": 650
				},
				{
					"agent": "codex",
					"cacheCreationTokens": 0,
					"cacheReadTokens": 100,
					"inputTokens": 900,
					"modelBreakdowns": [
						{
							"cacheCreationTokens": 0,
							"cacheReadTokens": 100,
							"cost": 0.0031375,
							"inputTokens": 900,
							"modelName": "gpt-5",
							"outputTokens": 200
						}
					],
					"modelsUsed": ["gpt-5"],
					"outputTokens": 200,
					"totalCost": 0.0031375,
					"totalTokens": 1200
				}
			],
			"cacheCreationTokens": 20,
			"cacheReadTokens": 130,
			"inputTokens": 1400,
			"metadata": {
				"agents": ["claude", "codex"]
			},
			"modelBreakdowns": [
				{
					"cacheCreationTokens": 20,
					"cacheReadTokens": 30,
					"cost": 0.02,
					"inputTokens": 500,
					"modelName": "claude-sonnet-4-20250514",
					"outputTokens": 100
				},
				{
					"cacheCreationTokens": 0,
					"cacheReadTokens": 100,
					"cost": 0.0031375,
					"inputTokens": 900,
					"modelName": "gpt-5",
					"outputTokens": 200
				}
			],
			"modelsUsed": ["claude-sonnet-4-20250514", "gpt-5"],
			"outputTokens": 300,
			"period": "2026-01-02",
			"totalCost": 0.023137500000000002,
			"totalTokens": 1850
		}
	],
	"monthly": [
		{
			"agent": "all",
			"agents": [
				{
					"agent": "claude",
					"cacheCreationTokens": 20,
					"cacheReadTokens": 30,
					"inputTokens": 500,
					"modelBreakdowns": [
						{
							"cacheCreationTokens": 20,
							"cacheReadTokens": 30,
							"cost": 0.02,
							"inputTokens": 500,
							"modelName": "claude-sonnet-4-20250514",
							"outputTokens": 100
						}
					],
					"modelsUsed": ["claude-sonnet-4-20250514"],
					"outputTokens": 100,
					"totalCost": 0.02,
					"totalTokens": 650
				},
				{
					"agent": "codex",
					"cacheCreationTokens": 0,
					"cacheReadTokens": 100,
					"inputTokens": 900,
					"modelBreakdowns": [
						{
							"cacheCreationTokens": 0,
							"cacheReadTokens": 100,
							"cost": 0.0031375,
							"inputTokens": 900,
							"modelName": "gpt-5",
							"outputTokens": 200
						}
					],
					"modelsUsed": ["gpt-5"],
					"outputTokens": 200,
					"totalCost": 0.0031375,
					"totalTokens": 1200
				}
			],
			"cacheCreationTokens": 20,
			"cacheReadTokens": 130,
			"inputTokens": 1400,
			"metadata": {
				"agents": ["claude", "codex"]
			},
			"modelBreakdowns": [
				{
					"cacheCreationTokens": 20,
					"cacheReadTokens": 30,
					"cost": 0.02,
					"inputTokens": 500,
					"modelName": "claude-sonnet-4-20250514",
					"outputTokens": 100
				},
				{
					"cacheCreationTokens": 0,
					"cacheReadTokens": 100,
					"cost": 0.0031375,
					"inputTokens": 900,
					"modelName": "gpt-5",
					"outputTokens": 200
				}
			],
			"modelsUsed": ["claude-sonnet-4-20250514", "gpt-5"],
			"outputTokens": 300,
			"period": "2026-01",
			"totalCost": 0.023137500000000002,
			"totalTokens": 1850
		}
	],
	"session": [
		{
			"agent": "claude",
			"cacheCreationTokens": 20,
			"cacheReadTokens": 30,
			"inputTokens": 500,
			"metadata": {
				"lastActivity": "2026-01-02T10:00:00.000Z"
			},
			"modelBreakdowns": [
				{
					"cacheCreationTokens": 20,
					"cacheReadTokens": 30,
					"cost": 0.02,
					"inputTokens": 500,
					"modelName": "claude-sonnet-4-20250514",
					"outputTokens": 100
				}
			],
			"modelsUsed": ["claude-sonnet-4-20250514"],
			"outputTokens": 100,
			"period": "claude-session",
			"totalCost": 0.02,
			"totalTokens": 650
		},
		{
			"agent": "codex",
			"cacheCreationTokens": 0,
			"cacheReadTokens": 100,
			"inputTokens": 900,
			"metadata": {
				"lastActivity": "2026-01-02T11:00:00.000Z",
				"reasoningOutputTokens": 20
			},
			"modelBreakdowns": [
				{
					"cacheCreationTokens": 0,
					"cacheReadTokens": 100,
					"cost": 0.0031375,
					"inputTokens": 900,
					"modelName": "gpt-5",
					"outputTokens": 200
				}
			],
			"modelsUsed": ["gpt-5"],
			"outputTokens": 200,
			"period": "codex-session",
			"totalCost": 0.0031375,
			"totalTokens": 1200
		}
	],
	"totals": {
		"cacheCreationTokens": 20,
		"cacheReadTokens": 130,
		"inputTokens": 1400,
		"outputTokens": 300,
		"totalCost": 0.023137500000000002,
		"totalTokens": 1850
	}
}
```

## JSON Structure

### Daily Reports (Standard)

Standard daily reports aggregate usage across all projects:

<!-- eslint-skip -->

```json
{
	"daily": [
		{
			"date": "2026-05-16",
			"inputTokens": 277,
			"outputTokens": 31456,
			"cacheCreationTokens": 512,
			"cacheReadTokens": 1024,
			"totalTokens": 33269,
			"totalCost": 17.58,
			"modelsUsed": ["claude-opus-4-1-20250805", "claude-sonnet-4-5-20250929"],
			"modelBreakdowns": [...]
		}
	],
	"totals": {
		"inputTokens": 11174,
		"outputTokens": 720366,
		"cacheCreationTokens": 896,
		"cacheReadTokens": 2304,
		"totalTokens": 734740,
		"totalCost": 336.47
	}
}
```

### Daily Reports (Project-Grouped)

When using `--instances`, daily reports group usage by project:

<!-- eslint-skip -->

```json
{
	"projects": {
		"my-frontend-app": [
			{
				"date": "2026-05-16",
				"inputTokens": 177,
				"outputTokens": 16456,
				"cacheCreationTokens": 256,
				"cacheReadTokens": 512,
				"totalTokens": 17401,
				"totalCost": 7.33,
				"modelsUsed": ["claude-sonnet-4-5-20250929"],
				"modelBreakdowns": [...]
			}
		],
		"backend-api": [
			{
				"date": "2026-05-16",
				"inputTokens": 100,
				"outputTokens": 15000,
				"cacheCreationTokens": 256,
				"cacheReadTokens": 512,
				"totalTokens": 15868,
				"totalCost": 10.25,
				"modelsUsed": ["claude-opus-4-1-20250805"],
				"modelBreakdowns": [...]
			}
		]
	},
	"totals": {
		"inputTokens": 277,
		"outputTokens": 31456,
		"cacheCreationTokens": 512,
		"cacheReadTokens": 1024,
		"totalTokens": 33269,
		"totalCost": 17.58
	}
}
```

#### Usage

```bash
# Standard aggregated output
ccusage daily --json

# Project-grouped output
ccusage daily --instances --json

# Filter to specific project
ccusage daily --project my-frontend-app --json
```

### Monthly Reports

```json
{
	"type": "monthly",
	"data": [
		{
			"month": "2026-04",
			"models": ["claude-opus-4-1-20250805", "claude-sonnet-4-5-20250929"],
			"inputTokens": 11174,
			"outputTokens": 720366,
			"cacheCreationTokens": 896,
			"cacheReadTokens": 2304,
			"totalTokens": 734740,
			"costUSD": 336.47
		}
	],
	"summary": {
		"totalInputTokens": 11174,
		"totalOutputTokens": 720366,
		"totalCacheCreationTokens": 896,
		"totalCacheReadTokens": 2304,
		"totalTokens": 734740,
		"totalCostUSD": 336.47
	}
}
```

### Session Reports

```json
{
	"type": "session",
	"data": [
		{
			"session": "session-1",
			"models": ["claude-opus-4-1-20250805", "claude-sonnet-4-5-20250929"],
			"inputTokens": 4512,
			"outputTokens": 350846,
			"cacheCreationTokens": 512,
			"cacheReadTokens": 1024,
			"totalTokens": 356894,
			"costUSD": 156.4,
			"firstActivity": "2026-05-15T09:30:00.000Z",
			"lastActivity": "2026-05-15T17:45:30.000Z"
		}
	],
	"summary": {
		"totalInputTokens": 11174,
		"totalOutputTokens": 720445,
		"totalCacheCreationTokens": 768,
		"totalCacheReadTokens": 1792,
		"totalTokens": 734179,
		"totalCostUSD": 336.68
	}
}
```

### Blocks Reports

```json
{
	"type": "blocks",
	"data": [
		{
			"blockStart": "2026-05-16T10:00:00.000Z",
			"blockEnd": "2026-05-16T15:00:00.000Z",
			"isActive": true,
			"timeRemaining": "2h 15m",
			"models": ["claude-sonnet-4-5-20250929"],
			"inputTokens": 1250,
			"outputTokens": 15000,
			"cacheCreationTokens": 256,
			"cacheReadTokens": 512,
			"totalTokens": 17018,
			"costUSD": 8.75,
			"burnRate": 2400,
			"projectedTotal": 25000,
			"projectedCost": 12.5
		}
	],
	"summary": {
		"totalInputTokens": 11174,
		"totalOutputTokens": 720366,
		"totalCacheCreationTokens": 896,
		"totalCacheReadTokens": 2304,
		"totalTokens": 734740,
		"totalCostUSD": 336.47
	}
}
```

## Field Descriptions

### Common Fields

- `models`: Array of Claude model names used
- `inputTokens`: Number of input tokens consumed
- `outputTokens`: Number of output tokens generated
- `cacheCreationTokens`: Tokens used for cache creation
- `cacheReadTokens`: Tokens read from cache
- `totalTokens`: Sum of all token types
- `costUSD`: Estimated cost in US dollars

### Report-Specific Fields

#### Daily Reports

- `date`: Date in YYYY-MM-DD format

#### Monthly Reports

- `month`: Month in YYYY-MM format

#### Session Reports

- `session`: Session identifier
- `firstActivity`: RFC 3339 timestamp of first activity in the session
- `lastActivity`: RFC 3339 timestamp of last activity in the session

#### Blocks Reports

- `blockStart`: ISO timestamp of block start
- `blockEnd`: ISO timestamp of block end
- `isActive`: Whether the block is currently active
- `timeRemaining`: Human-readable time remaining (active blocks only)
- `burnRate`: Tokens per hour rate (active blocks only)
- `projectedTotal`: Projected total tokens for the block
- `projectedCost`: Projected total cost for the block

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

# With model breakdown
ccusage daily --json --breakdown

# Hide cost fields
ccusage daily --json --no-cost

# Project analysis
ccusage daily --json --instances                    # Group by project
ccusage daily --json --project my-project           # Filter to project
ccusage daily --json --instances --project my-app   # Combined usage
```

### Model Breakdown JSON

When using `--breakdown`, the JSON includes per-model details:

```json
{
	"type": "daily",
	"data": [
		{
			"date": "2026-05-16",
			"models": ["claude-opus-4-1-20250805", "claude-sonnet-4-5-20250929"],
			"inputTokens": 277,
			"outputTokens": 31456,
			"totalTokens": 33269,
			"costUSD": 17.58,
			"breakdown": {
				"claude-opus-4-1-20250805": {
					"inputTokens": 100,
					"outputTokens": 15000,
					"cacheCreationTokens": 256,
					"cacheReadTokens": 512,
					"totalTokens": 15868,
					"costUSD": 10.25
				},
				"claude-sonnet-4-5-20250929": {
					"inputTokens": 177,
					"outputTokens": 16456,
					"cacheCreationTokens": 256,
					"cacheReadTokens": 512,
					"totalTokens": 17401,
					"costUSD": 7.33
				}
			}
		}
	]
}
```

## Integration Examples

### Using with jq

You can pipe JSON output to jq for advanced filtering and formatting:

```bash
# Get total cost for the last 7 days
ccusage daily --json --since $(date -d '7 days ago' +%Y%m%d) | jq '.summary.totalCostUSD'

# List all unique models used
ccusage session --json | jq -r '.data[].models[]' | sort -u

# Find the most expensive session
ccusage session --json | jq -r '.data | sort_by(.costUSD) | reverse | .[0].session'

# Get daily costs as CSV
ccusage daily --json | jq -r '.daily[] | [.date, .totalCost] | @csv'

# Analyze project costs
ccusage daily --instances --json | jq -r '.projects | to_entries[] | [.key, (.value | map(.totalCost) | add)] | @csv'

# Find most expensive project
ccusage daily --instances --json | jq -r '.projects | to_entries | map({project: .key, total: (.value | map(.totalCost) | add)}) | sort_by(.total) | reverse | .[0].project'

# Get usage by project for specific date
ccusage daily --instances --json | jq '.projects | to_entries[] | select(.value[].date == "2026-05-16") | {project: .key, usage: .value[0]}'
```

### Using with Python

```python
import json
import subprocess

# Get daily usage data
result = subprocess.run(['ccusage', 'daily', '--json'], capture_output=True, text=True)
data = json.loads(result.stdout)

# Process the data
for day in data['data']:
    print(f"Date: {day['date']}, Cost: ${day['costUSD']:.2f}")

total_cost = data['totals']['totalCost']
print(f"Total cost: ${total_cost:.2f}")

# Project analysis example
result = subprocess.run(['ccusage', 'daily', '--instances', '--json'], capture_output=True, text=True)
project_data = json.loads(result.stdout)

if 'projects' in project_data:
    for project_name, daily_entries in project_data['projects'].items():
        project_total = sum(day['totalCost'] for day in daily_entries)
        print(f"Project {project_name}: ${project_total:.2f}")

    # Find highest spending project
    project_totals = {
        project: sum(day['totalCost'] for day in days)
        for project, days in project_data['projects'].items()
    }
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
const expensiveSessions = data.data.filter((session) => session.costUSD > 10);
console.log(`Found ${expensiveSessions.length} expensive sessions`);

expensiveSessions.forEach((session) => {
	console.log(`${session.session}: $${session.costUSD.toFixed(2)}`);
});

// Project analysis example
const projectOutput = execSync('ccusage daily --instances --json', { encoding: 'utf-8' });
const projectData = JSON.parse(projectOutput);

if (projectData.projects) {
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
}
```

## Programmatic Usage

JSON output is designed for programmatic consumption:

- **Consistent structure**: All fields are always present (with 0 or empty values when not applicable)
- **Standard types**: Numbers for metrics, strings for identifiers, arrays for lists
- **ISO timestamps**: Standardized date/time formats for reliable parsing
- **Stable schema**: Field names and structures remain consistent across versions
