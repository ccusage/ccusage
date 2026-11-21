use ccusage_adapter_common::billing::{billing_route, host_provider, price_entry_for_provider};
use ccusage_adapter_common::{
    UnitError,
    jsonl::{self, Interruption, Line, ReadFailure, ReadPolicy},
};
use ccusage_core::cost::PriceTable;
use ccusage_core::{
    cost::{EntryPricing, UsageGranularity},
    pricing::{BillingRoute, MatchKind},
};
use std::{io, ops::ControlFlow, path::Path, sync::Arc};

use jiff::tz::TimeZone as JiffTimeZone;
use serde::Deserialize;

use crate::{
    Diagnostics, LoadedEntry, Skip, SkipKind, TimestampMs, TokenUsageRaw, date_at,
    fast::LinePrefilter,
};

use super::paths::is_compressed;
const UNKNOWN_MODEL: &str = "unknown";

#[derive(Debug, Deserialize)]
struct HeaderLine {
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    r#type: Option<String>,
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    id: Option<String>,
    #[serde(rename = "createdAt", default, deserialize_with = "jsonl::lenient_i64")]
    created_at: Option<i64>,
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    cwd: Option<String>,
    #[serde(rename = "isSeeded", default)]
    is_seeded: bool,
}

#[derive(Debug, Deserialize)]
struct EventLine {
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    r#type: Option<String>,
    #[serde(default, deserialize_with = "jsonl::lenient_i64")]
    seq: Option<i64>,
    #[serde(default, deserialize_with = "jsonl::lenient_i64")]
    time: Option<i64>,
    #[serde(default, deserialize_with = "jsonl::lenient_object")]
    data: Option<EventData>,
}

#[derive(Debug, Default, Deserialize)]
struct EventData {
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    provider: Option<String>,
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    model: Option<String>,
    #[serde(default, deserialize_with = "jsonl::lenient_object")]
    header: Option<RequestHeader>,
    #[serde(default, deserialize_with = "jsonl::lenient_object")]
    usage: Option<TokenUsage>,
    #[serde(default, deserialize_with = "jsonl::lenient_vec")]
    stream: Vec<StreamRecord>,
    #[serde(default)]
    inherited: bool,
}

impl EventData {
    // NOTE(2026-11-16, #2112): DeepSeek Harness's meter falls back to the last streamed usage chunk when an assistant event lacks settled usage. That chunk can include usage from a failed or aborted stream.
    fn settled_usage(self) -> Option<TokenUsage> {
        self.usage.or_else(|| {
            self.stream
                .into_iter()
                .rev()
                .filter(|record| record.r#type.as_deref() == Some("chunk"))
                .filter_map(|record| record.chunk)
                .find(|chunk| chunk.r#type.as_deref() == Some("usage"))
                .and_then(|chunk| chunk.usage)
        })
    }
}

#[derive(Debug, Deserialize)]
struct StreamRecord {
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    r#type: Option<String>,
    #[serde(default, deserialize_with = "jsonl::lenient_object")]
    chunk: Option<StreamChunk>,
}

#[derive(Debug, Deserialize)]
struct StreamChunk {
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    r#type: Option<String>,
    #[serde(default, deserialize_with = "jsonl::lenient_object")]
    usage: Option<TokenUsage>,
}

#[derive(Debug, Default, Deserialize)]
struct RequestHeader {
    #[serde(default, deserialize_with = "jsonl::lenient_object")]
    config: Option<Route>,
}

#[derive(Debug, Default, Deserialize)]
struct Route {
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    provider: Option<String>,
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    model: Option<String>,
}

// NOTE(2026-11-16, #2112): DeepSeek Harness emits disjoint usage counters. Its inputTokens already excludes cache reads and writes.
#[derive(Debug, Default, Deserialize)]
struct TokenUsage {
    #[serde(
        rename = "inputTokens",
        default,
        deserialize_with = "jsonl::lenient_u64"
    )]
    input_tokens: u64,
    #[serde(
        rename = "outputTokens",
        default,
        deserialize_with = "jsonl::lenient_u64"
    )]
    output_tokens: u64,
    #[serde(
        rename = "totalTokens",
        default,
        deserialize_with = "jsonl::lenient_u64"
    )]
    total_tokens: u64,
    #[serde(
        rename = "cacheReadTokens",
        default,
        deserialize_with = "jsonl::lenient_u64"
    )]
    cache_read_tokens: u64,
    #[serde(
        rename = "cacheWriteTokens",
        default,
        deserialize_with = "jsonl::lenient_u64"
    )]
    cache_write_tokens: u64,
}

