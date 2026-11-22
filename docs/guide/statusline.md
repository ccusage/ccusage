# Statusline Summaries

ccusage provides compact usage summaries for Claude Code and Codex. Claude Code provides hook data on stdin. Codex's native statusline reads a UI resource from ccusage's stdio MCP provider.

## Codex native statusline (experimental)

This integration was verified with the public Codex `0.171.0-alpha.13` prerelease. It requires Codex's experimental `tui_extensions_statusline` feature; Codex `0.170.0` stable does not include the native provider reader.

Add the following to your user-level `~/.codex/config.toml`, using an absolute path to the installed `ccusage` binary:

```toml
[features]
tui_extensions_statusline = true

[mcp_servers.ccusage]
command = "/absolute/path/to/ccusage"
args = ["codex", "statusline", "--mcp"]

[tui]
status_line_provider = { server = "ccusage", tool = "ccusage_codex_statusline" }
```

Merge these entries into existing tables, then restart Codex. You can also select **ccusage Codex usage** in `/statusline`. Register the provider in the user config: Codex rejects project-level MCP providers for the statusline.

Like the Claude Code statusline, the provider shows the current model and reasoning level, **session cost** and **today's cost**. Session usage includes the displayed thread's stored rollouts across days, including consumption before backtracking. Today's cost covers all local Codex threads in the selected timezone. Token counts describe recorded consumption rather than remaining context.

In auto mode, changes to a Codex home's configured service tier update fallback pricing on the next statusline refresh.

```text
🤖 GPT-6 Sol (medium) | Est. $0.23 session / $1.23 today | 15,000 tokens
```

Codex refreshes the resource approximately once per second. Log aggregation runs in the background: the provider shows a loading message initially and keeps the last completed summary while refreshing, so a slow log scan does not block native rendering. Each thread has its own cached display. ccusage reuses its existing model, service-tier, fork and pricing calculations, with embedded pricing and `pricingOverrides` and no network requests. No MCP SDK or additional runtime is bundled, and the app-only tool is excluded from model-facing tool declarations.

For a custom Codex home, set its path explicitly in the MCP server registration, as Codex does not inherit `CODEX_HOME` into MCP servers by default:

```toml
[mcp_servers.ccusage.env]
CODEX_HOME = "/absolute/path/to/your/codex-home"
```

Add `--no-cost` to the server's `args` to show session and Today token totals without prices, or `--speed <auto|standard|flex|fast>` to override the speed tier. Native mode ignores preview date filters and always shows the full session and today. A new thread without token usage has zero session usage; its Today total can include other threads. Before a thread is available, the provider clears its statusline.

### Preview in a terminal

