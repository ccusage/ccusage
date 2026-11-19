---
name: cmux-debug
description: Verifies ccusage terminal output in a real cmux pane. Use when changing responsive tables, column widths, progress bars, spinners, or colors, or when a long-running command's output must be captured beyond the visible viewport.
---

# cmux Debug

Verify terminal rendering in a real cmux pane. Select a workspace and surface
with `cmux --help` or `cmux capabilities --json`. Run the command on that
surface with `cmux send` and capture the same pane with
`cmux capture-pane --scrollback --lines 120`. Reset the screen before the run.

For responsive tables, capture `stty size` and `COLUMNS` in the same run.
Check for wrapping and truncated date, model, or total columns. Repeat at a
narrow width.

If pane capture is unavailable, use `cmux rpc surface.read_text` with workspace
and surface UUIDs, scrollback enabled, and a line limit.
