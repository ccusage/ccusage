use ccusage_core::{MILLIS_PER_HOUR, MILLIS_PER_MINUTE, Result, TimestampMs};
use ccusage_report::{am_pm, hour_12, zoned_parts};
use jiff::tz::TimeZone;

use super::SessionBlock;

pub(super) fn format_block_time(
    block: &SessionBlock,
    compact: bool,
    now: TimestampMs,
    zone: &TimeZone,
) -> Result<String> {
    let start = format_block_start(block.start_time, compact, zone)?;
    if block.is_gap {
        let end = format_block_end(block.end_time, compact, zone)?;
        let duration = block.end_time.duration_since(block.start_time) / MILLIS_PER_HOUR;
        return Ok(if compact {
            format!("{start}-{end}\n({duration}h gap)")
        } else {
            format!("{start} - {end} ({duration}h gap)")
        });
    }

    if block.is_active {
        let elapsed = now.duration_since(block.start_time) / MILLIS_PER_MINUTE;
        let remaining = block.end_time.duration_since(now) / MILLIS_PER_MINUTE;
        let elapsed_hours = elapsed / 60;
        let elapsed_minutes = elapsed.rem_euclid(60);
        let remaining_hours = remaining / 60;
        let remaining_minutes = remaining.rem_euclid(60);
        return Ok(if compact {
            format!(
                "{start}\n({elapsed_hours}h{elapsed_minutes}m/{remaining_hours}h{remaining_minutes}m)"
            )
        } else {
            format!(
                "{start} ({elapsed_hours}h {elapsed_minutes}m elapsed, {remaining_hours}h {remaining_minutes}m remaining)"
            )
        });
    }

    let duration = block
        .actual_end_time
        .map(|end| end.duration_since(block.start_time) / MILLIS_PER_MINUTE)
        .unwrap_or(0);
    let hours = duration / 60;
    let minutes = duration.rem_euclid(60);
    Ok(if compact {
        if hours > 0 {
            format!("{start}\n({hours}h{minutes}m)")
        } else {
            format!("{start}\n({minutes}m)")
        }
    } else if hours > 0 {
        format!("{start} ({hours}h {minutes}m)")
    } else {
        format!("{start} ({minutes}m)")
    })
}

pub(super) fn format_block_start(
    timestamp: TimestampMs,
    compact: bool,
    zone: &TimeZone,
) -> Result<String> {
    let parts = zoned_parts(timestamp, zone)?;
    Ok(if compact {
        format!(
            "{:02}/{:02}, {:02}:{:02} {}",
            parts.month,
            parts.day,
            hour_12(parts.hour),
            parts.minute,
            am_pm(parts.hour)
        )
    } else {
        format!(
            "{}/{}/{}, {}:{:02}:{:02} {}",
            parts.month,
            parts.day,
            parts.year,
            hour_12(parts.hour),
            parts.minute,
            parts.second,
            am_pm(parts.hour)
        )
    })
}

pub(super) fn format_block_end(
    timestamp: TimestampMs,
    compact: bool,
    zone: &TimeZone,
) -> Result<String> {
    if compact {
        let parts = zoned_parts(timestamp, zone)?;
        Ok(format!(
            "{:02}:{:02} {}",
            hour_12(parts.hour),
            parts.minute,
            am_pm(parts.hour)
        ))
    } else {
        format_block_start(timestamp, false, zone)
    }
}

pub(super) fn format_block_started(timestamp: TimestampMs, zone: &TimeZone) -> Result<String> {
    let parts = zoned_parts(timestamp, zone)?;
    Ok(format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        parts.year, parts.month, parts.day, parts.hour, parts.minute, parts.second
    ))
}

pub(crate) fn format_remaining_time(minutes: i64) -> String {
    let hours = minutes / 60;
    let mins = minutes % 60;
    if hours > 0 {
        format!("{hours}h {mins}m left")
    } else {
        format!("{mins}m left")
    }
}
