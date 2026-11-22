use ccusage_adapter_common::{
    claims::NativeSessions,
    json::JsonText,
    jsonl::{self, Interruption, ReadPolicy},
};
use ccusage_core::{Diagnostics, SkipKind};
use std::{
    ops::ControlFlow,
    path::Path,
    sync::{Arc, LazyLock},
};

use jiff::tz::TimeZone as JiffTimeZone;

use crate::{
    IteratedTokenUsage, Rejection, TimestampMs, TokenUsageRaw,
    cost::{PriceTable, price_resolved_usage},
    date_at,
    dedupe::{DedupEntry, DedupKey, Request},
    fast::LinePrefilter,
    parse_ts_timestamp,
    paths::extract_project,
    pricing::PricingHandle,
};

use super::{
    ClaudeEntry, DisplayModels, ParsedUsage, parse_usage_line,
    record::{ClaudeUsageRecord, is_valid_usage_record},
    response_chain::ResponseChains,
};

pub(super) const READ_POLICY: ReadPolicy = ReadPolicy {
    capacity: 256 * 1024,
    keep_interrupted_tail: |_| true,
    open_failure: Some("file open failed"),
    interrupted: |_| Interruption::KeepRecords(SkipKind::ReadInterrupted, "file read failed"),
    ..ReadPolicy::PLAIN
};

pub(crate) fn has_usage(path: &Path) -> bool {
    jsonl::read(path, None, READ_POLICY, |line, _, _| {
        if admit(line.json()).is_ok() {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    })
    .is_ok_and(|found| found.is_some())
}

pub(super) enum Refusal {
    NotUsage,
    Rejected(Rejection),
    InvalidTimestamp,
    UnrepresentableUsage,
}

pub(super) struct Admitted<'a> {
    pub(super) record: ClaudeUsageRecord<'a>,
    timestamp: TimestampMs,
    advisors: crate::AdvisorUsages,
}

