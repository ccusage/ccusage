use std::ffi::OsString;
use std::path::Path;

use ccusage_test_support::{Instant, instant, set_modified};

use crate::golden::Case;

const GPT5: &str = "gpt-5";
const GPT55: &str = "gpt-5.5";
const SOL: &str = "gpt-6.1-sol";
const CODEX: &str = "gpt-5-codex";
const UNPRICED: &str = "codex-mystery-1";

const MAIN: &str = "11111111-1111-4111-8111-111111111111";
const COMPACTING: &str = "22222222-2222-4222-8222-222222222222";
const USER_FORK: &str = "33333333-3333-4333-8333-333333333333";
const SUBAGENT: &str = "44444444-4444-4444-8444-444444444444";
const ORPHAN_FORK: &str = "55555555-5555-4555-8555-555555555555";
const LEGACY: &str = "77777777-7777-4777-8777-777777777777";
const COMPRESSED: &str = "88888888-8888-4888-8888-888888888888";
const BOTH_ENCODINGS: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
const LEGACY_FORK: &str = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
const COMPACTING_FORK: &str = "cccccccc-cccc-4ccc-8ccc-cccccccccccc";
const ARCHIVED_COPY: &str = "dddddddd-dddd-4ddd-8ddd-dddddddddddd";
const MISSING_PARENT: &str = "99999999-9999-4999-8999-999999999999";

const REPLACEMENT: &str = "eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee";
const SUBAGENT_SIBLING: &str = "ffffffff-ffff-4fff-8fff-ffffffffffff";

fn rollout(date: &str, time: &str, id: &str) -> String {
    format!(
        "{}/rollout-{date}T{time}-{id}.jsonl",
        date.replace('-', "/")
    )
}

fn usage(input: u64, cached: u64, output: u64, reasoning: u64) -> String {
    format!(
        r#"{{"input_tokens":{input},"cached_input_tokens":{cached},"output_tokens":{output},"reasoning_output_tokens":{reasoning},"total_tokens":{}}}"#,
        input + output
    )
}

fn meta(timestamp: &str, id: &str, extra: &str) -> String {
    format!(
        r#"{{"timestamp":"{timestamp}","type":"session_meta","payload":{{"id":"{id}","cli_version":"0.144.0"{extra}}}}}"#
    )
}