#[derive(Debug, Clone)]
pub(super) struct DshUsageEntry {
    timestamp: TimestampMs,
    session_id: String,
    cwd: Option<Arc<str>>,
    seq: Option<i64>,
    provider: Option<String>,
    model: String,
    usage: TokenUsageRaw,
    residual_tokens: u64,
}

pub(super) fn read_session_file(
    path: &Path,
    diagnostics: &Diagnostics,
) -> Result<Vec<DshUsageEntry>, UnitError> {
    let mut session = SessionFold::default();
    let prefilter = LinePrefilter::any(&EVENT_MARKERS);
    let policy = if is_compressed(path) {
        ReadPolicy {
            compressed: true,
            keep_interrupted_tail: |kind| kind != io::ErrorKind::UnexpectedEof,
            interrupted: compressed_interruption,
            ..ReadPolicy::PLAIN
        }
    } else {
        ReadPolicy::PLAIN
    };
    jsonl::read(
        path,
        Some(diagnostics),
        ReadPolicy {
            stage_diagnostics: true,
            ..policy
        },
        |line, _, local| {
            if let Some(local) = local {
                session.record(line, path, local, &prefilter);
            }
            ControlFlow::<()>::Continue(())
        },
    )?;
    Ok(session.finish(path, diagnostics))
}

fn compressed_interruption(failure: ReadFailure) -> Interruption {
    let has_content = failure.complete_lines != 0
        || (failure.tail_bytes != 0 && failure.error.kind() != io::ErrorKind::UnexpectedEof);
    let kind = failure.error.kind();
    // NOTE(2026-11-16, #2112): DeepSeek Harness appends one zstd frame per write batch. A partial line at the decoded end of a torn frame belongs to the interrupted write.
    let error = if kind == io::ErrorKind::UnexpectedEof {
        failure.incomplete(if has_content {
            "zstd stream ends inside a frame; earlier records were kept"
        } else {
            "zstd stream ends inside its first frame"
        })
    } else if kind == io::ErrorKind::Other {
        UnitError::new(
            SkipKind::InvalidData,
            if has_content {
                "zstd stream is corrupt; earlier records were kept"
            } else {
                "not a valid zstd stream"
            },
        )
    } else if has_content {
        UnitError::new(
            SkipKind::ReadInterrupted,
            "zstd stream read stopped early; earlier records were kept",
        )
    } else {
        UnitError::from(failure.error)
    };
    if has_content {
        Interruption::KeepUnit(error)
    } else {
        Interruption::Reject(error)
    }
}

const EVENT_MARKERS: [&[u8]; 6] = [
    b"request/header",
    b"request/context",
    b"session/end-seed",
    b"assistant/message",
    b"assistant/attempt",
    b"compaction/summary",
];

#[derive(Default)]
enum Header {
    #[default]
    Pending,
    Rejected,
    Accepted {
        session_id: String,
        created_at: Option<TimestampMs>,
        cwd: Option<Arc<str>>,
    },
}

#[derive(Default)]
struct SessionFold {
    header: Header,
    awaiting_seed_end: bool,
    route: Route,
    entries: Vec<DshUsageEntry>,
    undated: Vec<u32>,
}

