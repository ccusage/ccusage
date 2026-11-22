use std::collections::HashMap;

use super::TableSchema;

pub(super) fn fork_copy_cutoffs(
    connection: &sqlite::Connection,
    session_v2: &TableSchema,
    session_message: &TableSchema,
) -> HashMap<String, i64> {
    let mut cutoffs = HashMap::new();
    if !session_v2.has_all(&["id", "fork_session_id", "fork_boundary"])
        || !session_message.has_all(&["id", "session_id", "seq"])
    {
        return cutoffs;
    }
    let Ok(mut forks) = connection.prepare(
        "SELECT id, fork_session_id, fork_boundary FROM session_v2 \
         WHERE fork_session_id IS NOT NULL AND fork_boundary IS NOT NULL",
    ) else {
        return cutoffs;
    };
    while matches!(forks.next(), Ok(sqlite::State::Row)) {
        let Ok(fork_id) = forks.read::<String, _>(0) else {
            continue;
        };
        let Ok(parent_id) = forks.read::<String, _>(1) else {
            continue;
        };
        let Ok(boundary) = forks.read::<String, _>(2) else {
            continue;
        };
        if fork_id.is_empty() || parent_id.is_empty() {
            continue;
        }
        if let Some(cutoff) = fork_copy_cutoff(connection, &parent_id, &boundary) {
            cutoffs.insert(fork_id, cutoff);
        }
    }
    cutoffs
}

fn fork_copy_cutoff(
    connection: &sqlite::Connection,
    parent_id: &str,
    boundary: &str,
) -> Option<i64> {
    let boundary: serde_json::Value = serde_json::from_str(boundary).ok()?;
    let boundary_type = boundary.get("type")?.as_str()?;
    // NOTE(2026-09-25, OpenCode 2.0.16, #1782): OpenCode fork copies get new message IDs and preserve seq. Resolve the boundary ID in the parent session.
    let parent_message_id = boundary.get("messageID")?.as_str()?;
    let Ok(mut statement) = connection
        .prepare("SELECT seq FROM session_message WHERE session_id = ?1 AND id = ?2 LIMIT 1")
    else {
        return None;
    };
    statement.bind((1, parent_id)).ok()?;
    statement.bind((2, parent_message_id)).ok()?;
    let boundary_seq = match statement.next() {
        Ok(sqlite::State::Row) => statement.read::<Option<i64>, _>(0).ok().flatten()?,
        _ => return None,
    };
    match boundary_type {
        "through" => Some(boundary_seq),
        // NOTE(2026-09-25, OpenCode 2.0.16, #1782): OpenCode seq values can be sparse, and fork-local rows follow the last copy. Use the preceding parent seq so the cutoff keeps fork-local rows.
        "before" => last_parent_seq_before(connection, parent_id, boundary_seq),
        _ => None,
    }
}

fn last_parent_seq_before(
    connection: &sqlite::Connection,
    parent_id: &str,
    boundary_seq: i64,
) -> Option<i64> {
    let Ok(mut statement) = connection
        .prepare("SELECT MAX(seq) FROM session_message WHERE session_id = ?1 AND seq < ?2")
    else {
        return None;
    };
    statement.bind((1, parent_id)).ok()?;
    statement.bind((2, boundary_seq)).ok()?;
    match statement.next() {
        Ok(sqlite::State::Row) => statement.read::<Option<i64>, _>(0).ok()?,
        _ => None,
    }
}
