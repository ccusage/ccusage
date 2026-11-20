use std::sync::Arc;

use ccusage_core::{TimestampMs, TokenUsageRaw};

use super::{DedupEntry, DedupeIndexes, Request, merge_slots, push_deduped_entry};

struct Entry {
    session: Arc<str>,
    sidechain: bool,
    request: Request,
    usage: TokenUsageRaw,
}

impl DedupEntry for Entry {
    fn timestamp(&self) -> TimestampMs {
        TimestampMs::from_millis(0)
    }

    fn session_id(&self) -> &str {
        &self.session
    }

    fn owned_session_id(&self) -> Arc<str> {
        Arc::clone(&self.session)
    }

    fn message_id(&self) -> Option<&str> {
        Some("msg")
    }

    fn request(&self) -> &Request {
        &self.request
    }

    fn is_sidechain(&self) -> bool {
        self.sidechain
    }

    fn usage(&self) -> &TokenUsageRaw {
        &self.usage
    }

    fn records_cost(&self) -> bool {
        false
    }
}

fn entry(session: &str, sidechain: bool, input_tokens: u64) -> Entry {
    Entry {
        session: Arc::from(session),
        sidechain,
        request: Request::Missing(None),
        usage: TokenUsageRaw {
            input_tokens,
            ..TokenUsageRaw::default()
        },
    }
}

#[test]
fn dedupes_records_whose_token_buckets_exceed_a_u64() {
    let mut indexes = DedupeIndexes::default();
    let mut deduped = Vec::new();
    push_deduped_entry(
        Entry {
            usage: TokenUsageRaw {
                input_tokens: u64::MAX,
                output_tokens: 50,
                ..TokenUsageRaw::default()
            },
            ..entry("session-a", false, 0)
        },
        &mut indexes,
        &mut deduped,
    );

    push_deduped_entry(entry("session-a", false, 1), &mut indexes, &mut deduped);

    assert_eq!(deduped.len(), 1);
    assert_eq!(deduped[0].usage.input_tokens, u64::MAX);
}

#[test]
fn merging_slots_keeps_the_session_of_a_forwarded_main_thread_entry() {
    let mut indexes = DedupeIndexes::default();
    let mut deduped = vec![
        entry("session-a", false, 200),
        entry("session-b", false, 100),
    ];

    let kept = merge_slots(&mut indexes, &mut deduped, 0, 1);

    assert_eq!(kept, 0);
    assert_eq!(indexes.copies, [(0, Arc::from("session-b"))]);
}

#[test]
fn merging_slots_leaves_a_forwarded_sidechain_out_of_the_sessions() {
    let mut indexes = DedupeIndexes::default();
    let mut deduped = vec![
        entry("session-a", false, 200),
        entry("session-b", true, 100),
    ];

    merge_slots(&mut indexes, &mut deduped, 0, 1);

    assert!(indexes.copies.is_empty());
}