impl SessionFold {
    fn record(
        &mut self,
        line: Line<'_>,
        path: &Path,
        diagnostics: &Diagnostics,
        prefilter: &LinePrefilter,
    ) {
        if matches!(self.header, Header::Pending) {
            if !line.bytes().iter().all(u8::is_ascii_whitespace) {
                self.read_header(line, path, diagnostics);
            }
            return;
        }
        let Header::Accepted {
            session_id,
            created_at,
            cwd,
        } = &self.header
        else {
            return;
        };
        if !line.selected(Some(prefilter)) {
            return;
        }
        let Some(event) = line.record::<EventLine>(path, diagnostics, Some(is_usage_event)) else {
            return;
        };
        let number = line.number();
        let Self {
            route,
            entries,
            undated,
            awaiting_seed_end,
            ..
        } = self;
        let Some(data) = event.data else {
            return;
        };
        let usage = match event.r#type.as_deref() {
            Some("request/header") => {
                if let Some(config) = data.header.and_then(|header| header.config) {
                    update_route(route, config.provider, config.model);
                }
                return;
            }
            Some("request/context") => {
                update_route(route, data.provider, data.model);
                return;
            }
            // NOTE(2026-11-16, #2112): A DeepSeek Harness fork can carry more than one inherited marker from its ancestry. Only usage after the last marker belongs to the current session.
            Some("session/end-seed") => {
                if data.inherited {
                    entries.clear();
                    undated.clear();
                    *awaiting_seed_end = false;
                }
                return;
            }
            Some("assistant/message" | "assistant/attempt") => data
                .settled_usage()
                .map(|usage| (usage, route.provider.clone(), route.model.clone())),
            Some("compaction/summary") => {
                data.usage.map(|usage| (usage, data.provider, data.model))
            }
            _ => None,
        };
        let Some((usage, provider, model)) = usage else {
            return;
        };
        let raw_usage = TokenUsageRaw {
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cache_creation_input_tokens: usage.cache_write_tokens,
            cache_read_input_tokens: usage.cache_read_tokens,
            speed: None,
            cache_creation: None,
        };
        let residual_tokens =
            ccusage_core::AdditionalTokens::from_total(raw_usage, 0, usage.total_tokens).residual;
        if crate::total_usage_tokens(raw_usage).saturating_add(residual_tokens) == 0 {
            return;
        }
        let Some(timestamp) = event
            .time
            .filter(|millis| *millis > 0)
            .map(TimestampMs::from_millis)
            .or(*created_at)
        else {
            undated.push(number);
            return;
        };
        entries.push(DshUsageEntry {
            timestamp,
            session_id: session_id.clone(),
            cwd: cwd.clone(),
            seq: event.seq,
            provider,
            model: model.unwrap_or_else(|| UNKNOWN_MODEL.to_string()),
            usage: raw_usage,
            residual_tokens,
        });
    }

    fn read_header(&mut self, line: Line<'_>, path: &Path, diagnostics: &Diagnostics) {
        self.header = Header::Rejected;
        let Some(header) = line.record::<HeaderLine>(path, diagnostics, Some(|_| true)) else {
            return;
        };
        if header.r#type.as_deref() != Some("session") {
            diagnostics.record(Skip::record(
                path,
                line.number(),
                SkipKind::InvalidData,
                "first record is not a session header",
            ));
            return;
        }
        // NOTE(2026-11-16, #2112): DeepSeek Harness seeds a fork by copying its parent up to the inherited marker. A seeded log without that marker contains only parent usage.
        self.awaiting_seed_end = header.is_seeded;
        self.header = Header::Accepted {
            session_id: header.id.unwrap_or_else(|| session_id_from_path(path)),
            created_at: header
                .created_at
                .filter(|millis| *millis > 0)
                .map(TimestampMs::from_millis),
            cwd: header.cwd.map(Arc::from),
        };
    }

    fn finish(self, path: &Path, diagnostics: &Diagnostics) -> Vec<DshUsageEntry> {
        let Self {
            awaiting_seed_end,
            entries,
            undated,
            ..
        } = self;
        if awaiting_seed_end {
            diagnostics.debug_note(
                path,
                format!(
                    "{}: {}: seeded log has no inherited end-seed marker; read as its parent's usage",
                    diagnostics.source(),
                    path.display()
                ),
            );
            return Vec::new();
        }
        for number in undated {
            diagnostics.record(Skip::record(
                path,
                number,
                SkipKind::InvalidTimestamp,
                "request has no time and its session header no createdAt",
            ));
        }
        entries
    }
}

fn is_usage_event(value: &serde_json::Value) -> bool {
    matches!(
        value.get("type").and_then(serde_json::Value::as_str),
        Some("assistant/message" | "assistant/attempt" | "compaction/summary")
    ) && value
        .get("data")
        .is_some_and(|data| data.get("usage").is_some() || data.get("stream").is_some())
}

fn update_route(route: &mut Route, provider: Option<String>, model: Option<String>) {
    if let Some(model) = model {
        route.provider = provider;
        route.model = Some(model);
    }
}

fn session_id_from_path(path: &Path) -> String {
    path.parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or(UNKNOWN_MODEL)
        .to_string()
}

