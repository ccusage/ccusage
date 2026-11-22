use ccusage_core::{
    Diagnostics, MILLIS_PER_DAY, TimestampMs,
    cli::{SharedArgs, StatuslineArgs},
};

use crate::{
    ClaudeEntry,
    blocks::{SessionBlock, identify_session_blocks, starts_fresh_block},
    load_entries_since_with_diagnostics,
};

const STATUSLINE_BLOCK_LOOKBACK_DAYS: [i64; 3] = [1, 7, 30];

pub(super) fn load_statusline_active_block(
    args: &StatuslineArgs,
    shared: &SharedArgs,
    now: TimestampMs,
    diagnostics: &Diagnostics,
) -> Option<SessionBlock> {
    let session_ms = ccusage_core::cli::SessionLength::DEFAULT.as_millis();
    for lookback_days in STATUSLINE_BLOCK_LOOKBACK_DAYS {
        let window_shared = statusline_block_shared(args, shared, now, lookback_days);
        let mut entries =
            load_entries_since_with_diagnostics(&window_shared, None, diagnostics).ok()?;
        entries.sort_by_key(|entry| entry.timestamp);
        let timestamps = entries
            .iter()
            .map(|entry| entry.timestamp)
            .collect::<Vec<_>>();
        match statusline_block_anchor(&timestamps, now, session_ms) {
            BlockAnchor::NoActiveBlock => return None,
            BlockAnchor::From(start) => {
                entries.retain(|entry| entry.timestamp >= start);
                return find_active_block(entries, now);
            }
            BlockAnchor::NeedsMoreHistory => {}
            BlockAnchor::NeedsFullHistory => break,
        }
    }
    find_active_block(
        load_entries_since_with_diagnostics(shared, None, diagnostics).ok()?,
        now,
    )
}

pub(super) fn find_active_block(
    entries: Vec<ClaudeEntry>,
    now: TimestampMs,
) -> Option<SessionBlock> {
    identify_session_blocks(
        entries.into_iter().map(|entry| entry.entry).collect(),
        ccusage_core::cli::SessionLength::DEFAULT,
        now,
    )
    .into_iter()
    .find(|block| block.is_active && !block.is_gap)
}

#[derive(Debug, PartialEq)]
pub(super) enum BlockAnchor {
    NoActiveBlock,
    From(TimestampMs),
    NeedsMoreHistory,
    NeedsFullHistory,
}

pub(super) fn statusline_block_anchor(
    timestamps: &[TimestampMs],
    now: TimestampMs,
    session_ms: i64,
) -> BlockAnchor {
    let Some(&last) = timestamps.last() else {
        return BlockAnchor::NoActiveBlock;
    };
    if last > now {
        return BlockAnchor::NeedsFullHistory;
    }
    if now.duration_since(last) >= session_ms {
        return BlockAnchor::NoActiveBlock;
    }
    timestamps
        .windows(2)
        .rposition(|pair| starts_fresh_block(pair[0], pair[1], session_ms))
        .map_or(BlockAnchor::NeedsMoreHistory, |index| {
            BlockAnchor::From(timestamps[index + 1])
        })
}

pub(super) fn statusline_block_shared(
    args: &StatuslineArgs,
    shared: &SharedArgs,
    now: TimestampMs,
    lookback_days: i64,
) -> SharedArgs {
    let lookback =
        ccusage_core::cli::SessionLength::DEFAULT.as_millis() + lookback_days * MILLIS_PER_DAY;
    let since = ccusage_core::date_at(
        TimestampMs::from_millis(now.as_millis() - lookback),
        &ccusage_core::resolve_tz(args.timezone.as_ref()),
    )
    .into();
    SharedArgs {
        since: Some(since),
        timezone: args.timezone.clone(),
        ..shared.clone()
    }
}
