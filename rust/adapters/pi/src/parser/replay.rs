use std::collections::{HashMap, HashSet};

use super::{PiEntryLink, PiReplayUsagePath, PiUsageRecord, PiUsageSignature};
use crate::cli::CostMode;

pub(super) fn matching_replay_prefix<'a>(
    child_usage: impl Iterator<Item = &'a PiUsageSignature>,
    parent_usage: impl Iterator<Item = &'a PiUsageSignature>,
    fork_timestamp: crate::TimestampMs,
) -> usize {
    child_usage
        .zip(parent_usage.take_while(|usage| usage.timestamp <= fork_timestamp))
        .take_while(|(child, parent)| child == parent)
        .count()
}

pub(super) fn replay_usage_path(
    links: Vec<PiEntryLink>,
    usage: &[PiUsageRecord],
) -> PiReplayUsagePath {
    // NOTE(2026-08-29, #1662): Version 1 Pi sessions predate entry links. Their physical order is the only available replay history.
    if links
        .iter()
        .all(|link| link.id.is_none() && link.parent_id.is_none())
    {
        return PiReplayUsagePath::Linear;
    }

    let mut parents_by_id = HashMap::new();
    let mut leaf_id = None;
    for link in links {
        let Some(id) = link.id else {
            return PiReplayUsagePath::Invalid;
        };
        if parents_by_id.insert(id.clone(), link.parent_id).is_some() {
            return PiReplayUsagePath::Invalid;
        }
        leaf_id = Some(id);
    }
    // NOTE(2026-08-29, #1662): Pi writes the current leaf last and leaves abandoned siblings earlier in the file. Follow the leaf's parents to identify the replayable branch.
    let Some(mut entry_id) = leaf_id else {
        return PiReplayUsagePath::Linear;
    };
    if parents_by_id
        .values()
        .flatten()
        .any(|parent| !parents_by_id.contains_key(parent))
        || has_cyclic_entry_links(&parents_by_id)
    {
        return PiReplayUsagePath::Invalid;
    }

    let usage_by_id = usage
        .iter()
        .enumerate()
        .filter_map(|(index, usage)| usage.entry_id.as_deref().map(|id| (id, index)))
        .collect::<HashMap<_, _>>();
    let mut active_usage = Vec::new();
    let mut visited = HashSet::new();
    loop {
        if !visited.insert(entry_id.clone()) {
            return PiReplayUsagePath::Invalid;
        }
        if let Some(&usage_index) = usage_by_id.get(entry_id.as_str()) {
            active_usage.push(usage_index);
        }
        let Some(parent_id) = parents_by_id.get(&entry_id) else {
            return PiReplayUsagePath::Invalid;
        };
        let Some(parent_id) = parent_id else {
            break;
        };
        entry_id = parent_id.clone();
    }
    active_usage.reverse();
    PiReplayUsagePath::Active(active_usage)
}

fn has_cyclic_entry_links(parents_by_id: &HashMap<String, Option<String>>) -> bool {
    let mut validated = HashSet::new();
    for start in parents_by_id.keys() {
        if validated.contains(start) {
            continue;
        }

        let mut path = HashSet::new();
        let mut entry_id = start.clone();
        while !validated.contains(&entry_id) {
            if !path.insert(entry_id.clone()) {
                return true;
            }
            let Some(parent_id) = parents_by_id.get(&entry_id) else {
                return true;
            };
            let Some(parent_id) = parent_id else {
                break;
            };
            entry_id = parent_id.clone();
        }
        validated.extend(path);
    }
    false
}

pub(super) fn replay_billed_cost_bits(mode: CostMode, cost: f64) -> Option<u64> {
    if mode == CostMode::Calculate {
        return None;
    }
    Some(if cost == 0.0 {
        0.0_f64.to_bits()
    } else {
        cost.to_bits()
    })
}