pub(super) fn dsh_entry_key(entry: &DshUsageEntry) -> String {
    let (usage, _) = entry.reported_usage();
    match entry.seq {
        Some(seq) => format!("{}:{seq}", entry.session_id),
        None => format!(
            "{}:{}:{}:{}:{}:{}:{}",
            entry.session_id,
            entry.timestamp.as_millis(),
            entry.model,
            usage.input_tokens,
            usage.output_tokens,
            usage.cache_creation_input_tokens,
            usage.cache_read_input_tokens,
        ),
    }
}

impl DshUsageEntry {
    fn reported_usage(&self) -> (TokenUsageRaw, u64) {
        ccusage_core::AdditionalTokens {
            reasoning: 0,
            residual: self.residual_tokens,
        }
        .for_report(self.usage)
    }
}

pub(super) fn dsh_entry_to_loaded(
    entry: DshUsageEntry,
    tz: &JiffTimeZone,
    pricing: PriceTable<'_>,
) -> LoadedEntry {
    let candidates = model_candidates(entry.provider.as_deref(), &entry.model);
    let candidates: Vec<_> = candidates
        .iter()
        .map(|name| {
            let kind = if *name == entry.model {
                MatchKind::Fuzzy
            } else {
                MatchKind::Exact
            };
            (name.as_str(), kind)
        })
        .collect();
    let entry_pricing = EntryPricing {
        granularity: UsageGranularity::Request,
        usage: entry.usage,
        reasoning_tokens: 0,
        residual_tokens: entry.residual_tokens,
        reported_model: Some(&entry.model),
        candidates: &candidates,
        stored_cost: None,
        timestamp: Some(entry.timestamp),
    };
    let priced = price_entry_for_provider(entry_pricing, entry.provider.as_deref(), pricing);
    let cost = priced.cost;
    let estimated_pricing = priced.estimated;
    let missing_pricing_model = priced
        .missing_pricing
        .then(|| ccusage_core::model_aliases::resolve_model_name(&entry.model).into_owned());
    let (usage, extra_total_tokens) = entry.reported_usage();
    LoadedEntry {
        date: date_at(entry.timestamp, tz),
        timestamp: entry.timestamp,
        project: Arc::from("dsh"),
        session_id: Arc::from(entry.session_id),
        project_path: entry.cwd.unwrap_or_else(|| Arc::from("DeepSeek Harness")),
        cost,
        extra_total_tokens,
        credits: None,
        message_count: None,
        model: Some(entry.model),
        usage_limit_reset_time: None,
        missing_pricing_model,
        estimated_pricing,
        usage,
        message_id: None,
        version: None,
        recorded_cost: None,
        stored_cost: None,
    }
}

