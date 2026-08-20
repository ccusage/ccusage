use std::{collections::HashSet, sync::Arc};

use jiff::tz::TimeZone as JiffTimeZone;

use crate::{
    LoadedEntry, PricingMap, TimestampMs, TokenUsageRaw, UsageEntry, UsageMessage,
    calculate_cost_for_usage, cli::CostMode, format_date_tz, format_rfc3339_millis,
    missing_pricing_model_for_candidates,
};

pub(super) const DEFAULT_ANTIGRAVITY_MODEL: &str = "gemini-3.7-flash";

#[derive(Clone, Debug, Default)]
pub(super) struct RawTokenStats {
    pub(super) input_tokens: u64,
    pub(super) output_tokens: u64,
    pub(super) cache_read_tokens: u64,
    pub(super) reasoning_tokens: u64,
    pub(super) generation_tokens: u64,
}

impl RawTokenStats {
    pub(super) fn is_zero(&self) -> bool {
        self.input_tokens == 0
            && self.output_tokens == 0
            && self.cache_read_tokens == 0
            && self.reasoning_tokens == 0
            && self.generation_tokens == 0
    }
}

pub(super) struct AntigravityEntry {
    pub(super) timestamp: TimestampMs,
    pub(super) timestamp_text: String,
    pub(super) session_id: String,
    pub(super) model: String,
    pub(super) usage: TokenUsageRaw,
    pub(super) reasoning_tokens: u64,
    pub(super) message_count: u64,
}

pub(super) struct ProtoReader<'a> {
    data: &'a [u8],
    cursor: usize,
}

impl<'a> ProtoReader<'a> {
    pub(super) fn new(data: &'a [u8]) -> Self {
        Self { data, cursor: 0 }
    }

    pub(super) fn read_varint(&mut self) -> Option<u64> {
        let mut result = 0u64;
        let mut shift = 0;
        while self.cursor < self.data.len() {
            let byte = self.data[self.cursor];
            self.cursor += 1;
            result |= u64::from(byte & 0x7F) << shift;
            if (byte & 0x80) == 0 {
                return Some(result);
            }
            shift += 7;
            if shift >= 64 {
                return None;
            }
        }
        None
    }

    pub(super) fn next_tag(&mut self) -> Option<(u32, u8)> {
        let key = self.read_varint()?;
        let field_num = (key >> 3) as u32;
        let wire_type = (key & 0x07) as u8;
        Some((field_num, wire_type))
    }

    pub(super) fn read_bytes(&mut self) -> Option<&'a [u8]> {
        let len = self.read_varint()? as usize;
        if self.cursor + len > self.data.len() {
            return None;
        }
        let slice = &self.data[self.cursor..self.cursor + len];
        self.cursor += len;
        Some(slice)
    }

    pub(super) fn skip_field(&mut self, wire_type: u8) -> bool {
        match wire_type {
            0 => self.read_varint().is_some(),
            1 => {
                if self.cursor + 8 <= self.data.len() {
                    self.cursor += 8;
                    true
                } else {
                    false
                }
            }
            2 => {
                if let Some(len) = self.read_varint() {
                    let len = len as usize;
                    if self.cursor + len <= self.data.len() {
                        self.cursor += len;
                        true
                    } else {
                        false
                    }
                } else {
                    false
                }
            }
            5 if self.cursor + 4 <= self.data.len() => {
                self.cursor += 4;
                true
            }
            5 => false,
            _ => false,
        }
    }
}

pub(super) fn parse_token_stats_proto(bytes: &[u8]) -> RawTokenStats {
    let mut reader = ProtoReader::new(bytes);
    let mut stats = RawTokenStats::default();

    while let Some((field, wire_type)) = reader.next_tag() {
        match (field, wire_type) {
            (2, 0) => {
                if let Some(val) = reader.read_varint() {
                    stats.input_tokens = val;
                }
            }
            (3, 0) => {
                if let Some(val) = reader.read_varint() {
                    stats.output_tokens = val;
                }
            }
            (5, 0) => {
                if let Some(val) = reader.read_varint() {
                    stats.cache_read_tokens = val;
                }
            }
            (9, 0) => {
                if let Some(val) = reader.read_varint() {
                    stats.reasoning_tokens = val;
                }
            }
            (10, 0) => {
                if let Some(val) = reader.read_varint() {
                    stats.generation_tokens = val;
                }
            }
            _ => {
                if !reader.skip_field(wire_type) {
                    break;
                }
            }
        }
    }
    stats
}