fn turn(timestamp: &str, model: &str) -> String {
    format!(r#"{{"timestamp":"{timestamp}","type":"turn_context","payload":{{{model}}}}}"#)
}

fn count(timestamp: &str, last: Option<&str>, total: Option<&str>) -> String {
    let info = [
        last.map(|last| format!(r#""last_token_usage":{last}"#)),
        total.map(|total| format!(r#""total_token_usage":{total}"#)),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(",");
    format!(
        r#"{{"timestamp":"{timestamp}","type":"event_msg","payload":{{"type":"token_count","info":{{{info}}}}}}}"#
    )
}

fn settings(timestamp: &str, settings: &str) -> String {
    format!(
        r#"{{"timestamp":"{timestamp}","type":"event_msg","payload":{{"type":"thread_settings_applied","thread_settings":{{{settings}}}}}}}"#
    )
}

fn record(timestamp: &str, response_id: &str, usage: &str, extra: &str) -> String {
    format!(
        r#"{{"timestamp":"{timestamp}","type":"token_usage_record","payload":{{"response_id":"{response_id}","usage":{usage}{extra}}}}}"#
    )
}

fn compacted(timestamp: &str, response_id: &str) -> String {
    format!(
        r#"{{"timestamp":"{timestamp}","type":"compacted","payload":{{"compaction_response_id":"{response_id}"}}}}"#
    )
}

fn remote_compaction_with_repeated_marker(stamp: &dyn Fn(&str) -> String) -> [String; 3] {
    [
        record(
            &stamp("2025-12-31T16:05:00.000Z"),
            "resp-remote-1",
            &usage(5000, 4000, 500, 0),
            "",
        ),
        compacted(&stamp("2025-12-31T16:05:01.000Z"), "resp-remote-1"),
        compacted(&stamp("2025-12-31T16:05:02.000Z"), "resp-remote-1"),
    ]
}

fn local_compaction_already_in_token_count(stamp: &dyn Fn(&str) -> String) -> [String; 3] {
    [
        record(
            &stamp("2025-12-31T16:10:00.000Z"),
            "resp-local-1",
            &usage(800, 0, 80, 0),
            &format!(r#","thread_token_usage":{}"#, usage(1800, 400, 180, 0)),
        ),
        count(
            &stamp("2025-12-31T16:10:01.000Z"),
            None,
            Some(&usage(1800, 400, 180, 0)),
        ),
        compacted(&stamp("2025-12-31T16:10:02.000Z"), "resp-local-1"),
    ]
}

fn unmatched_or_empty_compaction_records(stamp: &dyn Fn(&str) -> String) -> [String; 4] {
    [
        record(
            &stamp("2025-12-31T16:15:00.000Z"),
            "resp-not-compaction",
            &usage(9999, 0, 9999, 0),
            "",
        ),
        record(
            &stamp("2025-12-31T16:16:00.000Z"),
            "  ",
            &usage(9999, 0, 9999, 0),
            "",
        ),
        record(
            &stamp("2025-12-31T16:17:00.000Z"),
            "resp-zero",
            &usage(0, 0, 0, 0),
            "",
        ),
        compacted(&stamp("2025-12-31T16:17:01.000Z"), "resp-zero"),
    ]
}

fn remote_compaction_marker_before_record(stamp: &dyn Fn(&str) -> String) -> [String; 2] {
    [
        compacted(&stamp("2026-01-01T01:00:00.000Z"), "resp-remote-2"),
        record(
            &stamp("2026-01-01T01:00:01.000Z"),
            "resp-remote-2",
            &usage(2000, 1500, 200, 50),
            &format!(r#","model":"{GPT5}""#),
        ),
    ]
}

fn compacting_history(stamp: &dyn Fn(&str) -> String) -> Vec<String> {
    [
        turn(
            &stamp("2025-12-31T16:00:05.000Z"),
            &format!(r#""model_name":"{CODEX}""#),
        ),
        count(
            &stamp("2025-12-31T16:00:10.000Z"),
            None,
            Some(&usage(1000, 400, 100, 0)),
        ),
    ]
    .into_iter()
    .chain(remote_compaction_with_repeated_marker(stamp))
    .chain(local_compaction_already_in_token_count(stamp))
    .chain(unmatched_or_empty_compaction_records(stamp))
    .chain(remote_compaction_marker_before_record(stamp))
    .chain([count(
        &stamp("2026-01-01T01:05:00.000Z"),
        Some(&usage(300, 100, 30, 0)),
        Some(&usage(2100, 500, 210, 0)),
    )])
    .collect()
}

fn seed_exec_json_with_timestamp_fallback(write: &impl Fn(&str, &[String], Instant)) {
    write(
        "work/sessions/exec/headless-run.jsonl",
        &[
            format!(
                r#"{{"timestamp":"2026-01-01T03:00:00.000Z","model":"{GPT5}","usage":{{"prompt_tokens":900,"cached_tokens":300,"completion_tokens":90,"reasoning_tokens":9}}}}"#
            ),
            r#"{"timestamp":"2026-01-01T03:10:00.000Z","usage":{"input_tokens":500,"cached_input_tokens":200,"cache_write_input_tokens":400,"output_tokens":50}}"#
                .to_string(),
            r#"{"timestamp":"2026-01-01T03:30:00.000Z","usage":{"input_tokens":0,"output_tokens":0}}"#
                .to_string(),
            r#"{"timestamp":"2026-01-01T03:31:00.000Z","usage":"n/a","note":"9999"}"#.to_string(),
            format!(
                r#"{{"created_at":{created_at},"response":{{"model":"{GPT5}","usage":{{"input_tokens":150,"output_tokens":15,"total_tokens":0}}}}}}"#,
                created_at = instant("2026-01-02T03:00:00Z").secs()
            ),
            r#"{"result":{"usage":{"input":120,"output":12}}}"#.to_string(),
            r#"{"timestamp":"2026-01-02T05:00:00Z","model":5,"usage":{"input_tokens":80,"output_tokens":8}}"#
                .to_string(),
            r#"{"timestamp":"2025-12-31T23:59:59.000Z","model":"codex-auto-review","usage":{"input_tokens":640,"cached_input_tokens":40,"output_tokens":64}}"#
                .to_string(),
        ],
        instant("2026-01-02T04:00:00Z"),
    );
}

fn seed_fork_with_copied_compactions(write: &impl Fn(&str, &[String], Instant)) {
    let mut compacting_fork = vec![meta(
        "2026-01-01T02:00:00.000Z",
        COMPACTING_FORK,
        &format!(r#","forked_from_id":"{COMPACTING}""#),
    )];
    compacting_fork.extend(compacting_history(&|_| {
        "2026-01-01T02:00:00.000Z".to_string()
    }));
    compacting_fork.extend([
        record(
            "2026-01-01T02:10:00.000Z",
            "resp-fork-own",
            &usage(1500, 1200, 150, 0),
            "",
        ),
        compacted("2026-01-01T02:10:01.000Z", "resp-fork-own"),
        count(
            "2026-01-01T02:11:00.000Z",
            Some(&usage(220, 20, 22, 0)),
            Some(&usage(2320, 520, 232, 0)),
        ),
    ]);
    write(
        &format!(
            "work/sessions/{}",
            rollout("2026-01-01", "02-00-00", COMPACTING_FORK)
        ),
        &compacting_fork,
        instant("2026-01-01T02:12:00Z"),
    );
}

fn seed_archived_copy_with_service_tier(write: &impl Fn(&str, &[String], Instant)) {
    write(
        &format!("work/archived_sessions/rollout-2025-12-31T16-00-00-{ARCHIVED_COPY}.jsonl"),
        &[
            meta("2025-12-31T16:00:00.000Z", ARCHIVED_COPY, ""),
            turn(
                "2025-12-31T16:00:05.000Z",
                &format!(r#""model_name":"{CODEX}""#),
            ),
            count(
                "2025-12-31T16:00:10.000Z",
                None,
                Some(&usage(1000, 400, 100, 0)),
            ),
            settings("2025-12-31T16:04:59.000Z", r#""service_tier":"priority""#),
            record(
                "2025-12-31T16:05:00.000Z",
                "resp-remote-1",
                &usage(5000, 4000, 500, 0),
                "",
            ),
            compacted("2025-12-31T16:05:01.000Z", "resp-remote-1"),
        ],
        instant("2025-12-31T16:06:00Z"),
    );
}

pub(crate) fn seed(root: &Path) -> Vec<(&'static str, Vec<OsString>)> {
    let write_bytes = |relative: &str, contents: &[u8], modified: Instant| {
        let path = root.join("codex").join(relative);
        std::fs::create_dir_all(path.parent().expect("fixture file has a parent"))
            .expect("failed to create fixture directory");
        std::fs::write(&path, contents).expect("failed to write fixture file");
        set_modified(&path, modified);
    };
    let write = |relative: &str, lines: &[String], modified: Instant| {
        write_bytes(relative, lines.join("\n").as_bytes(), modified);
    };
    let compress = |lines: &[String]| {
        zstd::encode_all(lines.join("\n").as_bytes(), 0).expect("failed to compress fixture")
    };

    write(
        "work/config.toml",
        &[r#"service_tier = "fast"
profile = "slow"

[profiles.slow]
service_tier = "flex"

[profiles.unused]
service_tier = "fast"
"#
        .to_string()],
        instant("2025-12-30T00:00:00Z"),
    );

    let main = rollout("2025-12-30", "10-00-00", MAIN);
    let first_turn = count(
        "2025-12-30T10:01:00.000Z",
        Some(&usage(1000, 200, 100, 20)),
        Some(&usage(1000, 200, 100, 20)),
    );
    write(
        &format!("work/sessions/{main}"),
        &[
            meta("2025-12-30T10:00:00.000Z", MAIN, ""),
            turn("2025-12-30T10:00:01.000Z", &format!(r#""model":"{GPT55}""#)),
            r#"{"timestamp":"2025-12-30T10:00:05.000Z","type":"event_msg","payload":{"type":"token_count","info":null}}"#
                .to_string(),
            first_turn.clone(),
            count(
                "2025-12-30T10:01:01.000Z",
                Some(&usage(9999, 0, 9999, 0)),
                Some(&usage(1000, 200, 100, 20)),
            ),
            settings("2025-12-30T10:01:30.000Z", r#""service_tier":"priority""#),
            count(
                "2025-12-30T10:02:00.000Z",
                None,
                Some(&usage(3000, 1200, 400, 50)),
            ),
            settings("2025-12-30T10:03:00.000Z", r#""approval_policy":"never""#),
            count(
                "2025-12-31T09:00:00.000Z",
                Some(&usage(500, 100, 50, 0)),
                Some(&usage(3500, 1300, 450, 50)),
            ),
            settings("2025-12-31T15:00:00.000Z", r#""service_tier":"warp""#),
            count(
                "2025-12-31T15:30:00.000Z",
                Some(&usage(700, 300, 70, 10)),
                Some(&usage(4200, 1600, 520, 60)),
            ),
            turn(
                "2026-01-01T11:59:00.000Z",
                &format!(r#""model":"{UNPRICED}""#),
            ),
            count(
                "2026-01-01T12:00:00.000Z",
                Some(&usage(300, 0, 30, 0)),
                Some(&usage(4500, 1600, 550, 60)),
            ),
            turn(
                "2026-01-01T12:09:00.000Z",
                &format!(r#""metadata":{{"model":"{GPT55}"}}"#),
            ),
            count(
                "2026-01-01T12:10:00.000Z",
                None,
                Some(&usage(100, 50, 10, 0)),
            ),
            count(
                "2026-01-01T12:11:00.000Z",
                None,
                Some(&usage(400, 150, 40, 0)),
            ),
            r#"{"timestamp":"2026-01-01T13:00:00.000Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":9999,"outp"#
                .to_string(),
            r#"{"timestamp":"2026-01-01T13:01:00.000Z","type":"response_item","payload":{"type":"message","usage":{"input_tokens":7777,"output_tokens":7777}}}"#
                .to_string(),
            r#"{"timestamp":"2026-01-01T13:02:00.000Z","type":"event_msg","payload":{"type":"agent_message","message":"token_count 9999"}}"#
                .to_string(),
            format!(
                r#"{{"timestamp":{timestamp},"type":"event_msg","payload":{{"type":"token_count","info":{{"last_token_usage":{},"total_token_usage":{}}}}}}}"#,
                usage(600, 200, 60, 0),
                usage(1000, 350, 100, 0),
                timestamp = instant("2026-01-02T08:00:00Z").secs()
            ),
            format!(
                r#"{{"type":"event_msg","payload":{{"type":"token_count","info":{{"last_token_usage":{}}}}}}}"#,
                usage(9999, 0, 9999, 0)
            ),
            format!(
                r#"{{"timestamp":"2026-01-02T08:30:00.000Z","type":"event_msg","payload":{{"type":"token_count","info":{{"model":"gpt-5-mini","last_token_usage":{},"total_token_usage":{}}}}}}}"#,
                usage(200, 0, 20, 0),
                usage(1200, 350, 120, 0)
            ),
        ],
        instant("2026-01-02T08:31:00Z"),
    );

    let twins = count("2025-12-30T23:40:00.000Z", Some(&usage(60, 0, 6, 0)), None);
    write(
        &format!("personal/{main}"),
        &[
            meta("2025-12-30T10:00:00.000Z", MAIN, ""),
            turn("2025-12-30T10:00:01.000Z", &format!(r#""model":"{GPT55}""#)),
            settings("2025-12-30T10:00:30.000Z", r#""service_tier":"default""#),
            first_turn,
            settings("2025-12-30T23:00:00.000Z", r#""service_tier":"ultrafast""#),
            turn("2025-12-30T23:00:01.000Z", &format!(r#""model":"{SOL}""#)),
            count(
                "2025-12-30T23:30:00.000Z",
                Some(&usage(450, 50, 45, 5)),
                Some(&usage(1450, 250, 145, 25)),
            ),
            twins.clone(),
            twins,
        ],
        instant("2025-12-30T23:41:00Z"),
    );

    let mut compacting = vec![meta("2025-12-31T16:00:00.000Z", COMPACTING, "")];
    compacting.extend(compacting_history(&|timestamp| timestamp.to_string()));
    write(
        &format!(
            "work/sessions/{}",
            rollout("2025-12-31", "16-00-00", COMPACTING)
        ),
        &compacting,
        instant("2026-01-01T01:06:00Z"),
    );

    seed_fork_with_copied_compactions(&write);

    seed_archived_copy_with_service_tier(&write);

    write(
        &format!(
            "work/sessions/{}",
            rollout("2025-12-31", "10-00-00", USER_FORK)
        ),
        &[
            meta(
                "2025-12-31T10:00:00.000Z",
                USER_FORK,
                &format!(r#","forked_from_id":"{MAIN}""#),
            ),
            turn("2025-12-31T10:00:00.005Z", &format!(r#""model":"{GPT55}""#)),
            count(
                "2025-12-31T10:00:00.010Z",
                Some(&usage(1000, 200, 100, 20)),
                Some(&usage(1000, 200, 100, 20)),
            ),
            count(
                "2025-12-31T10:00:00.020Z",
                None,
                Some(&usage(3000, 1200, 400, 50)),
            ),
            count(
                "2025-12-31T10:00:00.030Z",
                Some(&usage(500, 100, 50, 0)),
                Some(&usage(3500, 1300, 450, 50)),
            ),
            count(
                "2025-12-31T10:05:00.000Z",
                Some(&usage(400, 100, 40, 0)),
                Some(&usage(3900, 1400, 490, 50)),
            ),
            count(
                "2025-12-31T15:30:00.000Z",
                Some(&usage(700, 300, 70, 10)),
                Some(&usage(4600, 1700, 560, 60)),
            ),
            count(
                "2025-12-31T15:30:01.000Z",
                Some(&usage(700, 300, 70, 10)),
                Some(&usage(5300, 2000, 630, 70)),
            ),
        ],
        instant("2025-12-31T15:31:00Z"),
    );

    write(
        &format!(
            "work/sessions/{}",
            rollout("2026-01-01", "12-05-00", SUBAGENT)
        ),
        &[
            meta(
                "2026-01-01T12:05:00.000Z",
                SUBAGENT,
                &format!(
                    r#","source":{{"subagent":{{"thread_spawn":{{"parent_thread_id":"{MAIN}"}}}}}}"#
                ),
            ),
            turn("2026-01-01T12:05:00.010Z", &format!(r#""model":"{CODEX}""#)),
            settings("2026-01-01T12:05:00.020Z", r#""service_tier":"flex""#),
            count(
                "2026-01-01T12:05:00.100Z",
                Some(&usage(1000, 200, 100, 20)),
                Some(&usage(1000, 200, 100, 20)),
            ),
            count(
                "2026-01-01T12:05:00.200Z",
                None,
                Some(&usage(3000, 1200, 400, 50)),
            ),
            record(
                "2026-01-01T12:05:00.400Z",
                "resp-subagent-own",
                &usage(500, 100, 50, 0),
                "",
            ),
            count(
                "2026-01-01T12:05:00.500Z",
                Some(&usage(500, 100, 50, 0)),
                Some(&usage(3500, 1300, 450, 50)),
            ),
            count(
                "2026-01-01T12:06:00.000Z",
                Some(&usage(250, 50, 25, 5)),
                Some(&usage(3750, 1350, 475, 55)),
            ),
        ],
        instant("2026-01-01T12:07:00Z"),
    );

    write(
        &format!(
            "work/sessions/{}",
            rollout("2026-01-01", "12-05-00", SUBAGENT_SIBLING)
        ),
        &[
            meta(
                "2026-01-01T12:05:00.000Z",
                SUBAGENT_SIBLING,
                &format!(
                    r#","source":{{"subagent":{{"thread_spawn":{{"parent_thread_id":"{MAIN}"}}}}}}"#
                ),
            ),
            turn("2026-01-01T12:05:00.010Z", &format!(r#""model":"{CODEX}""#)),
            settings("2026-01-01T12:05:00.020Z", r#""service_tier":"flex""#),
            record(
                "2026-01-01T12:05:00.400Z",
                "resp-subagent-sibling-own",
                &usage(500, 100, 50, 0),
                "",
            ),
            count(
                "2026-01-01T12:05:00.500Z",
                Some(&usage(500, 100, 50, 0)),
                Some(&usage(500, 100, 50, 0)),
            ),
            record(
                "2026-01-01T12:05:59.900Z",
                "resp-subagent-sibling-later",
                &usage(250, 50, 25, 5),
                "",
            ),
            count(
                "2026-01-01T12:06:00.000Z",
                Some(&usage(250, 50, 25, 5)),
                Some(&usage(750, 150, 75, 5)),
            ),
        ],
        instant("2026-01-01T12:07:00Z"),
    );

    write(
        &format!("work/archived_sessions/rollout-2025-12-30T20-00-00-{ORPHAN_FORK}.jsonl"),
        &[
            meta(
                "2025-12-30T20:00:00.000Z",
                ORPHAN_FORK,
                &format!(r#","forked_from_id":"{MISSING_PARENT}""#),
            ),
            turn("2025-12-30T20:00:00.005Z", &format!(r#""model":"{GPT5}""#)),
            count(
                "2025-12-30T20:00:00.010Z",
                Some(&usage(9999, 0, 999, 0)),
                None,
            ),
            count(
                "2025-12-30T20:00:00.030Z",
                Some(&usage(7777, 0, 777, 0)),
                None,
            ),
            count(
                "2025-12-30T20:00:10.000Z",
                Some(&usage(350, 50, 35, 0)),
                None,
            ),
        ],
        instant("2025-12-30T20:01:00Z"),
    );

    write(
        &format!(
            "work/sessions/{}",
            rollout("2025-12-27", "22-00-00", LEGACY)
        ),
        &[
            meta("2025-12-27T21:59:59.000Z", LEGACY, ""),
            count(
                "2025-12-27T22:00:00.000Z",
                None,
                Some(&usage(100, 0, 50, 0)),
            ),
        ],
        instant("2025-12-27T22:01:00Z"),
    );
    write(
        &format!(
            "work/sessions/{}",
            rollout("2025-12-31", "12-00-00", LEGACY_FORK)
        ),
        &[
            meta(
                "2025-12-31T12:00:00.000Z",
                LEGACY_FORK,
                &format!(r#","forked_from_id":"{LEGACY}""#),
            ),
            turn("2025-12-31T12:00:00.001Z", &format!(r#""model":"{GPT5}""#)),
            count(
                "2025-12-31T12:00:00.005Z",
                None,
                Some(&usage(100, 0, 50, 0)),
            ),
            count(
                "2025-12-31T12:01:00.000Z",
                Some(&usage(130, 30, 13, 0)),
                Some(&usage(230, 30, 63, 0)),
            ),
        ],
        instant("2025-12-31T12:02:00Z"),
    );

    seed_exec_json_with_timestamp_fallback(&write);

    let compressed = rollout("2026-01-02", "09-00-00", COMPRESSED);
    write_bytes(
        &format!("work/sessions/{compressed}.zst"),
        &compress(&[
            meta("2026-01-02T09:00:00.000Z", COMPRESSED, ""),
            turn("2026-01-02T09:00:00.001Z", &format!(r#""model":"{GPT5}""#)),
            count(
                "2026-01-02T09:00:00.000Z",
                Some(&usage(1100, 100, 110, 0)),
                None,
            ),
            turn("2026-01-02T09:10:00.000Z", &format!(r#""model":"{GPT55}""#)),
            count(
                "2026-01-02T09:11:00.000Z",
                Some(&usage(300_000, 250_000, 1000, 0)),
                None,
            ),
            count(
                "2026-01-02T09:12:00.000Z",
                Some(&usage(250_000, 200_000, 500, 0)),
                None,
            ),
        ]),
        instant("2026-01-02T09:01:00Z"),
    );
    write(
        &format!("work/archived_sessions/{compressed}"),
        &[
            turn("2026-01-02T09:00:00.001Z", &format!(r#""model":"{GPT5}""#)),
            count(
                "2026-01-02T09:30:00.000Z",
                Some(&usage(9999, 0, 9999, 0)),
                None,
            ),
        ],
        instant("2026-01-02T09:31:00Z"),
    );

    let both = rollout("2026-01-01", "05-00-00", BOTH_ENCODINGS);
    write(
        &format!("work/sessions/{both}"),
        &[
            format!(
                r#"{{"timestamp": "2026-01-01T14:00:00.000+09:00", "type": "turn_context", "payload": {{"model": "{GPT5}"}}}}"#
            ),
            r#"{"timestamp": "2026-01-01T14:00:00.000+09:00", "type": "event_msg", "payload": {"type": "thread_settings_applied", "thread_settings": {"service_tier": "standard"}}}"#
                .to_string(),
            r#"{"timestamp": "2026-01-01T14:00:00.000+09:00", "type": "event_msg", "payload": {"type": "token_count", "info": {"last_token_usage": {"input_tokens": 210, "cached_input_tokens": 10, "output_tokens": 21}}}}"#
                .to_string(),
        ],
        instant("2026-01-01T05:01:00Z"),
    );
    write_bytes(
        &format!("work/sessions/{both}.zst"),
        &compress(&[
            turn("2026-01-01T05:00:00.000Z", &format!(r#""model":"{GPT5}""#)),
            count(
                "2026-01-01T05:30:00.000Z",
                Some(&usage(7777, 0, 7777, 0)),
                None,
            ),
        ]),
        instant("2026-01-01T05:31:00Z"),
    );

    write(
        "work/sessions/2025/12/30/rollout-2025-12-30T00-00-00-empty.jsonl",
        &[],
        instant("2025-12-30T00:00:00Z"),
    );
    write_bytes(
        "work/sessions/2025/12/31/rollout-2025-12-31T00-00-00-corrupt.jsonl.zst",
        b"not a zstd frame",
        instant("2025-12-31T00:00:00Z"),
    );
    let stray = [
        turn("2025-12-31T08:00:00.000Z", &format!(r#""model":"{GPT5}""#)),
        count(
            "2025-12-31T08:00:01.000Z",
            Some(&usage(9999, 0, 9999, 0)),
            None,
        ),
    ];
    write(
        "work/sessions/2025/12/31/rollout-export.json",
        &stray,
        instant("2025-12-31T08:01:00Z"),
    );
    write_bytes(
        "work/sessions/2025/12/31/notes.zst",
        &compress(&stray),
        instant("2025-12-31T08:01:00Z"),
    );

    let replacement = rollout("2026-01-02", "10-30-00", &format!("{MAIN}_{REPLACEMENT}"));
    write(
        &format!("work/sessions/{replacement}"),
        &[
            meta("2026-01-02T10:30:00.000Z", MAIN, ""),
            turn("2026-01-02T10:30:00.001Z", &format!(r#""model":"{GPT5}""#)),
            count(
                "2026-01-02T10:31:00.000Z",
                Some(&usage(500, 100, 50, 0)),
                None,
            ),
        ],
        instant("2026-01-02T10:32:00Z"),
    );

    let homes = vec![
        root.join("codex/work").into_os_string(),
        root.join("codex/personal").into_os_string(),
    ];
    vec![("CODEX_HOME", homes)]
}

pub(crate) fn extra_cases(_root: &Path) -> Vec<Case> {
    let thread_url = format!("codex://threads/{SUBAGENT}");
    let physical = rollout("2025-12-30", "10-00-00", MAIN).replace(".jsonl", "");
    vec![
        Case::new(
            "weekly",
            "json_start_sunday",
            &["codex", "weekly", "--offline", "--json", "-w", "sunday"],
        ),
        Case::new(
            "statusline",
            "session_physical_json",
            &["codex", "statusline", "--offline", "--json", "--id", &physical],
        ),
        Case::new("statusline", "today_json", &["codex", "statusline", "--offline", "--json"]),
        Case::new("statusline", "today_table", &["codex", "statusline", "--offline"]),
        Case::new("statusline", "today_no_cost", &["codex", "statusline", "--offline", "--no-cost"]),
        Case::new("statusline", "today_kiritimati", &["codex", "statusline", "--offline", "--json", "--timezone", "Pacific/Kiritimati"]),
        Case::new("statusline", "range_json", &["codex", "statusline", "--offline", "--json", "--since", "20251231", "--until", "20260101"]),
        Case::new("statusline", "session_json", &["codex", "statusline", "--offline", "--json", "--id", MAIN]),
        Case::new("statusline", "session_outside_window", &["codex", "statusline", "--offline", "--json", "--id", MAIN, "--since", "20990101", "--until", "20990101"]),
        Case::new("statusline", "session_missing", &["codex", "statusline", "--offline", "--json", "--id", "missing"]),
        Case::new("statusline", "native_provider", &["codex", "statusline", "--offline", "--mcp"])
            .stdin(concat!(
                "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/list\"}\n",
                "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"resources/list\"}\n",
                "{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"resources/read\",\"params\":{\"uri\":\"ui://ccusage/codex-statusline\"}}\n",
                "{\"jsonrpc\":\"2.0\",\"id\":4,\"method\":\"tools/call\",\"params\":{\"name\":\"ccusage_codex_statusline\"}}\n"
            )),
        Case::new(
            "daily",
            "json_speed_standard",
            &[
                "codex",
                "daily",
                "--offline",
                "--json",
                "--speed",
                "standard",
            ],
        ),
        Case::new(
            "daily",
            "json_speed_fast",
            &["codex", "daily", "--offline", "--json", "--speed", "fast"],
        ),
        Case::new(
            "daily",
            "json_speed_flex",
            &["codex", "daily", "--offline", "--json", "--speed", "flex"],
        ),
        Case::new(
            "daily",
            "speed_invalid",
            &["codex", "daily", "--offline", "--json", "--speed", "warp"],
        ),
        Case::new(
            "daily",
            "json_single_thread",
            &["codex", "daily", "--offline", "--json", "--single-thread"],
        ),
        Case::new(
            "daily",
            "id_rejected",
            &["codex", "daily", "--offline", "--json", "--id", MAIN],
        ),
        Case::new(
            "session",
            "json_single_thread",
            &["codex", "session", "--offline", "--json", "--single-thread"],
        ),
        Case::new(
            "session",
            "id_json",
            &["codex", "session", "--offline", "--json", "--id", MAIN],
        ),
        Case::new(
            "session",
            "id_json_thread_url",
            &[
                "codex",
                "session",
                "--offline",
                "--json",
                "--id",
                &thread_url,
            ],
        ),
        Case::new(
            "session",
            "id_json_file_name",
            &[
                "codex",
                "session",
                "--offline",
                "--json",
                "--id",
                "headless-run.jsonl",
            ],
        ),
        Case::new(
            "session",
            "id_table_breakdown",
            &[
                "codex",
                "session",
                "--offline",
                "--breakdown",
                "--id",
                COMPACTING,
            ],
        ),
        Case::new(
            "session",
            "id_json_missing",
            &[
                "codex",
                "session",
                "--offline",
                "--json",
                "--id",
                "sess-missing",
            ],
        ),
    ]
}
