---
name: cmux-debug
description: Verifies ccusage terminal output in a real cmux pane. Use when changing responsive tables, column widths, progress bars, spinners, or colors, or when a long-running command's output must be captured beyond the visible viewport.
---

# cmux Debug

Verify terminal rendering in a real cmux pane. Pass the same `--workspace` and
`--surface` to `cmux send` and `cmux capture-pane --scrollback`. Reset the screen
before the run.

For responsive tables, capture `stty size` and `COLUMNS` in the same run.
Check for wrapping and truncated date, model, or total columns. Repeat at a
narrow width.

If `cmux capabilities --json` lacks `capture-pane`, use
`cmux rpc surface.read_text` with workspace and surface UUIDs, scrollback enabled,
and a line limit.