pub(super) fn parse_timestamp_proto(bytes: &[u8]) -> Option<TimestampMs> {
    let mut reader = ProtoReader::new(bytes);
    let mut seconds = 0u64;
    let mut nanos = 0u64;

    while let Some((field, wire_type)) = reader.next_tag() {
        match (field, wire_type) {
            (1, 0) => {
                if let Some(val) = reader.read_varint() {
                    seconds = val;
                }
            }
            (2, 0) => {
                if let Some(val) = reader.read_varint() {
                    nanos = val;
                }
            }
            _ => {
                if !reader.skip_field(wire_type) {
                    break;
                }
            }
        }
    }

    if seconds > 0 {
        let millis = (seconds as i64) * 1000 + (nanos as i64) / 1_000_000;
        Some(TimestampMs::from_millis(millis))
    } else {
        None
    }
}

pub(super) fn parse_step_metadata(
    bytes: &[u8],
) -> (Option<TimestampMs>, Option<String>, RawTokenStats) {
    let mut reader = ProtoReader::new(bytes);
    let mut timestamp = None;
    let mut session_id = None;
    let mut stats = RawTokenStats::default();

    while let Some((field, wire_type)) = reader.next_tag() {
        match (field, wire_type) {
            (1, 2) => {
                if let Some(sub_bytes) = reader.read_bytes() {
                    timestamp = parse_timestamp_proto(sub_bytes);
                }
            }
            (9 | 28, 2) => {
                if let Some(sub_bytes) = reader.read_bytes() {
                    let sub_stats = parse_token_stats_proto(sub_bytes);
                    if !sub_stats.is_zero() {
                        stats = sub_stats;
                    } else {
                        let mut sub_reader = ProtoReader::new(sub_bytes);
                        while let Some((sub_f, sub_w)) = sub_reader.next_tag() {
                            if sub_f == 2 && sub_w == 2 {
                                if let Some(nested) = sub_reader.read_bytes() {
                                    let nested_stats = parse_token_stats_proto(nested);
                                    if !nested_stats.is_zero() {
                                        stats = nested_stats;
                                    }
                                }
                            } else if !sub_reader.skip_field(sub_w) {
                                break;
                            }
                        }
                    }
                }
            }
            (20, 2) => {
                if let Some(sub_bytes) = reader.read_bytes() {
                    let mut sub_reader = ProtoReader::new(sub_bytes);
                    while let Some((sub_f, sub_w)) = sub_reader.next_tag() {
                        if sub_f == 4 && sub_w == 2 {
                            if let Some(text) = sub_reader
                                .read_bytes()
                                .and_then(|val| std::str::from_utf8(val).ok())
                            {
                                session_id = Some(text.trim().to_string());
                            }
                        } else if !sub_reader.skip_field(sub_w) {
                            break;
                        }
                    }
                }
            }
            _ => {
                if !reader.skip_field(wire_type) {
                    break;
                }
            }
        }
    }

    (timestamp, session_id, stats)
}

pub(super) fn parse_gen_metadata(bytes: &[u8]) -> (Option<String>, RawTokenStats) {
    let mut reader = ProtoReader::new(bytes);
    let mut model = None;
    let mut stats = RawTokenStats::default();

    while let Some((field, wire_type)) = reader.next_tag() {
        if field == 1 && wire_type == 2 {
            if let Some(sub_bytes) = reader.read_bytes() {
                let mut sub_reader = ProtoReader::new(sub_bytes);
                while let Some((sub_f, sub_w)) = sub_reader.next_tag() {
                    match (sub_f, sub_w) {
                        (4, 2) => {
                            if let Some(stats_bytes) = sub_reader.read_bytes() {
                                stats = parse_token_stats_proto(stats_bytes);
                            }
                        }
                        (19, 2) => {
                            if let Some(text) = sub_reader
                                .read_bytes()
                                .and_then(|str_bytes| std::str::from_utf8(str_bytes).ok())
                            {
                                let trimmed = text.trim();
                                if !trimmed.is_empty() {
                                    model = Some(trimmed.to_string());
                                }
                            }
                        }
                        _ => {
                            if !sub_reader.skip_field(sub_w) {
                                break;
                            }
                        }
                    }
                }
            }
        } else if !reader.skip_field(wire_type) {
            break;
        }
    }

    (model, stats)
}

pub(super) fn to_loaded_entry(
    entry: AntigravityEntry,
    tz: Option<&JiffTimeZone>,
    pricing: &PricingMap,
) -> LoadedEntry {
    let cost = calculate_antigravity_cost(&entry, pricing);
    let missing_pricing_model = missing_antigravity_pricing(&entry, pricing);
    let data = UsageEntry {
        session_id: Some(entry.session_id.clone()),
        timestamp: entry.timestamp_text.clone(),
        version: None,
        message: UsageMessage {
            usage: entry.usage,
            model: Some(entry.model.clone()),
            id: Some(format!("antigravity:{}", entry.session_id)),
        },
        cost_usd: None,
        request_id: None,
        is_api_error_message: None,
        is_sidechain: None,
    };
    LoadedEntry {
        date: format_date_tz(entry.timestamp, tz),
        timestamp: entry.timestamp,
        project: Arc::from("antigravity"),
        session_id: Arc::from(entry.session_id.as_str()),
        project_path: Arc::from("Antigravity"),
        cost,
        credits: None,
        extra_total_tokens: entry.reasoning_tokens,
        message_count: Some(entry.message_count),
        model: Some(entry.model),
        usage_limit_reset_time: None,
        missing_pricing_model,
        data,
    }
}

