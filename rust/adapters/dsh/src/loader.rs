use std::collections::HashMap;

use crate::{
    LoadedEntry, PricingMap, Result,
    cli::SharedArgs,
    debug_log, parse_tz,
    parser::{read_session_file, to_loaded_entry},
    paths::{discover_session_files, session_file_version},
};
use ccusage_adapter_common::read_files_parallel;

pub fn load_entries(shared: &SharedArgs, pricing: &PricingMap) -> Result<Vec<LoadedEntry>> {
    crate::progress::track_usage_load(
        crate::progress::UsageLoadAgent("DeepSeek Harness"),
        shared.json,
        || load_entries_inner(shared, pricing),
    )
}

fn load_entries_inner(shared: &SharedArgs, pricing: &PricingMap) -> Result<Vec<LoadedEntry>> {
    let tz = parse_tz(shared.timezone.as_deref());
    let files = discover_session_files();
    let loaded = read_files_parallel(&files, shared.single_thread, |file| {
        let filename_version = session_file_version(file).unwrap_or_default();
        read_session_file(file, filename_version).unwrap_or_else(|error| {
            debug_log(
                shared,
                format!(
                    "Failed to read DeepSeek Harness session {}: {error}",
                    file.display()
                ),
            );
            Vec::new()
        })
    });
    let mut entries = Vec::new();
    let mut indexes = HashMap::<(String, String), (u64, usize)>::new();
    for file_entries in loaded {
        for entry in file_entries {
            let key = (entry.session_id.clone(), entry.message_id.clone());
            if let Some((version, index)) = indexes.get(&key).copied() {
                if entry.format_version > version {
                    let entry_version = entry.format_version;
                    entries[index] = to_loaded_entry(entry, tz.as_ref(), shared.mode, pricing);
                    indexes.insert(key, (entry_version, index));
                }
                continue;
            }
            let index = entries.len();
            indexes.insert(key, (entry.format_version, index));
            entries.push(to_loaded_entry(entry, tz.as_ref(), shared.mode, pricing));
        }
    }
    entries.sort_by_key(|entry| entry.timestamp);
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use std::{
        fs::File,
        io::{Read, Seek, SeekFrom, Write},
    };

    use super::*;
    use ccusage_test_support::{EnvVarGuard, fs_fixture};

    #[test]
    fn loads_v3_plain_assistant_message_usage() {
        let fixture = fs_fixture!({
            "sessions/workspace/session-a/session.v3.jsonl": [
                r#"{"type":"session","id":"session-a","cwd":"/workspace/api","version":3}"#,
                r#"{"type":"assistant/message","time":1792974896000,"data":{"message":{"id":"message-a","source":{"provider":"deepseek","model":"deepseek-v3"}},"usage":{"inputTokens":100,"outputTokens":50,"cacheReadTokens":25,"cacheWriteTokens":5,"totalTokens":180}}}"#,
            ]
            .join("\n"),
        });
        let _cleanup = EnvVarGuard::set("DSH_HOME", fixture.root());
        let shared = SharedArgs {
            timezone: Some("UTC".to_string()),
            ..SharedArgs::default()
        };

        let entries = load_entries(&shared, &PricingMap::load_embedded()).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].date, "2026-10-26");
        assert_eq!(entries[0].session_id.as_ref(), "session-a");
        assert_eq!(entries[0].project_path.as_ref(), "/workspace/api");
        assert_eq!(entries[0].data.message.usage.input_tokens, 100);
        assert_eq!(entries[0].data.message.usage.output_tokens, 50);
        assert_eq!(entries[0].data.message.usage.cache_read_input_tokens, 25);
        assert_eq!(entries[0].data.message.usage.cache_creation_input_tokens, 5);
        assert_eq!(entries[0].model.as_deref(), Some("deepseek-v3"));
    }

    #[test]
    fn accepts_session_headers_without_cwd() {
        let fixture = fs_fixture!({
            "sessions/workspace/session-a/session.v4.jsonl": concat!(
                "{\"type\":\"session\",\"id\":\"session-a\",\"version\":4}\n",
                "{\"type\":\"assistant/message\",\"time\":1792974896000,\"data\":{\"message\":{\"id\":\"message-a\",\"source\":{\"provider\":\"deepseek\",\"model\":\"deepseek-v4.1-flash\"}},\"usage\":{\"inputTokens\":100,\"outputTokens\":50}}}\n"
            ),
        });
        let _cleanup = EnvVarGuard::set("DSH_HOME", fixture.root());

        let entries = load_entries(&SharedArgs::default(), &PricingMap::load_embedded()).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].project_path.as_ref(), "DeepSeek Harness");
    }

    #[test]
    fn loads_unversioned_plain_and_compressed_sessions() {
        let fixture = fs_fixture!({
            "sessions/workspace/session-plain/session.jsonl": [
                r#"{"type":"session","id":"session-plain","cwd":"/workspace/plain"}"#,
                r#"{"type":"assistant/message","time":1792974896000,"data":{"message":{"id":"plain-message","source":{"provider":"deepseek","model":"deepseek-v4-flash"}},"usage":{"inputTokens":100,"outputTokens":50}}}"#,
            ]
            .join("\n"),
            "sessions/workspace/session-compressed/session.jsonl.zstd": "",
        });
        write_zstd_frame(
            &fixture.path("sessions/workspace/session-compressed/session.jsonl.zstd"),
            concat!(
                r#"{"type":"session","id":"session-compressed","cwd":"/workspace/compressed"}"#,
                "\n",
                r#"{"type":"assistant/message","time":1792974897000,"data":{"message":{"id":"compressed-message","source":{"provider":"deepseek","model":"deepseek-v4-flash"}},"usage":{"inputTokens":200,"outputTokens":60}}}"#,
            ),
        );
        let _cleanup = EnvVarGuard::set("DSH_HOME", fixture.root());

        let entries = load_entries(&SharedArgs::default(), &PricingMap::load_embedded()).unwrap();

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].session_id.as_ref(), "session-plain");
        assert_eq!(entries[0].project_path.as_ref(), "/workspace/plain");
        assert_eq!(entries[0].data.version.as_deref(), Some("0"));
        assert_eq!(entries[1].session_id.as_ref(), "session-compressed");
        assert_eq!(entries[1].project_path.as_ref(), "/workspace/compressed");
        assert_eq!(entries[1].data.version.as_deref(), Some("0"));
    }

    #[test]
    fn applies_total_token_fallback_and_skips_malformed_or_zero_usage() {
        let fixture = fs_fixture!({
            "sessions/workspace/session-a/session.v4.jsonl": concat!(
                "{\"type\":\"session\",\"id\":\"session-a\",\"cwd\":\"/workspace/api\",\"version\":4}\n",
                "not json\n",
                "{\"type\":\"assistant/chunk\",\"time\":1792974895000,\"data\":{\"message\":{\"id\":\"chunk-only\",\"source\":{\"provider\":\"deepseek\",\"model\":\"deepseek-v4-flash\"}},\"usage\":{\"inputTokens\":100,\"outputTokens\":50}}}\n",
                "{\"type\":\"assistant/message\",\"time\":1792974895500,\"data\":{\"message\":{\"id\":\"zero\",\"source\":{\"provider\":\"deepseek\",\"model\":\"deepseek-v4-flash\"}},\"usage\":{\"inputTokens\":0,\"outputTokens\":0,\"cacheReadTokens\":0,\"cacheWriteTokens\":0,\"totalTokens\":0}}}\n",
                "{\"type\":\"assistant/message\",\"time\":1792974896000,\"data\":{\"message\":{\"id\":\"fallback\",\"source\":{\"provider\":\"deepseek\",\"model\":\"deepseek-v4-flash\"}},\"usage\":{\"inputTokens\":100,\"outputTokens\":0,\"cacheReadTokens\":25,\"cacheWriteTokens\":5,\"totalTokens\":150}}}\n"
            ),
        });
        let _cleanup = EnvVarGuard::set("DSH_HOME", fixture.root());

        let entries = load_entries(&SharedArgs::default(), &PricingMap::load_embedded()).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].data.message.id.as_deref(), Some("fallback"));
        assert_eq!(entries[0].data.message.usage.input_tokens, 100);
        assert_eq!(entries[0].data.message.usage.output_tokens, 20);
        assert_eq!(entries[0].data.message.usage.cache_read_input_tokens, 25);
        assert_eq!(entries[0].data.message.usage.cache_creation_input_tokens, 5);
        assert_eq!(entries[0].extra_total_tokens, 0);
    }

    #[test]
    fn deduplicates_repeated_final_messages_with_the_same_id() {
        let repeated = r#"{"type":"assistant/message","time":1792974896000,"data":{"message":{"id":"message-a","source":{"provider":"deepseek","model":"deepseek-v4-flash"}},"usage":{"inputTokens":100,"outputTokens":50}}}"#;
        let fixture = fs_fixture!({
            "sessions/workspace/session-a/session.v4.jsonl": [
                r#"{"type":"session","id":"session-a","cwd":"/workspace/api","version":4}"#,
                repeated,
                repeated,
            ]
            .join("\n"),
        });
        let _cleanup = EnvVarGuard::set("DSH_HOME", fixture.root());

        let entries = load_entries(&SharedArgs::default(), &PricingMap::load_embedded()).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].data.message.id.as_deref(), Some("message-a"));
        assert_eq!(entries[0].data.message.usage.input_tokens, 100);
        assert_eq!(entries[0].data.message.usage.output_tokens, 50);
    }

    #[test]
    fn loads_v4_plain_and_concatenated_zstd_frames_ignoring_chunks() {
        let fixture = fs_fixture!({
            "sessions/workspace/session-a/session.v4.jsonl.zstd": "",
            "sessions/workspace/session-b/session.v4.jsonl.zstd": "",
        });
        let ordinary = fixture.path("sessions/workspace/session-a/session.v4.jsonl.zstd");
        write_zstd_frame(
            &ordinary,
            concat!(
                "{\"type\":\"session\",\"id\":\"session-a\",\"cwd\":\"/workspace/alpha\",\"version\":4}\n",
                "{\"type\":\"assistant/chunk\",\"time\":1792974895000,\"data\":{\"message\":{\"id\":\"shared-id\",\"source\":{\"provider\":\"deepseek\",\"model\":\"deepseek-v4.1-flash\"}},\"usage\":{\"inputTokens\":900,\"outputTokens\":900}}}\n",
                "{\"type\":\"assistant/message\",\"time\":1792974896000,\"data\":{\"message\":{\"id\":\"shared-id\",\"source\":{\"provider\":\"deepseek\",\"model\":\"deepseek-v4.1-flash\"}},\"usage\":{\"inputTokens\":100,\"outputTokens\":50,\"cacheReadTokens\":25,\"cacheWriteTokens\":5,\"totalTokens\":180}}}\n"
            ),
        );
        let concatenated = fixture.path("sessions/workspace/session-b/session.v4.jsonl.zstd");
        write_concatenated_zstd_frames(
            &concatenated,
            &[
                "{\"type\":\"session\",\"id\":\"session-b\",\"cwd\":\"/workspace/beta\",\"version\":4}\n",
                "{\"type\":\"assistant/chunk\",\"time\":1792974895000,\"data\":{\"message\":{\"id\":\"shared-id\",\"source\":{\"provider\":\"deepseek\",\"model\":\"deepseek-v4.1-flash\"}},\"usage\":{\"inputTokens\":900,\"outputTokens\":900}}}\n",
                "{\"type\":\"assistant/message\",\"time\":1792974896000,\"data\":{\"message\":{\"id\":\"shared-id\",\"source\":{\"provider\":\"deepseek\",\"model\":\"deepseek-v4.1-flash\"}},\"usage\":{\"inputTokens\":200,\"outputTokens\":60,\"cacheReadTokens\":10,\"cacheWriteTokens\":0,\"totalTokens\":270}}}\n",
            ],
        );
        let _cleanup = EnvVarGuard::set("DSH_HOME", fixture.root());
        let shared = SharedArgs {
            timezone: Some("UTC".to_string()),
            ..SharedArgs::default()
        };

        let entries = load_entries(&shared, &PricingMap::load_embedded()).unwrap();

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].session_id.as_ref(), "session-a");
        assert_eq!(entries[1].session_id.as_ref(), "session-b");
        assert_eq!(entries[0].data.message.id.as_deref(), Some("shared-id"));
        assert_eq!(entries[1].data.message.id.as_deref(), Some("shared-id"));
        assert_eq!(entries[0].data.message.usage.input_tokens, 100);
        assert_eq!(entries[1].data.message.usage.input_tokens, 200);
        assert_eq!(entries[0].project_path.as_ref(), "/workspace/alpha");
        assert_eq!(entries[1].project_path.as_ref(), "/workspace/beta");
    }

    #[test]
    fn preserves_complete_records_before_a_truncated_concatenated_frame() {
        let fixture = fs_fixture!({
            "sessions/workspace/session-a/session.v4.jsonl.zstd": "",
        });
        let path = fixture.path("sessions/workspace/session-a/session.v4.jsonl.zstd");
        let second_frame = format!(
            "{}\n{}\n{}",
            r#"{"type":"assistant/message","time":1792974897000,"data":{"message":{"id":"complete-two","source":{"provider":"deepseek","model":"deepseek-v4-flash"}},"usage":{"inputTokens":200,"outputTokens":60}}}"#,
            " ".repeat(140_000),
            r#"{"type":"assistant/message","time":1792974898000,"data":{"message":{"id":"partial-three","source":{"provider":"deepseek","model":"deepseek-v4-flash"}},"usage":{"inputTokens":300"#,
        );
        write_concatenated_zstd_frames(
            &path,
            &[
                concat!(
                    r#"{"type":"session","id":"session-a","cwd":"/workspace/api","version":4}"#,
                    "\n",
                    r#"{"type":"assistant/message","time":1792974896000,"data":{"message":{"id":"complete-one","source":{"provider":"deepseek","model":"deepseek-v4-flash"}},"usage":{"inputTokens":100,"outputTokens":50}}}"#,
                    "\n",
                ),
                second_frame.as_str(),
            ],
        );
        let compressed_length = std::fs::metadata(&path).unwrap().len();
        std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(compressed_length - 1)
            .unwrap();
        let mut decoder = zstd::stream::read::Decoder::new(File::open(&path).unwrap()).unwrap();
        let mut decoded = Vec::new();
        assert!(decoder.read_to_end(&mut decoded).is_err());

        let _cleanup = EnvVarGuard::set("DSH_HOME", fixture.root());
        let entries = load_entries(&SharedArgs::default(), &PricingMap::load_embedded()).unwrap();

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].data.message.id.as_deref(), Some("complete-one"));
        assert_eq!(entries[1].data.message.id.as_deref(), Some("complete-two"));
    }

    #[test]
    fn returns_read_error_when_truncated_file_has_no_valid_records() {
        let fixture = fs_fixture!({
            "sessions/workspace/session-a/session.v4.jsonl.zstd": "",
        });
        let path = fixture.path("sessions/workspace/session-a/session.v4.jsonl.zstd");
        write_zstd_frame(
            &path,
            concat!(
                r#"{"type":"session","id":"session-a","version":4}"#,
                "\n",
                r#"{"type":"assistant/message","time":1792974896000"#,
            ),
        );
        let compressed_length = std::fs::metadata(&path).unwrap().len();
        std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(compressed_length - 1)
            .unwrap();

        assert!(super::read_session_file(&path, 4).is_err());
    }

    #[test]
    fn prefers_newest_generation_and_parallel_loading_matches_single_threaded() {
        let fixture = fs_fixture!({
            "sessions/workspace/session-a/session.v3.jsonl": concat!(
                "{\"type\":\"session\",\"id\":\"session-a\",\"cwd\":\"/workspace/api\",\"version\":3}\n",
                "{\"type\":\"assistant/message\",\"time\":1792974896000,\"data\":{\"message\":{\"id\":\"message-a\",\"source\":{\"provider\":\"deepseek\",\"model\":\"deepseek-v3\"}},\"usage\":{\"inputTokens\":100,\"outputTokens\":50}}}\n"
            ),
            "sessions/workspace/session-a/session.v4.jsonl": concat!(
                "{\"type\":\"session\",\"id\":\"session-a\",\"cwd\":\"/workspace/api\",\"version\":4}\n",
                "{\"type\":\"assistant/message\",\"time\":1792974897000,\"data\":{\"message\":{\"id\":\"message-a\",\"source\":{\"provider\":\"deepseek\",\"model\":\"deepseek-v4.1-flash\"}},\"usage\":{\"inputTokens\":200,\"outputTokens\":60}}}\n"
            ),
            "sessions/workspace/session-b/session.v4.jsonl": concat!(
                "{\"type\":\"session\",\"id\":\"session-b\",\"cwd\":\"/workspace/api\",\"version\":4}\n",
                "{\"type\":\"assistant/message\",\"time\":1792974898000,\"data\":{\"message\":{\"id\":\"message-a\",\"source\":{\"provider\":\"deepseek\",\"model\":\"deepseek-v4.1-flash\"}},\"usage\":{\"inputTokens\":25,\"outputTokens\":10}}}\n"
            ),
        });
        let _cleanup = EnvVarGuard::set("DSH_HOME", fixture.root());
        let pricing = PricingMap::load_embedded();
        let parallel = load_entries(&SharedArgs::default(), &pricing).unwrap();
        let single_thread = load_entries(
            &SharedArgs {
                single_thread: true,
                ..SharedArgs::default()
            },
            &pricing,
        )
        .unwrap();

        assert_eq!(parallel.len(), 2);
        assert_eq!(parallel.len(), single_thread.len());
        assert_eq!(parallel[0].session_id.as_ref(), "session-a");
        assert_eq!(parallel[0].data.message.usage.input_tokens, 200);
        assert_eq!(parallel[1].session_id.as_ref(), "session-b");
        assert_eq!(parallel[1].data.message.usage.input_tokens, 25);
        for (left, right) in parallel.iter().zip(single_thread.iter()) {
            assert_eq!(left.session_id, right.session_id);
            assert_eq!(left.timestamp, right.timestamp);
            assert_eq!(left.model, right.model);
            assert_eq!(
                left.data.message.usage.input_tokens,
                right.data.message.usage.input_tokens
            );
        }
    }

    #[test]
    fn prices_provider_models_without_using_direct_deepseek_schedule() {
        let fixture = fs_fixture!({
            "sessions/workspace/session-together/session.v4.jsonl": concat!(
                "{\"type\":\"session\",\"id\":\"session-together\",\"cwd\":\"/workspace/api\",\"version\":4}\n",
                "{\"type\":\"assistant/message\",\"time\":1792974896000,\"data\":{\"message\":{\"id\":\"together-message\",\"source\":{\"provider\":\"together\",\"model\":\"deepseek-ai/DeepSeek-V4-Flash-0731\"}},\"usage\":{\"inputTokens\":1000000,\"outputTokens\":1000000}}}\n"
            ),
            "sessions/workspace/session-ollama/session.v4.jsonl": concat!(
                "{\"type\":\"session\",\"id\":\"session-ollama\",\"cwd\":\"/workspace/api\",\"version\":4}\n",
                "{\"type\":\"assistant/message\",\"time\":1792974896000,\"data\":{\"message\":{\"id\":\"ollama-message\",\"source\":{\"provider\":\"ollama\",\"model\":\"deepseek-v4.1-flash:1017-cloud\"}},\"usage\":{\"inputTokens\":1000000,\"outputTokens\":1000000}}}\n"
            ),
            "sessions/workspace/session-direct/session.v4.jsonl": concat!(
                "{\"type\":\"session\",\"id\":\"session-direct\",\"cwd\":\"/workspace/api\",\"version\":4}\n",
                "{\"type\":\"assistant/message\",\"time\":1792974896000,\"data\":{\"message\":{\"id\":\"direct-message\",\"source\":{\"provider\":\"deepseek\",\"model\":\"deepseek-v4-flash\"}},\"usage\":{\"inputTokens\":1000000,\"outputTokens\":1000000}}}\n"
            ),
            "sessions/workspace/session-direct-pro/session.v4.jsonl": concat!(
                "{\"type\":\"session\",\"id\":\"session-direct-pro\",\"cwd\":\"/workspace/api\",\"version\":4}\n",
                "{\"type\":\"assistant/message\",\"time\":1792974896000,\"data\":{\"message\":{\"id\":\"direct-pro-message\",\"source\":{\"provider\":\"deepseek\",\"model\":\"deepseek-v4-pro\"}},\"usage\":{\"inputTokens\":1000000,\"outputTokens\":1000000}}}\n"
            ),
        });
        let _cleanup = EnvVarGuard::set("DSH_HOME", fixture.root());
        let entries = load_entries(&SharedArgs::default(), &PricingMap::load_embedded()).unwrap();
        let together = entries
            .iter()
            .find(|entry| entry.session_id.as_ref() == "session-together")
            .unwrap();
        let ollama = entries
            .iter()
            .find(|entry| entry.session_id.as_ref() == "session-ollama")
            .unwrap();
        let direct = entries
            .iter()
            .find(|entry| entry.session_id.as_ref() == "session-direct")
            .unwrap();
        let direct_pro = entries
            .iter()
            .find(|entry| entry.session_id.as_ref() == "session-direct-pro")
            .unwrap();

        assert_eq!(
            together.model.as_deref(),
            Some("together_ai/deepseek-ai/DeepSeek-V4-Flash-0731")
        );
        assert!(together.cost > 0.0);
        assert_eq!(
            ollama.model.as_deref(),
            Some("ollama/deepseek-v4.1-flash:1017-cloud")
        );
        assert!((ollama.cost - 1.5).abs() < 1e-9);
        assert_eq!(ollama.missing_pricing_model, None);
        assert_eq!(direct.model.as_deref(), Some("deepseek-v4-flash"));
        assert!((direct.cost - 0.88).abs() < 1e-9);
        assert_eq!(direct_pro.model.as_deref(), Some("deepseek-v4-pro"));
        assert!((direct_pro.cost - 2.64).abs() < 1e-9);
        assert!((together.cost - 0.42).abs() < 1e-9);
    }

    #[test]
    fn retains_usage_and_marks_unknown_models_without_a_price() {
        let fixture = fs_fixture!({
            "sessions/workspace/session-a/session.v4.jsonl": concat!(
                r#"{"type":"session","id":"session-a","cwd":"/workspace/api","version":4}"#,
                "\n",
                r#"{"type":"assistant/message","time":1792974896000,"data":{"message":{"id":"unknown-message","source":{"provider":"deepseek","model":"deepseek-v5-unlisted"}},"usage":{"inputTokens":77,"outputTokens":13}}}"#,
                "\n",
            ),
        });
        let _cleanup = EnvVarGuard::set("DSH_HOME", fixture.root());

        let entries = load_entries(&SharedArgs::default(), &PricingMap::load_embedded()).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].cost, 0.0);
        assert_eq!(
            entries[0].missing_pricing_model.as_deref(),
            Some("deepseek-v5-unlisted")
        );
        assert_eq!(entries[0].data.message.usage.input_tokens, 77);
        assert_eq!(entries[0].data.message.usage.output_tokens, 13);
    }

    fn write_zstd_frame(path: &std::path::Path, content: &str) {
        let mut encoder =
            zstd::stream::write::Encoder::new(File::create(path).unwrap(), 0).unwrap();
        encoder.write_all(content.as_bytes()).unwrap();
        encoder.finish().unwrap();
    }

    fn write_concatenated_zstd_frames(path: &std::path::Path, frames: &[&str]) {
        let mut file = File::create(path).unwrap();
        for frame in frames {
            file.seek(SeekFrom::End(0)).unwrap();
            let mut encoder = zstd::stream::write::Encoder::new(file, 0).unwrap();
            encoder.write_all(frame.as_bytes()).unwrap();
            file = encoder.finish().unwrap();
        }
    }
}
