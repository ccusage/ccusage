use crate::{CodexGroup, Result, cli_error};

pub(super) fn resolve_session_id<'a>(
    groups: &'a std::collections::BTreeMap<String, CodexGroup>,
    requested_id: &str,
) -> Result<&'a str> {
    let original_id = requested_id;
    let requested_id = normalize_session_id(original_id);
    let matches = groups
        .keys()
        .filter(|candidate| session_id_matches(candidate, requested_id))
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [session_id] => Ok(session_id.as_str()),
        [] => Err(cli_error(format!(
            "No Codex session found with ID: {original_id}"
        ))),
        _ => Err(cli_error(format!(
            "Codex session ID '{original_id}' is ambiguous and matches {} sessions.",
            matches.len()
        ))),
    }
}

pub(super) fn normalize_session_id(value: &str) -> &str {
    value
        .trim_prefix("codex://threads/")
        .trim_suffix(".zst")
        .trim_suffix(".jsonl")
}

pub(super) fn rollout_identity(value: &str) -> Option<(&str, &str)> {
    let name = normalize_session_id(value)
        .rsplit('/')
        .next()?
        .strip_prefix("rollout-")?;
    let (timestamp, ids) = name.split_at_checked(20)?;
    if !timestamp
        .bytes()
        .enumerate()
        .all(|(index, byte)| match index {
            4 | 7 | 13 | 16 | 19 => byte == b'-',
            10 => byte == b'T',
            _ => byte.is_ascii_digit(),
        })
    {
        return None;
    }
    let (thread_id, rollout_id) = ids.split_once('_').unwrap_or((ids, ids));
    (is_codex_uuid(thread_id) && is_codex_uuid(rollout_id)).then_some((thread_id, rollout_id))
}

pub(super) fn session_id_matches(candidate: &str, requested_id: &str) -> bool {
    let candidate = candidate.trim_suffix(".jsonl");
    if candidate == requested_id
        || candidate
            .rsplit_once('/')
            .map_or(candidate, |(_, file)| file)
            == requested_id
    {
        return true;
    }
    is_codex_uuid(requested_id)
        && candidate
            .strip_suffix(requested_id)
            .is_some_and(|prefix| prefix.ends_with('-'))
}

pub(super) fn is_codex_uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => byte == b'-',
            _ => byte.is_ascii_hexdigit(),
        })
}