fn model_candidates(provider: Option<&str>, model: &str) -> Vec<String> {
    if billing_route(provider) == Some(BillingRoute::Direct("deepseek")) {
        return vec![model.to_string(), format!("deepseek/{model}")];
    }
    let mut candidates = Vec::new();
    if let Some(provider) = provider.map(|provider| host_provider(provider).unwrap_or(provider))
        && !model.starts_with(&format!("{provider}/"))
    {
        candidates.push(format!("{provider}/{model}"));
    }
    candidates.push(model.to_string());
    candidates
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(lines: &[&str]) -> Vec<u8> {
        lines.join("\n").into_bytes()
    }

    fn parse(content: &[u8]) -> Vec<DshUsageEntry> {
        parse_diagnosed(content).0
    }

    fn parse_diagnosed(content: &[u8]) -> (Vec<DshUsageEntry>, Diagnostics) {
        let diagnostics = Diagnostics::new("dsh");
        let fixture = ccusage_test_support::Fixture::new();
        let path = fixture.path("sessions/--p--/fallback/session.v4.jsonl");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, content).unwrap();
        let entries = read_session_file(&path, &diagnostics).unwrap();
        (entries, diagnostics)
    }

    fn debug_warnings(diagnostics: &Diagnostics) -> String {
        diagnostics
            .warnings(&crate::cli::SharedArgs {
                debug: true,
                ..crate::cli::SharedArgs::default()
            })
            .join("\n")
    }

    #[test]
    fn maps_disjoint_usage_and_tracks_the_request_route() {
        let entries = parse(&lines(&[
            r#"{"type":"session","version":4,"id":"sid","createdAt":1785015039278,"cwd":"/home/u/proj","isSeeded":false,"delegationDepth":0}"#,
            r#"{"type":"request/header","seq":3,"time":1785015040000,"data":{"header":{"config":{"provider":"deepseek-official","model":"deepseek-v4-pro"},"tools":[]},"reason":"initial"}}"#,
            r#"{"type":"assistant/message","seq":5,"time":1785015045000,"data":{"turn":1,"step":1,"message":{"role":"assistant","content":[]},"stream":[{"type":"chunk","chunk":{"type":"usage","usage":{"inputTokens":999}}}],"usage":{"inputTokens":1200,"outputTokens":300,"cacheReadTokens":8000,"totalTokens":9500}}}"#,
            r#"{"type":"request/context","seq":6,"time":1785015046000,"data":{"provider":"openrouter","model":"anthropic/claude-sonnet-4.5","contextWindow":200000}}"#,
            r#"{"type":"assistant/message","seq":7,"time":1785015047000,"data":{"turn":1,"step":2,"message":{},"stream":[],"usage":{"inputTokens":10,"outputTokens":5,"cacheWriteTokens":20}}}"#,
        ]));

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].session_id, "sid");
        assert_eq!(entries[0].seq, Some(5));
        assert_eq!(entries[0].timestamp.as_millis(), 1_785_015_045_000);
        assert_eq!(entries[0].provider.as_deref(), Some("deepseek-official"));
        assert_eq!(entries[0].model, "deepseek-v4-pro");
        assert_eq!(entries[0].usage.input_tokens, 1200);
        assert_eq!(entries[0].usage.output_tokens, 300);
        assert_eq!(entries[0].usage.cache_read_input_tokens, 8000);
        assert_eq!(entries[0].usage.cache_creation_input_tokens, 0);
        assert_eq!(entries[0].residual_tokens, 0);
        assert_eq!(entries[1].provider.as_deref(), Some("openrouter"));
        assert_eq!(entries[1].model, "anthropic/claude-sonnet-4.5");
        assert_eq!(entries[1].usage.cache_creation_input_tokens, 20);
    }

    #[test]
    fn falls_back_to_the_last_stream_usage_chunk_for_attempts_and_interrupted_messages() {
        let entries = parse(&lines(&[
            r#"{"type":"session","version":4,"id":"sid","createdAt":1}"#,
            r#"{"type":"request/context","seq":1,"time":2,"data":{"provider":"deepseek-official","model":"deepseek-v4-pro"}}"#,
            r#"{"type":"assistant/attempt","seq":2,"time":3,"data":{"stream":[{"type":"chunk","chunk":{"type":"usage","usage":{"inputTokens":5,"outputTokens":0}}},{"type":"chunk","chunk":{"type":"text","text":"x"}},{"type":"chunk","chunk":{"type":"usage","usage":{"inputTokens":50,"outputTokens":3}}}]}}"#,
            r#"{"type":"assistant/message","seq":3,"time":4,"data":{"interrupted":true,"stream":[{"type":"chunk","chunk":{"type":"usage","usage":{"inputTokens":40,"outputTokens":2}}}]}}"#,
            r#"{"type":"assistant/attempt","seq":4,"time":5,"data":{"stream":[]}}"#,
        ]));
        let usage = entries
            .iter()
            .map(|entry| {
                (
                    entry.seq,
                    entry.usage.input_tokens,
                    entry.usage.output_tokens,
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(usage, [(Some(2), 50, 3), (Some(3), 40, 2)]);
    }

    #[test]
    fn drops_a_seeded_log_torn_before_its_inherited_marker() {
        let (entries, diagnostics) = parse_diagnosed(&lines(&[
            r#"{"type":"session","version":4,"id":"fork","isSeeded":true}"#,
            r#"{"type":"request/context","seq":1,"time":2,"data":{"provider":"deepseek-official","model":"deepseek-v4-pro"}}"#,
            r#"{"type":"assistant/message","seq":2,"time":3,"data":{"usage":{"inputTokens":10,"outputTokens":1}}}"#,
            r#"{"type":"assistant/message","seq":3,"data":{"usage":{"inputTokens":10,"outputTokens":1}}}"#,
        ]));

        assert!(entries.is_empty());
        assert_eq!(diagnostics.skipped(SkipKind::InvalidTimestamp), 0);
        assert!(
            debug_warnings(&diagnostics).contains("seeded log has no inherited end-seed marker"),
            "{}",
            debug_warnings(&diagnostics)
        );
    }

    #[test]
    fn counts_compaction_summaries_under_their_own_route() {
        let entries = parse(&lines(&[
            r#"{"type":"session","version":4,"id":"sid","createdAt":1}"#,
            r#"{"type":"request/context","seq":1,"time":2,"data":{"provider":"deepseek-official","model":"deepseek-v4-pro"}}"#,
            r#"{"type":"compaction/summary","seq":2,"time":3,"data":{"provider":"deepseek-account","model":"deepseek-flash","usage":{"inputTokens":40,"outputTokens":4}}}"#,
        ]));

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].provider.as_deref(), Some("deepseek-account"));
        assert_eq!(entries[0].model, "deepseek-flash");
    }

    #[test]
    fn skips_usage_inherited_by_a_fork_but_keeps_its_route() {
        let entries = parse(&lines(&[
            r#"{"type":"session","version":4,"id":"child","createdAt":1,"isSeeded":true}"#,
            r#"{"type":"request/header","seq":1,"time":2,"data":{"header":{"config":{"provider":"deepseek-official","model":"deepseek-flash"}}}}"#,
            r#"{"type":"assistant/message","seq":2,"time":3,"data":{"usage":{"inputTokens":100,"outputTokens":1}}}"#,
            r#"{"type":"session/end-seed","seq":3,"time":4,"data":{}}"#,
            r#"{"type":"assistant/message","seq":4,"time":5,"data":{"usage":{"inputTokens":200,"outputTokens":2}}}"#,
            r#"{"type":"session/end-seed","seq":5,"time":6,"data":{"inherited":true}}"#,
            r#"{"type":"assistant/message","seq":6,"time":7,"data":{"usage":{"inputTokens":300,"outputTokens":3}}}"#,
        ]));

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].seq, Some(6));
        assert_eq!(entries[0].model, "deepseek-flash");
    }

    #[test]
    fn reads_event_types_written_with_escaped_slashes() {
        let entries = parse(&lines(&[
            r#"{"type":"session","version":4,"id":"child","createdAt":1,"isSeeded":true}"#,
            r#"{"type":"request\/context","seq":1,"time":2,"data":{"provider":"deepseek-official","model":"deepseek-flash"}}"#,
            r#"{"type":"assistant/message","seq":2,"time":3,"data":{"usage":{"inputTokens":100,"outputTokens":1}}}"#,
            r#"{"type":"session\/end-seed","seq":3,"time":4,"data":{"inherited":true}}"#,
            r#"{"type":"assistant\/message","seq":4,"time":5,"data":{"usage":{"inputTokens":200,"outputTokens":2}}}"#,
        ]));

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].seq, Some(4));
        assert_eq!(entries[0].model, "deepseek-flash");
    }

    #[test]
    fn reports_a_log_without_a_session_header() {
        let (entries, diagnostics) = parse_diagnosed(
            br#"{"type":"assistant/message","seq":1,"data":{"usage":{"inputTokens":1}}}"#,
        );

        assert!(entries.is_empty());
        assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 1);
        assert!(
            debug_warnings(&diagnostics).contains("first record is not a session header"),
            "{}",
            debug_warnings(&diagnostics)
        );
    }

    #[test]
    fn reports_a_session_header_that_does_not_decode() {
        let (entries, diagnostics) = parse_diagnosed(&lines(&[
            r#"{"type":"session","id":"sid","isSeeded":"yes"}"#,
            r#"{"type":"assistant/message","seq":1,"time":2,"data":{"usage":{"inputTokens":1}}}"#,
        ]));

        assert!(entries.is_empty());
        assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 1);
    }

    #[test]
    fn reports_requests_that_do_not_decode_and_keeps_the_rest() {
        let (entries, diagnostics) = parse_diagnosed(&lines(&[
            r#"{"type":"session","id":"sid","createdAt":1}"#,
            "not json",
            r#"{"type":"assistant/message","seq":1,"seq":1,"time":2,"data":{"usage":{"inputTokens":5}}}"#,
            r#"{"type":"assistant/message","seq":2,"time":3,"data":{"usage":{"inputTok"#,
            r#"{"type":"request/context","seq":3,"seq":3,"data":{"model":"deepseek-v4-pro"}}"#,
            r#"{"type":"assistant/message","seq":4,"time":4,"data":{"usage":{"inputTokens":7}}}"#,
        ]));

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].seq, Some(4));
        assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 2);
        assert_eq!(diagnostics.skipped(SkipKind::UndecodedUsage), 1);
    }

    #[test]
    fn reports_requests_dated_by_neither_their_time_nor_the_header() {
        let (entries, diagnostics) = parse_diagnosed(&lines(&[
            r#"{"type":"session","id":"sid","createdAt":0}"#,
            r#"{"type":"assistant/message","seq":1,"data":{"usage":{"inputTokens":5}}}"#,
            r#"{"type":"assistant/message","seq":2,"time":-1,"data":{"usage":{"inputTokens":5}}}"#,
            r#"{"type":"assistant/message","seq":3,"time":7,"data":{"usage":{"inputTokens":5}}}"#,
        ]));

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].timestamp.as_millis(), 7);
        assert_eq!(diagnostics.skipped(SkipKind::InvalidTimestamp), 2);
    }

    #[test]
    fn ignores_zero_usage_and_dates_requests_by_the_header() {
        let entries = parse(&lines(&[
            r#"{"type":"session","createdAt":1700000000000}"#,
            "not json",
            r#"{"type":"assistant/message","seq":1,"data":{"usage":{"inputTokens":0,"outputTokens":0}}}"#,
            r#"{"type":"assistant/message","seq":2,"data":{"usage":{"totalTokens":7}}}"#,
        ]));

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].session_id, "fallback");
        assert_eq!(entries[0].model, UNKNOWN_MODEL);
        assert_eq!(entries[0].timestamp.as_millis(), 1_700_000_000_000);
        assert_eq!(entries[0].reported_usage().0.output_tokens, 7);
    }

    #[test]
    fn orders_pricing_candidates_by_route() {
        assert_eq!(
            model_candidates(Some("deepseek-official"), "deepseek-v4-pro"),
            ["deepseek-v4-pro", "deepseek/deepseek-v4-pro"]
        );
        assert_eq!(
            model_candidates(Some("openrouter"), "deepseek/deepseek-v4-flash"),
            [
                "openrouter/deepseek/deepseek-v4-flash",
                "deepseek/deepseek-v4-flash"
            ]
        );
        assert_eq!(
            model_candidates(Some("openrouter"), "openrouter/auto"),
            ["openrouter/auto"]
        );
        assert_eq!(model_candidates(None, "deepseek-flash"), ["deepseek-flash"]);
    }

    #[test]
    fn prices_default_deepseek_models_offline() {
        ccusage_test_support::install_pricing_snapshots();
        let pricing = ccusage_core::PricingMap::load_embedded();
        for model in ["deepseek-v4-pro", "deepseek-flash"] {
            let entry = DshUsageEntry {
                timestamp: TimestampMs::from_millis(1_785_015_045_000),
                session_id: "sid".to_string(),
                cwd: None,
                seq: Some(1),
                provider: Some("deepseek-official".to_string()),
                model: model.to_string(),
                usage: TokenUsageRaw {
                    input_tokens: 1_000_000,
                    output_tokens: 0,
                    cache_creation_input_tokens: 0,
                    cache_read_input_tokens: 0,
                    speed: None,
                    cache_creation: None,
                },
                residual_tokens: 0,
            };
            let loaded = dsh_entry_to_loaded(entry, &JiffTimeZone::UTC, PriceTable::Auto(&pricing));
            assert!(loaded.cost > 0.0, "{model} should be priced");
            assert_eq!(loaded.missing_pricing_model, None);
        }
    }

    #[test]
    fn prices_flash_tokens_by_request_time_on_api_key_and_account_routes() {
        ccusage_test_support::install_pricing_snapshots();
        let pricing = ccusage_core::PricingMap::load_embedded();
        for provider in ["deepseek-official", "deepseek-account"] {
            for timezone in [JiffTimeZone::UTC, JiffTimeZone::get("Asia/Tokyo").unwrap()] {
                for (instant, expected) in [
                    ("2026-11-18T03:00:00Z", 1.506),
                    ("2026-11-18T10:00:00Z", 0.753),
                ] {
                    let entry = DshUsageEntry {
                        timestamp: ccusage_core::parse_ts_timestamp(instant).unwrap(),
                        session_id: "sid".to_string(),
                        cwd: None,
                        seq: Some(1),
                        provider: Some(provider.to_string()),
                        model: "deepseek-flash".to_string(),
                        usage: TokenUsageRaw {
                            input_tokens: 1_000_000,
                            output_tokens: 1_000_000,
                            cache_read_input_tokens: 1_000_000,
                            ..Default::default()
                        },
                        residual_tokens: 0,
                    };
                    for mode in [PriceTable::Auto(&pricing), PriceTable::Calculate(&pricing)] {
                        let loaded = dsh_entry_to_loaded(entry.clone(), &timezone, mode);
                        assert!(
                            (loaded.cost - expected).abs() < 1e-12,
                            "{provider} at {instant}: {}",
                            loaded.cost
                        );
                        assert_eq!(loaded.missing_pricing_model, None);
                    }
                }
            }
        }
    }

    #[test]
    fn applies_the_deepseek_time_of_day_schedule_to_first_party_routes() {
        ccusage_test_support::install_pricing_snapshots();
        let pricing = ccusage_core::PricingMap::load_embedded();
        let cost_at = |provider: &str, millis: i64| {
            let entry = DshUsageEntry {
                timestamp: TimestampMs::from_millis(millis),
                session_id: "sid".to_string(),
                cwd: None,
                seq: Some(1),
                provider: Some(provider.to_string()),
                model: "deepseek-v4-pro".to_string(),
                usage: TokenUsageRaw {
                    input_tokens: 1_000_000,
                    output_tokens: 0,
                    cache_creation_input_tokens: 0,
                    cache_read_input_tokens: 0,
                    speed: None,
                    cache_creation: None,
                },
                residual_tokens: 0,
            };
            dsh_entry_to_loaded(entry, &JiffTimeZone::UTC, PriceTable::Auto(&pricing)).cost
        };
        for provider in ["deepseek-official", "deepseek-account"] {
            let peak = cost_at(provider, 1_794_880_800_000);
            let off_peak = cost_at(provider, 1_794_916_800_000);
            assert!(
                (peak - off_peak * 2.0).abs() < 1e-9,
                "{provider}: peak {peak} should be twice off-peak {off_peak}"
            );
        }
    }
}

