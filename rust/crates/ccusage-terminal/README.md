# ccusage-terminal

Terminal rendering primitives: width detection, color handling, box titles, and
the table writer every report uses.

## Owns

- `table.rs` — `SimpleTable`, column alignment, terminal-width fitting, and date
  compaction.
- `style.rs` — `TerminalStyle`, color selection, and `NO_COLOR` handling.
- `terminal.rs` — terminal width detection.
- `title.rs` — the boxed report titles.
- `width.rs` — ANSI-aware display width measurement and truncation.

`ccusage-core` re-exports this crate's types, so adapters reach them through
`ccusage_core` rather than depending on it directly.

## Public surface

- `style::Color`
- `style::TerminalStyle`
- `style::color`
- `table::Align`
- `table::SimpleTable`
- `terminal::terminal_width`
- `title::print_box_title`
- `width::truncate_to_width`

## Depends on

- `terminal_size`
- `unicode-width`

## Build layer

Built in the `foundation` Crane artifact layer, so a change here recompiles every adapter.
