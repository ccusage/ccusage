use std::collections::HashMap;

use super::record::Record;

pub(crate) fn dedup(records: Vec<Record>) -> Vec<Record> {
    let mut out: Vec<Record> = Vec::with_capacity(records.len());
    // key: (message_id, request_id) -> index in out
    let mut exact: HashMap<(String, String), usize> = HashMap::new();
    // key: message_id -> index of a non-sidechain entry already kept
    let mut by_message: HashMap<String, usize> = HashMap::new();

    for r in records {
        let (Some(mid), Some(rid)) = (r.message_id.clone(), r.request_id.clone()) else {
            out.push(r);
            continue;
        };
        if exact.contains_key(&(mid.clone(), rid.clone())) {
            continue; // exact duplicate
        }
        // sidechain replay: same message id, but this one is sidechain and a non-sidechain
        // copy is already kept -> drop the replay (keep the parent).
        if r.is_sidechain {
            if let Some(&idx) = by_message.get(&mid) {
                if !out[idx].is_sidechain {
                    continue;
                }
            }
        }
        let idx = out.len();
        exact.insert((mid.clone(), rid), idx);
        if !r.is_sidechain {
            by_message.insert(mid, idx);
        }
        out.push(r);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::claude::skills::record::{Record, RecordKind};

    fn rec(mid: &str, rid: &str, side: bool) -> Record {
        Record { kind: RecordKind::Assistant, timestamp: None,
            message_id: Some(mid.into()), request_id: Some(rid.into()),
            is_sidechain: side, is_meta: false, compact: false,
            usage: None, model: None, blocks: vec![] }
    }

    #[test]
    fn drops_exact_duplicate_message_request() {
        let out = dedup(vec![rec("m1","r1",false), rec("m1","r1",false), rec("m2","r1",false)]);
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn keeps_parent_when_sidechain_replays_same_message_new_request() {
        // parent (non-sidechain) then sidechain replay of same message under a new request id
        let out = dedup(vec![rec("m1","r-parent",false), rec("m1","r-replay",true)]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].request_id.as_deref(), Some("r-parent"));
    }

    #[test]
    fn records_without_ids_are_never_dropped() {
        let mut a = rec("","",false); a.message_id=None; a.request_id=None;
        let mut b = rec("","",false); b.message_id=None; b.request_id=None;
        assert_eq!(dedup(vec![a,b]).len(), 2);
    }
}