#[cfg(test)]
mod pricing_identity_tests {
    use super::*;

    #[test]
    fn adapter_pricing_identity() {
        assert_eq!(
            model_candidates(Some("deepseek-official"), "deepseek.v4.flash"),
            ["deepseek.v4.flash", "deepseek/deepseek.v4.flash"]
        );
        assert_eq!(
            model_candidates(Some("openai"), "gpt-5.2"),
            ["openai/gpt-5.2", "gpt-5.2"]
        );
    }

    #[test]
    fn shared_route_treats_deepseek_as_direct_and_unrecorded_providers_as_inferred() {
        assert_eq!(
            billing_route(Some("deepseek")),
            Some(BillingRoute::Direct("deepseek"))
        );
        for provider in [None, Some(""), Some("unknown")] {
            assert_eq!(billing_route(provider), None, "{provider:?}");
        }
    }

    #[test]
    fn prices_direct_identity_and_reported_override_through_foreign_routes() {
        ccusage_test_support::install_pricing_snapshots();
        let pricing = ccusage_core::PricingMap::load_embedded();
        let id = "deepseek-v4-flash".to_owned();
        let patch = ccusage_core::cli::PricingOverride {
            input_cost_per_token: Some(50e-6),
            output_cost_per_token: Some(50e-6),
            ..Default::default()
        };
        let with_override = pricing.with_user_overrides([(&id, &patch)]);
        for (case, provider, model, map, expected) in [
            (
                "36_foreign_route_missing_its_qualified_price_is_static",
                "openrouter",
                "deepseek-v4-flash",
                &pricing,
                0.0015,
            ),
            (
                "81_qualified_override",
                "anthropic",
                "deepseek/deepseek-v4-flash",
                &with_override,
                0.10,
            ),
        ] {
            let entry = DshUsageEntry {
                timestamp: TimestampMs::from_millis(1_794_909_600_000),
                session_id: "s".into(),
                cwd: None,
                seq: Some(1),
                provider: Some(provider.into()),
                model: model.into(),
                usage: TokenUsageRaw {
                    input_tokens: 1_000,
                    output_tokens: 1_000,
                    ..Default::default()
                },
                residual_tokens: 0,
            };
            let loaded = dsh_entry_to_loaded(entry, &JiffTimeZone::UTC, PriceTable::Calculate(map));
            assert!(
                (loaded.cost - expected).abs() < 1e-12,
                "{case}: {}",
                loaded.cost
            );
            assert_eq!(loaded.missing_pricing_model, None, "{case}");
        }
    }
}