pub(super) fn build_entry(
    timestamp: TimestampMs,
    session_id: String,
    model: String,
    stats: RawTokenStats,
    message_count: u64,
) -> AntigravityEntry {
    let usage = TokenUsageRaw {
        input_tokens: stats.input_tokens,
        output_tokens: stats.output_tokens,
        cache_creation_input_tokens: 0,
        cache_read_input_tokens: stats.cache_read_tokens,
        speed: None,
        cache_creation: None,
    };
    AntigravityEntry {
        timestamp,
        timestamp_text: format_rfc3339_millis(timestamp),
        session_id,
        model,
        usage,
        reasoning_tokens: stats.reasoning_tokens,
        message_count,
    }
}

fn calculate_antigravity_cost(entry: &AntigravityEntry, pricing: &PricingMap) -> f64 {
    let usage = TokenUsageRaw {
        output_tokens: entry.usage.output_tokens + entry.reasoning_tokens,
        cache_creation: None,
        ..entry.usage
    };
    for candidate in model_candidates(entry) {
        let cost = calculate_cost_for_usage(
            Some(&candidate),
            usage,
            None,
            CostMode::Calculate,
            Some(pricing),
        );
        if cost.is_finite() && cost > 0.0 {
            return cost;
        }
    }
    0.0
}

fn missing_antigravity_pricing(entry: &AntigravityEntry, pricing: &PricingMap) -> Option<String> {
    let usage = TokenUsageRaw {
        output_tokens: entry.usage.output_tokens + entry.reasoning_tokens,
        cache_creation: None,
        ..entry.usage
    };
    missing_pricing_model_for_candidates(
        &entry.model,
        model_candidates(entry),
        crate::total_usage_tokens(usage),
        Some(pricing),
    )
}

fn model_candidates(entry: &AntigravityEntry) -> Vec<String> {
    let mut candidates = Vec::new();
    let model = &entry.model;
    if !model.starts_with("google/") && !model.starts_with("gemini/") {
        candidates.push(format!("google/{model}"));
        candidates.push(format!("gemini/{model}"));
    }
    candidates.push(model.clone());
    let mut seen = HashSet::new();
    candidates
        .into_iter()
        .filter(|candidate| seen.insert(candidate.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_proto_varint_and_tags() {
        let mut buf = Vec::new();
        // field 2 (wire type 0): varint 1200
        // tag = (2 << 3) | 0 = 16 (0x10)
        buf.push(0x10);
        buf.extend_from_slice(&[0xb0, 0x09]); // 1200

        // field 3 (wire type 0): varint 300
        // tag = (3 << 3) | 0 = 24 (0x18)
        buf.push(0x18);
        buf.extend_from_slice(&[0xac, 0x02]); // 300

        let stats = parse_token_stats_proto(&buf);
        assert_eq!(stats.input_tokens, 1200);
        assert_eq!(stats.output_tokens, 300);
        assert_eq!(stats.cache_read_tokens, 0);
    }

    #[test]
    fn calculates_cost_for_gemini_models_from_pricing() {
        let pricing = PricingMap::load_embedded();
        let entry = build_entry(
            crate::parse_ts_timestamp("2026-05-19T00:00:00.000Z").unwrap(),
            "test-session".to_string(),
            "gemini-2.5-flash".to_string(),
            RawTokenStats {
                input_tokens: 10_000,
                output_tokens: 1_000,
                cache_read_tokens: 5_000,
                reasoning_tokens: 200,
                generation_tokens: 800,
            },
            1,
        );

        let cost = calculate_antigravity_cost(&entry, &pricing);
        assert!(cost > 0.0, "gemini-2.5-flash should have non-zero cost");
    }

    #[test]
    fn candidate_models_include_google_and_gemini_prefixes() {
        let entry = build_entry(
            TimestampMs::UNIX_EPOCH,
            "session-1".to_string(),
            "gemini-3.7-flash".to_string(),
            RawTokenStats::default(),
            1,
        );
        let candidates = model_candidates(&entry);
        assert_eq!(
            candidates,
            vec![
                "google/gemini-3.7-flash".to_string(),
                "gemini/gemini-3.7-flash".to_string(),
                "gemini-3.7-flash".to_string(),
            ]
        );
    }
}