fn has_usage_marker(line: &[u8]) -> bool {
    static USAGE_MARKER: LazyLock<LinePrefilter> =
        LazyLock::new(|| LinePrefilter::all(&[br#""usage":{"#]));
    USAGE_MARKER.matches(line)
}

pub(super) fn admit<'a>(json: &'a JsonText<'a>) -> std::result::Result<Admitted<'a>, Refusal> {
    let line = json.bytes();
    if !has_usage_marker(line) {
        return Err(Refusal::NotUsage);
    }
    let (record, advisor_lookup) = parse_usage_line(json).map_err(Refusal::Rejected)?;
    let timestamp = parse_ts_timestamp(&record.timestamp).ok_or(Refusal::InvalidTimestamp)?;
    if !is_valid_usage_record(&record) {
        return Err(Refusal::Rejected(Rejection::Malformed));
    }
    if ccusage_core::checked_total_usage_tokens(record.message.usage.usage, 0).is_none() {
        return Err(Refusal::UnrepresentableUsage);
    }
    let advisors = match advisor_lookup.usages(line) {
        Ok(advisors) => advisors,
        Err(Rejection::Malformed) => crate::AdvisorUsages {
            malformed: 1,
            ..crate::AdvisorUsages::default()
        },
        Err(Rejection::NullContract) => crate::AdvisorUsages::default(),
    };
    Ok(Admitted {
        record,
        timestamp,
        advisors,
    })
}

pub(super) fn read_daily_usage_file_with_diagnostics(
    path: &Path,
    projects_root: Option<&Path>,
    tz: &JiffTimeZone,
    pricing: PriceTable<'_>,
    diagnostics: &Diagnostics,
) -> Vec<ClaudeEntry> {
    let mut models = DisplayModels::default();
    read_usage_file(
        path,
        projects_root,
        tz,
        pricing,
        diagnostics,
        |parsed, _| parsed.into_daily(&mut models),
    )
}

struct FilePricing<'a> {
    table: PriceTable<'a>,
    models: smallvec::SmallVec<[(Box<str>, Option<PricingHandle>); 1]>,
}

impl FilePricing<'_> {
    fn price(
        &mut self,
        model: Option<&str>,
        usage: TokenUsageRaw,
        stored_cost: Option<f64>,
        timestamp: TimestampMs,
    ) -> (f64, Option<String>, bool) {
        let table = match self.table {
            PriceTable::Display => return (stored_cost.unwrap_or_default(), None, false),
            PriceTable::Auto(_) if stored_cost.is_some() => {
                return (stored_cost.unwrap_or_default(), None, false);
            }
            PriceTable::Auto(table) | PriceTable::Calculate(table) => table,
        };
        let Some(model) = model else {
            return (0.0, None, false);
        };
        let priced = |handle: Option<&PricingHandle>| {
            let (cost, missing) = price_resolved_usage(model, usage, timestamp, handle);
            (
                cost,
                missing,
                handle.is_some_and(PricingHandle::is_estimate),
            )
        };
        if let Some((_, handle)) = self.models.iter().find(|(name, _)| name.as_ref() == model) {
            return priced(handle.as_ref());
        }
        let handle = table.find_handle(model);
        let price = priced(handle.as_ref());
        self.models.push((model.into(), handle));
        price
    }
}

pub(crate) fn read_usage_file<E>(
    path: &Path,
    projects_root: Option<&Path>,
    tz: &JiffTimeZone,
    pricing: PriceTable<'_>,
    diagnostics: &Diagnostics,
    mut build: impl FnMut(ParsedUsage<'_>, &[u8]) -> E,
) -> Vec<E> {
    let project: Arc<str> = Arc::from(extract_project(path, projects_root));
    let mut file_pricing = FilePricing {
        table: pricing,
        models: smallvec::SmallVec::new(),
    };
    read_dedup_records(
        path,
        projects_root,
        diagnostics,
        |key, admitted, line, entries| {
            let Admitted {
                record: data,
                timestamp,
                advisors,
            } = admitted;
            let usage = data.message.usage.usage;
            let raw_model = data.message.model;
            let (cost, missing_pricing_model, estimated_pricing) =
                file_pricing.price(raw_model.as_deref(), usage, data.cost_usd, timestamp);
            let recorded_session_id = data
                .session_id
                .is_some()
                .then(|| Arc::clone(&key.session_id));
            let parent = ParsedUsage {
                key,
                is_sidechain: data.is_sidechain == Some(true),
                date: date_at(timestamp, tz),
                project: Arc::clone(&project),
                usage,
                cost,
                missing_pricing_model,
                estimated_pricing,
                stored_cost: data
                    .cost_usd
                    .filter(|_| matches!(pricing, PriceTable::Auto(_))),
                raw_model,
                timestamp_text: data.timestamp,
                version: data.version,
                recorded_session_id,
                cost_usd: data.cost_usd,
                is_api_error_message: data.is_api_error_message,
            };
            let advisors = advisors
                .usages
                .into_iter()
                .enumerate()
                .map(|(index, advisor)| {
                    let (cost, missing_pricing_model, estimated_pricing) =
                        file_pricing.price(Some(&advisor.model), advisor.usage, None, timestamp);
                    ParsedUsage {
                        key: parent.key.advisor(index),
                        is_sidechain: parent.is_sidechain,
                        date: parent.date,
                        project: Arc::clone(&project),
                        usage: advisor.usage,
                        cost,
                        missing_pricing_model,
                        estimated_pricing,
                        stored_cost: None,
                        raw_model: Some(advisor.model),
                        timestamp_text: parent.timestamp_text.clone(),
                        version: parent.version.clone(),
                        recorded_session_id: parent.recorded_session_id.clone(),
                        cost_usd: None,
                        is_api_error_message: parent.is_api_error_message,
                    }
                })
                .collect::<Vec<_>>();
            entries.push(build(parent, line));
            entries.extend(advisors.into_iter().map(|advisor| build(advisor, line)));
        },
    )
}

pub(crate) struct ClaimEntry {
    key: DedupKey,
    usage: TokenUsageRaw,
    is_sidechain: bool,
    records_cost: bool,
}

impl ClaimEntry {
    pub(crate) fn from_entry(entry: &impl DedupEntry) -> Self {
        Self {
            key: DedupKey {
                timestamp: entry.timestamp(),
                session_id: entry.owned_session_id(),
                message_id: entry.message_id().map(Box::from),
                request: entry.request().clone(),
            },
            usage: *entry.usage(),
            is_sidechain: entry.is_sidechain(),
            records_cost: entry.records_cost(),
        }
    }
}

impl DedupEntry for ClaimEntry {
    fn timestamp(&self) -> TimestampMs {
        self.key.timestamp
    }

    fn session_id(&self) -> &str {
        &self.key.session_id
    }

    fn owned_session_id(&self) -> Arc<str> {
        Arc::clone(&self.key.session_id)
    }

    fn message_id(&self) -> Option<&str> {
        self.key.message_id.as_deref()
    }

    fn request(&self) -> &Request {
        &self.key.request
    }

    fn is_sidechain(&self) -> bool {
        self.is_sidechain
    }

    fn usage(&self) -> &TokenUsageRaw {
        &self.usage
    }

    fn records_cost(&self) -> bool {
        self.records_cost
    }
}

pub(crate) fn read_claim_entries(
    path: &Path,
    projects_root: Option<&Path>,
    diagnostics: &Diagnostics,
) -> Vec<ClaimEntry> {
    read_dedup_records(
        path,
        projects_root,
        diagnostics,
        |key, admitted, _, entries| {
            let Admitted {
                record, advisors, ..
            } = admitted;
            let is_sidechain = record.is_sidechain == Some(true);
            let parent = entries.len();
            entries.push(ClaimEntry {
                key,
                usage: record.message.usage.usage,
                is_sidechain,
                records_cost: record.cost_usd.is_some(),
            });
            for (index, advisor) in advisors.usages.into_iter().enumerate() {
                entries.push(ClaimEntry {
                    key: entries[parent].key.advisor(index),
                    usage: advisor.usage,
                    is_sidechain,
                    records_cost: false,
                });
            }
        },
    )
}

pub(crate) fn find_mirror_sessions(
    path: &Path,
    projects_root: Option<&Path>,
    diagnostics: &Diagnostics,
    pending: &mut NativeSessions,
    found: &mut NativeSessions,
) {
    let (path_session, _) = crate::extract_session_parts(path, projects_root);
    let _ = jsonl::read(path, Some(diagnostics), READ_POLICY, |line, _, _| {
        match admit(line.json()) {
            Ok(admitted) if admitted.record.is_sidechain != Some(true) => {
                let session = admitted
                    .record
                    .session_id
                    .as_deref()
                    .unwrap_or(&path_session);
                if let Some(session) = pending.take(session) {
                    found.insert(session);
                }
            }
            Err(Refusal::Rejected(Rejection::Malformed)) => {
                crate::diagnostics::rejected(path, line, diagnostics);
            }
            Err(Refusal::InvalidTimestamp) => {
                crate::diagnostics::invalid_timestamp(path, line.index, diagnostics);
            }
            _ => {}
        }
        if pending.is_empty() {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    });
}

fn read_dedup_records<E>(
    path: &Path,
    projects_root: Option<&Path>,
    diagnostics: &Diagnostics,
    mut build: impl FnMut(DedupKey, Admitted<'_>, &[u8], &mut Vec<E>),
) -> Vec<E> {
    let (path_session_id, _) = crate::paths::extract_session_parts(path, projects_root);
    let path_session_id: Arc<str> = Arc::from(path_session_id);
    let mut recorded_session_id = Arc::clone(&path_session_id);
    let mut entries = Vec::new();
    let mut response_chains = ResponseChains::new(&path_session_id);
    let _ = jsonl::read(path, Some(diagnostics), READ_POLICY, |current, _, _| {
        let line = current.bytes();
        let Admitted {
            record: mut data,
            timestamp,
            advisors,
        } = match admit(current.json()) {
            Ok(admitted) => admitted,
            Err(Refusal::NotUsage) => {
                if response_chains.is_tracking() {
                    response_chains.link(line, false, !current.unterminated());
                }
                return ControlFlow::Continue(());
            }
            Err(Refusal::Rejected(Rejection::NullContract)) => {
                return ControlFlow::Continue(());
            }
            Err(Refusal::Rejected(Rejection::Malformed)) => {
                if response_chains.is_tracking() {
                    response_chains.link(line, true, !current.unterminated());
                }
                crate::diagnostics::rejected(path, current, diagnostics);
                return ControlFlow::Continue(());
            }
            Err(Refusal::InvalidTimestamp) => {
                crate::diagnostics::invalid_timestamp(path, current.index, diagnostics);
                return ControlFlow::Continue(());
            }
            Err(Refusal::UnrepresentableUsage) => {
                if response_chains.is_tracking() {
                    response_chains.link(line, true, !current.unterminated());
                }
                diagnostics.record(ccusage_core::Skip::record(
                    path,
                    ccusage_core::Skip::line_number(current.index),
                    ccusage_core::SkipKind::InvalidData,
                    "Claude token counts exceed the u64 range",
                ));
                return ControlFlow::Continue(());
            }
        };
        for _ in 0..advisors.malformed {
            diagnostics.record(ccusage_core::Skip::record(
                path,
                ccusage_core::Skip::line_number(current.index),
                ccusage_core::SkipKind::InvalidData,
                "invalid advisor usage",
            ));
        }
        let IteratedTokenUsage { usage, .. } = data.message.usage;
        let session_id = if let Some(session_id) = data.session_id.as_deref() {
            if recorded_session_id.as_ref() != session_id {
                recorded_session_id = Arc::from(session_id);
            }
            Arc::clone(&recorded_session_id)
        } else {
            Arc::clone(&path_session_id)
        };
        let response = match (&data.request_id, &data.message.id) {
            (None, Some(message_id)) => {
                response_chains.response_key(line, &session_id, message_id, &usage)
            }
            _ => None,
        };
        let key = DedupKey {
            timestamp,
            session_id,
            message_id: data.message.id.take().map(String::into_boxed_str),
            request: Request::new(data.request_id.take(), response),
        };
        build(
            key,
            Admitted {
                record: data,
                timestamp,
                advisors,
            },
            line,
            &mut entries,
        );
        ControlFlow::<()>::Continue(())
    });
    entries
}
