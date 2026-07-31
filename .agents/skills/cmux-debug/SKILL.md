---
name: cmux-debug
description: Verifies ccusage terminal output in a real cmux pane. Use when changing responsive tables, column widths, progress bars, spinners, or colors, or when a long-running command's output must be captured beyond the visible viewport.
---

# cmux Debug

Piped stdout is not a terminal: width detection, color, progress, and spinners
all take a different branch, so a non-interactive run cannot confirm a rendering
change. Run the command inside a cmux surface and read the pane back instead.

Pick a target with the listing commands (`cmux --help`, `cmux capabilities
--json`), then send and capture against that same surface — the capture is only
meaningful for the pane the command actually ran in:

```sh
cmux send --workspace <workspace> --surface <surface> "printf '\\033c'; cd <cwd>; <command>\n"
cmux capture-pane --workspace <workspace> --surface <surface> --scrollback --lines 120
```

The leading screen reset keeps the previous run's output out of the capture, and
`--scrollback` is what survives output longer than the viewport;
`cmux read-screen` without it only returns the visible screen.

For a responsive-layout bug, capture the geometry in the same send so the widths
are provably the ones the command saw:

```sh
cmux send --workspace <workspace> --surface <surface> "printf '\\033c'; stty size; printf 'COLUMNS=%s\n' \"\$COLUMNS\"; cd <cwd>; <command>\n"
```

Then check the rendered table against that width: no wrapping, and no truncated
date, model, or total columns. Repeat at a narrow width — layout regressions
show up there first.

If `capture-pane` or `read-screen` is missing from `cmux capabilities --json`,
fall back to socket RPC, which takes UUIDs rather than refs:

```sh
cmux rpc surface.read_text '{"workspace_id":"<workspace_uuid>","surface_id":"<surface_uuid>","scrollback":true,"lines":120}'
```