Run `ccusage codex statusline` for a one-line preview. See [Codex statusline previews](/guide/codex/#statusline) for session selection, date filters, pricing options and JSON output. The native `--mcp` provider uses offline embedded pricing with your `pricingOverrides` and the thread selected by Codex.

## Claude Code hook statusline (Beta)

### Overview

The `statusline` command provides a compact, real-time view of your Claude Code usage, designed to integrate with Claude Code's status line hooks. It shows:

- 💬 **Current session cost** - Cost for your active conversation session
- 💰 **Today's total cost** - Your cumulative spending for the current day
- 🚀 **Current session block** - Cost and time remaining in your active 5-hour billing block
- 🔥 **Burn rate** - Token consumption rate with visual indicators
- 🤖 **Active model** - The Claude model you're currently using, with its reasoning effort level when available

### Setup

#### Configure settings.json

Add this to your `~/.claude/settings.json` or `~/.config/claude/settings.json`:

::: code-group

```json [bun x (Recommended)]
{
	"statusLine": {
		"type": "command",
		"command": "bun x ccusage statusline",
		"padding": 0
	}
}
```

```json [claude x]
{
	"statusLine": {
		"type": "command",
		"command": "BUN_BE_BUN=1 claude x ccusage statusline",
		"padding": 0
	}
}
```

```json [npx]
{
	"statusLine": {
		"type": "command",
		"command": "npx -y ccusage statusline",
		"padding": 0
	}
}
```

:::

::: tip claude x option
The `claude x` option requires the native version of Claude Code (not the npm version). If you installed Claude Code via npm, use the `bun x` or `npx` options instead.
:::

By default, statusline uses **offline mode** with cached pricing data for optimal performance.

#### Online Mode (Optional)

If you need the latest pricing data from LiteLLM API, you can explicitly enable online mode:

```json
{
	"statusLine": {
		"type": "command",
		"command": "bun x ccusage statusline --no-offline", // Fetches latest pricing from API
		"padding": 0
	}
}
```

#### With Visual Burn Rate (Optional)

You can enhance the burn rate display with visual indicators:

```json
{
	"statusLine": {
		"type": "command",
		"command": "bun x ccusage statusline --visual-burn-rate emoji", // Add emoji indicators
		"padding": 0
	}
}
```

See [Visual Burn Rate](#visual-burn-rate) section for all available options.

#### With Cost Source Options (Optional)

You can control how session costs are calculated and displayed:

```json
{
	"statusLine": {
		"type": "command",
		"command": "bun x ccusage statusline --cost-source both", // Show both CC and ccusage costs
		"padding": 0
	}
}
```

See [Cost Source Options](#cost-source-options) section for all available modes.

### Output Format

The statusline displays a compact, single-line summary. For the hook payload in the test fixtures, `ccusage statusline` prints:

```text
🤖 Sonnet 4 (high) | 💰 $1.23 session / $0.06 today / $0.06 block (1h 0m left) | 🔥 $0.02/hr | 🧠 50,000 (25%)
```

When using `--cost-source both`, the session cost shows both Claude Code and ccusage calculations:

```text
🤖 Sonnet 4 (high) | 💰 ($1.23 cc / $0.19 ccusage) session / $0.06 today / $0.06 block (1h 0m left) | 🔥 $0.02/hr | 🧠 50,000 (25%)
```

The reasoning effort level next to the model name comes from Claude Code (2.1.119+). All current Claude models report it; for older Claude Code versions or models without the effort parameter, the model segment is just the model name, such as `🤖 Opus 4.1`.

#### Components Explained

- **Model** (`🤖 Sonnet 4 (high)`): Currently active Claude model, followed by the reasoning effort level in parentheses (`low`, `medium`, `high`, `xhigh`, or `max`) when Claude Code provides it
- **Session Cost** (`💰 $1.23 session`): Cost for the current conversation session (see [Cost Source Options](#cost-source-options) for different calculation modes)
- **Today's Cost** (`$0.06 today`): Total cost for the current day across all sessions
- **Session Block** (`$0.06 block (1h 0m left)`): Current 5-hour block cost with remaining time
- **Burn Rate** (`🔥 $0.02/hr`): Cost burn rate per hour with color-coded indicators:
  - Green text: Normal (< 2,000 tokens/min)
  - Yellow text: Moderate (2,000-5,000 tokens/min)
  - Red text: High (> 5,000 tokens/min)
  - Optional visual status indicators (see [Visual Burn Rate](#visual-burn-rate))
- **Context Usage** (`🧠 50,000 (25%)`): Shows input tokens with percentage of context limit:
  - Green text: Low usage (< 50% by default)
  - Yellow text: Medium usage (50-80% by default)
  - Red text: High usage (> 80% by default)
  - Uses Claude Code's [`context_window` data](https://code.claude.com/docs/en/statusline) when available for accurate token counts

When no block is active, the block segment reads `No active block` and the burn rate segment is left out; the context segment is still printed, as `🧠 N/A` when no context size is known.

### Technical Details

The statusline command:

- Reads session information from stdin (provided by Claude Code hooks)
- Identifies the active 5-hour billing block
- Calculates real-time burn rates and projections
- Outputs a single line suitable for status bar display
- **Uses offline mode by default** for instant response times without network dependencies
- Can be configured to use online mode with `--no-offline` for latest pricing data

### Beta Notice

⚠️ This feature is currently in **beta**. More customization options and features are coming soon:

- Custom format templates
- Configurable burn rate thresholds
- Additional metrics display options
- Session-specific cost tracking

#### Cost Source Options

The `--cost-source` option controls how session costs are calculated and displayed:

**Available modes:**

- `auto` (default): Prefer Claude Code's pre-calculated cost when available, fallback to ccusage calculation
- `ccusage`: Use ccusage's report cost with the selected `--mode`
- `cc`: Always use Claude Code's pre-calculated cost from session data
- `both`: Display both Claude Code and ccusage costs side by side for comparison

**Command-line usage:**

```bash
# Default auto mode
bun x ccusage statusline

# Always use ccusage calculation
bun x ccusage statusline --cost-source ccusage

# Always use Claude Code cost
bun x ccusage statusline --cost-source cc

# Show both costs for comparison
bun x ccusage statusline --cost-source both
```

**Settings.json configuration:**

```json
{
	"statusLine": {
		"type": "command",
		"command": "bun x ccusage statusline --cost-source both",
		"padding": 0
	}
}
```

**When to use each mode:**

- **`auto`**: Best for most users, provides accurate costs with fallback reliability
- **`ccusage`**: When you want consistent calculation methods across all ccusage commands
- **`cc`**: When you trust Claude Code's cost calculations and want minimal processing
- **`both`**: For debugging cost discrepancies or comparing calculation methods

**Output differences:**

The ccusage portion honors the configured `defaults.mode` (or `--mode`): `display` uses recorded costs, `calculate` prices tokens, and `auto` uses the report's stored-cost preference. `--cost-source` independently chooses whether to show Claude Code's value, ccusage's value, or both.

- **Single cost modes** (`auto`, `ccusage`, `cc`): `💰 $1.23 session`
- **Both mode**: `💰 ($1.23 cc / $0.19 ccusage) session`

### Configuration

#### Context Usage Thresholds

You can customize the context usage color thresholds using command-line options or configuration files:

- `--context-low-threshold` - Percentage below which context usage is shown in green (default: 50)
- `--context-medium-threshold` - Percentage below which context usage is shown in yellow (default: 80)

**Validation and Safety Features:**

- Values must be integers from 0 to 255
- The `LOW` threshold must be less than the `MEDIUM` threshold
- Thresholds are validated together after configuration and command-line overrides are merged. For example, a configured low threshold of `85` works with `--context-medium-threshold 95`.
- An invalid field in a discovered config is dropped with a warning while valid settings are retained. An invalid explicit `--config` file is an error.

**Command-line usage:**

```bash
bun x ccusage statusline --context-low-threshold 60 --context-medium-threshold 90
```

**Configuration file usage:**
You can also set these options in your configuration file. See the [Configuration Guide](/guide/configuration) for more details.

With these settings:

- Green: < 60%
- Yellow: 60-90%
- Red: > 90%

**Example usage in Claude Code settings:**

```json
{
	"command": "bun x ccusage statusline --context-low-threshold 60 --context-medium-threshold 90",
	"timeout": 5000
}
```

#### Visual Burn Rate

You can enhance the burn rate display with visual status indicators using the `--visual-burn-rate` option:

```bash
# Add to your settings.json command
bun x ccusage statusline --visual-burn-rate emoji
```

**Available options:**

- `off` (default): No visual indicators, only colored text
- `emoji`: Add emoji indicators (🟢/⚠️/🚨)
- `text`: Add text status in parentheses (Normal/Moderate/High)
- `emoji-text`: Combine both emoji and text indicators

**Examples:**

```bash
# Default (off)
🔥 $0.12/hr

# With emoji
🔥 $0.12/hr 🟢

# With text
🔥 $0.12/hr (Normal)

# With both emoji and text
🔥 $0.12/hr 🟢 (Normal)
```

**Status Indicators:**

- 🟢 Normal (Green)
- ⚠️ Moderate (Yellow)
- 🚨 High (Red)

#### Model Label Aliases

Some setups report very long model identifiers. For example, AWS Bedrock
inference profiles surface full ARNs such as
`arn:aws:bedrock:ap-northeast-1:012345678910:application-inference-profile/abcde12345`,
which gets truncated in the statusline and hides which model is in use.

Use `modelLabelAliases` in your configuration file to map a model identifier to
a short display label. When the active model matches a key, the statusline shows
the alias instead:

```json
{
	"commands": {
		"statusline": {
			"modelLabelAliases": {
				"arn:aws:bedrock:ap-northeast-1:012345678910:application-inference-profile/abcde12345": "claude-opus-4-6"
			}
		}
	}
}
```

With the alias above, the statusline changes from:

```text
🤖 arn:aws:bedrock:ap-northeast-1:012345678910:application-inference-profile/abcde12345 | 💰 ...
```

to:

```text
🤖 claude-opus-4-6 | 💰 ...
```

Keys are matched exactly against the model's display name. Models without a
matching alias are shown unchanged. See the [Configuration Guide](/guide/configuration)
for more details.

### Troubleshooting

#### No Output Displayed

If the statusline doesn't show:

1. Verify `ccusage` is in your PATH
2. Check Claude Code logs for any errors
3. Ensure you have valid usage data in your Claude data directory

#### Incorrect Costs

If costs seem incorrect:

- The command uses the same cost calculation as other ccusage commands
- Verify with `ccusage daily` or `ccusage blocks` for detailed breakdowns

### Related Commands

- [`blocks`](./blocks-reports.md) - Detailed 5-hour billing block analysis
- [`daily`](./daily-reports.md) - Daily usage reports
- [`session`](./session-reports.md) - Session-based usage analysis
