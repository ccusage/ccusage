use std::ffi::OsString;
use std::path::Path;

use ccusage_test_support::instant;

use crate::golden::{Case, json_path};

const SONNET: &str = "claude-sonnet-4-20250514";
const OPUS: &str = "claude-opus-4-20250514";
const HAIKU: &str = "claude-haiku-4-5-20251001";
const GPT5: &str = "gpt-5";
const UNPRICED: &str = "pi-mystery-1";
const UNPRICED_2: &str = "pi-mystery-2";

const ALPHA: &str = "pi/sessions/--Users-dev-alpha--";
const BETA: &str = "pi/sessions/--Users-dev-beta--";
const LAB: &str = "pi/sessions/--Users-dev-lab--";
const OUTSIDE_ROOT: &str = "pi/outside/extra-project/outside-root.jsonl";
const NAMED_STORE: &str = "pi/omp";
const NAMED_STORE_CONFIG: &str = "pi/config/ccusage.json";

fn header(id: &str, timestamp: Option<&str>, parent: Option<&str>) -> String {
    let timestamp = timestamp.map_or_else(String::new, |ts| format!(r#","timestamp":"{ts}""#));
    let parent = parent.map_or_else(String::new, |raw| format!(r#","parentSession":{raw}"#));
    format!(r#"{{"type":"session","version":3,"id":"{id}"{timestamp}{parent}}}"#)
}

fn tokens(input: u64, output: u64, cache_read: u64, cache_write: u64) -> String {
    format!(
        r#""input":{input},"output":{output},"cacheRead":{cache_read},"cacheWrite":{cache_write},"totalTokens":{}"#,
        input + output + cache_read + cache_write
    )
}

fn with_cost(usage: &str, cost: &str) -> String {
    format!(r#"{usage},"cost":{{"input":0.001,"output":0.002,"total":{cost}}}"#)
}

fn message(timestamp: &str, model: &str, usage: &str) -> String {
    format!(
        r#"{{"type":"message","timestamp":"{timestamp}","message":{{"role":"assistant","model":"{model}","usage":{{{usage}}}}}}}"#
    )
}

fn linked(id: &str, parent: &str, timestamp: &str, model: &str, usage: &str) -> String {
    format!(
        r#"{{"type":"message","id":"{id}","parentId":{parent},"timestamp":"{timestamp}","message":{{"role":"assistant","model":"{model}","usage":{{{usage}}}}}}}"#
    )
}

pub(crate) fn seed(root: &Path) -> Vec<(&'static str, Vec<OsString>)> {
    let write = |relative: &str, lines: &[String]| {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().expect("fixture file has a parent"))
            .expect("failed to create fixture directory");
        std::fs::write(&path, lines.join("\n")).expect("failed to write fixture file");
    };

    seed_main_sessions(root, &write);
    seed_fork_lab(root, &write);
    seed_named_store(root, &write);

    vec![(
        "PI_AGENT_DIR",
        vec![root.join("pi/sessions").into_os_string()],
    )]
}

pub(crate) fn seed_tied_fork(
    root: &Path,
    parent_first: bool,
) -> Vec<(&'static str, Vec<OsString>)> {
    let sessions = root.join("pi/sessions/project");
    std::fs::create_dir_all(&sessions).expect("create Pi fixture session directory");
    let timestamp = "2025-12-27T05:00:00.000Z";
    let usage = tokens(1000, 0, 0, 0);
    let parent = [
        header("shared", Some("2025-12-27T00:00:00.000Z"), None),
        message(timestamp, SONNET, &with_cost(&usage, "0.003")),
    ];
    let child = [
        header(
            "shared",
            Some("2025-12-27T04:00:00.000Z"),
            Some(r#""z_shared.jsonl""#),
        ),
        message(timestamp, SONNET, &usage),
    ];
    let mut files = [
        ("z_shared.jsonl", parent.join("\n")),
        ("a_shared.jsonl", child.join("\n")),
    ];
    if !parent_first {
        files.reverse();
    }
    for (name, contents) in files {
        std::fs::write(sessions.join(name), contents).expect("write Pi fixture session");
    }
    vec![(
        "PI_AGENT_DIR",
        vec![root.join("pi/sessions").into_os_string()],
    )]
}

pub(crate) const STORE_WITH_SESSIONS_PROJECT: &str = "pi-store";
pub(crate) const AGENT_DIR: &str = "pi-agent";

pub(crate) fn seed_store_with_sessions_project(root: &Path) -> Vec<(&'static str, Vec<OsString>)> {
    let session = |relative: &str, id: &str, input: u64| {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().expect("fixture file has a parent"))
            .expect("create Pi fixture session directory");
        let lines = [
            header(id, Some("2025-12-27T00:00:00.000Z"), None),
            message("2025-12-27T05:00:00.000Z", SONNET, &tokens(input, 0, 0, 0)),
        ];
        std::fs::write(path, lines.join("\n")).expect("write Pi fixture session");
    };
    session(
        &format!("{STORE_WITH_SESSIONS_PROJECT}/sessions/in-sessions.jsonl"),
        "in-sessions",
        1000,
    );
    session(
        &format!("{STORE_WITH_SESSIONS_PROJECT}/alpha/in-alpha.jsonl"),
        "in-alpha",
        2000,
    );
    session(
        &format!("{AGENT_DIR}/sessions/beta/in-agent.jsonl"),
        "in-agent",
        4000,
    );
    std::fs::create_dir_all(root.join("pi-empty")).expect("create empty Pi store");
    vec![("PI_AGENT_DIR", vec![root.join("pi-empty").into_os_string()])]
}

fn seed_main_sessions(root: &Path, write: &impl Fn(&str, &[String])) {
    let main_file = format!("{ALPHA}/2025-12-30T09-59-00-000Z_sess-alpha-main.jsonl");
    let a1 = linked(
        "a1",
        r#""u1""#,
        "2025-12-30T10:00:00.000Z",
        SONNET,
        &with_cost(&tokens(1000, 200, 4000, 300), "0.05"),
    );
    let a3 = linked(
        "a3",
        r#""mc1""#,
        "2025-12-31T15:30:00.000Z",
        SONNET,
        &with_cost(&tokens(700, 150, 5000, 100), "0.02"),
    );
    let a4 = linked(
        "a4",
        r#""a3""#,
        "2026-01-01T12:00:00.000Z",
        UNPRICED,
        &tokens(300, 70, 0, 0),
    );
    let user = r#"{"type":"message","id":"u1","parentId":null,"timestamp":"2025-12-30T09:59:30.000Z","message":{"role":"user","content":"hello"}}"#.to_string();
    let model_change = format!(
        r#"{{"type":"model_change","id":"mc1","parentId":"a1","timestamp":"2025-12-30T10:06:00.000Z","message":{{"role":"assistant","model":"{SONNET}","usage":{{{}}}}}}}"#,
        tokens(9999, 9999, 0, 0)
    );
    write(
        &main_file,
        &[
            header("sess-alpha-main", Some("2025-12-30T09:59:00.000Z"), None),
            user.clone(),
            a1.clone(),
            linked(
                "a2",
                r#""a1""#,
                "2025-12-30T10:05:00.000Z",
                OPUS,
                &with_cost(&tokens(120, 30, 0, 0), "0.9"),
            ),
            model_change.clone(),
            a3.clone(),
            a4.clone(),
            r#"{"type":"compaction","id":"c1","parentId":"a4","timestamp":"2026-01-01T12:30:00.000Z","summary":"compacted","tokensBefore":7777}"#.to_string(),
            linked(
                "a5",
                r#""c1""#,
                "2026-01-02T09:00:00.000Z",
                GPT5,
                &with_cost(&tokens(800, 300, 6000, 0), "0.5"),
            ),
        ],
    );

    write(
        &format!("{ALPHA}/2026-01-01T13-00-00-000Z_sess-alpha-fork.jsonl"),
        &[
            header(
                "sess-alpha-fork",
                Some("2026-01-01T13:00:00.000Z"),
                Some(&json_path(&root.join(&main_file))),
            ),
            user,
            a1,
            model_change,
            a3,
            a4,
            linked(
                "f5",
                r#""a4""#,
                "2026-01-01T13:10:00.000Z",
                HAIKU,
                &with_cost(&tokens(900, 400, 7000, 200), "0.03"),
            ),
            linked(
                "f6",
                r#""f5""#,
                "2026-01-02T10:30:00.000Z",
                SONNET,
                &tokens(250, 50, 0, 0),
            ),
        ],
    );

    write(
        &format!("{ALPHA}/sess-alpha-main/run-abc/run-0/session.jsonl"),
        &[message(
            "2025-12-31T16:00:00.000Z",
            HAIKU,
            &tokens(50, 5, 6, 1),
        )],
    );
    write(
        &format!("{ALPHA}/sess-alpha-main/subagent-artifacts/run1_agent_0_transcript.jsonl"),
        &[message(
            "2025-12-31T16:00:00.000Z",
            HAIKU,
            &tokens(9999, 9999, 0, 0),
        )],
    );

    let replayed = format!(
        r#"{{"timestamp":"2025-12-31T23:50:00.000Z","message":{{"role":"assistant","model":"{HAIKU}","usage":{{{}}}}}}}"#,
        with_cost(&tokens(900, 400, 7000, 200), "2.0")
    );
    write(
        &format!("{BETA}/2025-12-31T23-40-00-000Z_sess-beta-legacy.jsonl"),
        &[
            replayed.clone(),
            replayed,
            message(
                "2025-12-31T23:50:00.000Z",
                HAIKU,
                &with_cost(&tokens(900, 401, 7000, 200), "2.0"),
            ),
            format!(
                r#"{{"type":"message","timestamp":"2025-12-31T23:55:00.000Z","message":{{"role":"user","model":"{SONNET}","usage":{{{}}}}}}}"#,
                tokens(7777, 7777, 0, 0)
            ),
            format!(
                r#"{{"type":"custom_message","timestamp":"2025-12-31T23:56:00.000Z","message":{{"role":"assistant","model":"{SONNET}","usage":{{{}}}}}}}"#,
                tokens(9999, 9999, 0, 0)
            ),
            format!(
                r#"{{"type":"message","timestamp":"2025-12-31T23:57:00.000Z","message":{{"role":"assistant","model":"{SONNET}","content":"no usage block"}}}}"#
            ),
            message("2025-12-31T23:58:00.000Z", SONNET, &tokens(0, 0, 0, 0)),
            format!(
                r#"{{"type":"message","message":{{"role":"assistant","model":"{SONNET}","usage":{{{}}}}}}}"#,
                tokens(8888, 8888, 0, 0)
            ),
            message("2025-12-31 12:00:00", SONNET, &tokens(6666, 6666, 0, 0)),
            format!(
                r#"{{"type":"message","timestamp":{timestamp},"message":{{"role":"assistant","model":"{SONNET}","usage":{{{}}}}}}}"#,
                tokens(5555, 5555, 0, 0),
                timestamp = instant("2026-01-01T00:00:00Z").millis()
            ),
            String::new(),
            "not json".to_string(),
            r#"["usage","message"]"#.to_string(),
            message(
                "2026-01-01T09:10:00.000+09:00",
                SONNET,
                &tokens(600, 100, 3000, 0),
            ),
            message("2026-01-01T00:20:00.000Z", GPT5, r#""totalTokens":500"#),
            message(
                "2026-01-01T00:30:00Z",
                OPUS,
                r#""input":100,"output":50,"cacheRead":1000,"cacheWrite":0,"totalTokens":1400,"cost":0.07"#,
            ),
            message(
                "2026-01-01T00:40:00.000Z",
                SONNET,
                r#""input":"9999","output":60,"cacheRead":-5,"cacheWrite":1.5"#,
            ),
            format!(
                r#"{{"type":"message","timestamp":"2026-01-01T00:50:00.000Z","message":{{"role":"assistant","usage":{{{}}}}}}}"#,
                tokens(15, 5, 0, 0)
            ),
            message(
                "2026-01-02T10:00:00.000Z",
                SONNET,
                &with_cost(&tokens(500, 100, 0, 0), "-1.25"),
            ),
            message(
                "2026-01-02T10:10:00.000Z",
                HAIKU,
                &with_cost(&tokens(40, 10, 0, 0), r#""0.99""#),
            ),
            message(
                "2026-01-02T10:20:00.000Z",
                OPUS,
                &with_cost(&tokens(2000, 500, 0, 0), "0"),
            ),
            format!(
                r#"{{"type":"message","timestamp":"2026-01-02T11:00:00.000Z","message":{{"role":"assistant","model":"{GPT5}","usage":{{"input":9999,"outp"#
            ),
        ],
    );

    let shared = message(
        "2025-12-30T08:10:00.000Z",
        SONNET,
        &with_cost(&tokens(400, 80, 2000, 50), "0.01"),
    );
    let shared_header = || header("sess-beta-shared", Some("2025-12-30T08:00:00.000Z"), None);
    write(
        &format!("{BETA}/2025-12-30T08-00-00-000Z_sess-beta-shared.jsonl"),
        &[shared_header(), shared.clone()],
    );
    write(
        &format!("{BETA}/2025-12-30T20-00-00-000Z_sess-beta-shared.jsonl"),
        &[
            shared_header(),
            shared.clone(),
            message(
                "2025-12-30T20:10:00.000Z",
                OPUS,
                &with_cost(&tokens(150, 60, 0, 0), "0.2"),
            ),
        ],
    );
    write(
        &format!("{ALPHA}/2025-12-30T08-00-00-000Z_sess-beta-shared.jsonl"),
        &[shared_header(), shared],
    );

    write(
        &format!("{BETA}/2025-12-31T00-00-00-000Z_sess-beta-empty.jsonl"),
        &[],
    );
    write(
        &format!("{BETA}/2025-12-31T01-00-00-000Z_sess-beta-header-only.jsonl"),
        &[
            header(
                "sess-beta-header-only",
                Some("2025-12-31T01:00:00.000Z"),
                None,
            ),
            r#"{"type":"label","label":"usage message","value":9999}"#.to_string(),
        ],
    );
    let stray = [message(
        "2025-12-31T12:00:00.000Z",
        SONNET,
        &tokens(9999, 9999, 0, 0),
    )];
    write(&format!("{BETA}/notes.txt"), &stray);
    write(&format!("{BETA}/backup.jsonl.bak"), &stray);
    write("pi/sessions/index.json", &stray);

    write(
        "pi/sessions/loose.jsonl",
        &[message(
            "2026-01-02T12:00:00.000Z",
            GPT5,
            &tokens(30, 3, 0, 0),
        )],
    );
}

fn lab(name: &str) -> String {
    format!("{LAB}/{name}.jsonl")
}

fn relative(name: &str) -> String {
    format!(r#""{name}.jsonl""#)
}

fn seed_fork_with_parent_outside_store(
    root: &Path,
    write: &impl Fn(&str, &[String]),
    day_start: Option<&str>,
) {
    let outside = message("2025-12-27T11:30:00.000Z", UNPRICED, &tokens(25, 8, 0, 0));
    write(
        OUTSIDE_ROOT,
        &[
            header("outside-root", day_start, None),
            outside.clone(),
            message("2025-12-27T12:50:00.000Z", UNPRICED, &tokens(5, 5, 0, 0)),
        ],
    );
    write(
        &lab("lab-outside-child"),
        &[
            header(
                "lab-outside-child",
                Some("2025-12-27T12:00:00.000Z"),
                Some(&json_path(&root.join(OUTSIDE_ROOT))),
            ),
            outside,
            message("2025-12-27T11:40:00.000Z", UNPRICED, &tokens(2, 1, 0, 0)),
        ],
    );
}

fn seed_active_branch_and_partial_tree_replays(
    write: &impl Fn(&str, &[String]),
    day_start: Option<&str>,
) {
    let tree = |id: &str, parent: &str, time: &str, input: u64| {
        linked(
            id,
            parent,
            &format!("2025-12-27T{time}:00.000Z"),
            UNPRICED,
            &tokens(input, 10, 20, 3),
        )
    };
    write(
        &lab("lab-tree-root"),
        &[
            header("lab-tree-root", day_start, None),
            tree("t-unrelated", "null", "12:00", 50),
            tree("t-a", "null", "12:10", 100),
            tree("t-x", r#""t-a""#, "12:20", 200),
            tree("t-y", r#""t-a""#, "12:30", 300),
            tree("t-z", r#""t-y""#, "12:40", 400),
        ],
    );
    write(
        &lab("lab-tree-child"),
        &[
            header(
                "lab-tree-child",
                Some("2025-12-27T13:00:00.000Z"),
                Some(&relative("lab-tree-root")),
            ),
            tree("copy-a", "null", "12:10", 100),
            tree("copy-y", r#""copy-a""#, "12:30", 300),
            tree("copy-z", r#""copy-y""#, "12:40", 400),
            tree("child-only", r#""copy-z""#, "13:10", 500),
        ],
    );

    let partial_plain = message("2025-12-27T14:10:00.000Z", UNPRICED, &tokens(27, 9, 0, 0));
    write(
        &lab("lab-tree-partial"),
        &[
            header("lab-tree-partial", day_start, None),
            tree("pt1", "null", "14:00", 26),
            partial_plain.clone(),
        ],
    );
    write(
        &lab("lab-tree-partial-child"),
        &[
            header(
                "lab-tree-partial-child",
                Some("2025-12-27T14:20:00.000Z"),
                Some(&relative("lab-tree-partial")),
            ),
            message("2025-12-27T14:00:00.000Z", UNPRICED, &tokens(26, 10, 20, 3)),
            partial_plain,
        ],
    );
}

fn seed_fork_with_missing_and_zero_cost(write: &impl Fn(&str, &[String]), day_start: Option<&str>) {
    let zero_usage = tokens(1000, 100, 0, 0);
    write(
        &lab("lab-zero-root"),
        &[
            header("lab-zero-root", day_start, None),
            message("2025-12-27T14:30:00.000Z", GPT5, &zero_usage),
        ],
    );
    write(
        &lab("lab-zero-child"),
        &[
            header(
                "lab-zero-child",
                Some("2025-12-27T14:40:00.000Z"),
                Some(&relative("lab-zero-root")),
            ),
            message(
                "2025-12-27T14:30:00.000Z",
                GPT5,
                &with_cost(&zero_usage, "0"),
            ),
            message(
                "2025-12-27T14:50:00.000Z",
                GPT5,
                &with_cost(&tokens(10, 1, 0, 0), "0.125"),
            ),
        ],
    );
}

fn seed_fork_lab(root: &Path, write: &impl Fn(&str, &[String])) {
    let day_start = Some("2025-12-27T00:00:00.000Z");

    let cost_usage = tokens(10, 20, 30, 40);
    write(
        &lab("lab-cost-root"),
        &[
            header("lab-cost-root", day_start, None),
            message(
                "2025-12-27T01:00:00.000Z",
                UNPRICED,
                &with_cost(&cost_usage, "1.0"),
            ),
        ],
    );
    write(
        &lab("lab-cost-child"),
        &[
            header(
                "lab-cost-child",
                Some("2025-12-27T01:30:00.000Z"),
                Some(&relative("lab-cost-root")),
            ),
            message(
                "2025-12-27T01:00:00.000Z",
                UNPRICED,
                &with_cost(&cost_usage, "2.0"),
            ),
            message(
                "2025-12-27T01:40:00.000Z",
                UNPRICED,
                &with_cost(&tokens(1, 2, 3, 4), "0.25"),
            ),
        ],
    );

    let p1 = message(
        "2025-12-27T02:00:00.000Z",
        UNPRICED,
        &tokens(100, 10, 20, 3),
    );
    let p2 = message(
        "2025-12-27T03:00:00.000Z",
        UNPRICED_2,
        &tokens(200, 20, 30, 4),
    );
    let p3 = message("2025-12-27T05:00:00.000Z", UNPRICED, &tokens(50, 5, 6, 1));
    write(
        &lab("lab-root"),
        &[
            header("lab-root", day_start, None),
            p1.clone(),
            p2.clone(),
            p3.clone(),
        ],
    );

    let child_own = message("2025-12-27T06:00:00.000Z", UNPRICED, &tokens(70, 7, 8, 2));
    write(
        &lab("lab-child-exact"),
        &[
            header(
                "lab-child-exact",
                Some("2025-12-27T04:00:00.000Z"),
                Some(&relative("lab-root")),
            ),
            p1.clone(),
            p2.clone(),
            p3.clone(),
            child_own.clone(),
        ],
    );
    write(
        &lab("lab-child-nearmiss"),
        &[
            header(
                "lab-child-nearmiss",
                Some("2025-12-27T04:00:00.000Z"),
                Some(&json_path(&root.join(lab("lab-root")))),
            ),
            p1.clone(),
            message(
                "2025-12-27T03:00:00.000Z",
                UNPRICED_2,
                &tokens(200, 20, 30, 999),
            ),
            message("2025-12-27T06:30:00.000Z", UNPRICED, &tokens(71, 7, 0, 0)),
        ],
    );
    write(
        &lab("lab-grandchild"),
        &[
            header(
                "lab-grandchild",
                Some("2025-12-27T07:00:00.000Z"),
                Some(&relative("lab-child-exact")),
            ),
            p1.clone(),
            p2.clone(),
            p3,
            child_own,
            message(
                "2025-12-27T08:00:00.000Z",
                UNPRICED,
                &tokens(400, 40, 50, 6),
            ),
        ],
    );
    write(
        &lab("lab-orphan"),
        &[
            header(
                "lab-orphan",
                Some("2025-12-27T04:00:00.000Z"),
                Some(&relative("missing")),
            ),
            p1.clone(),
        ],
    );
    let duplicate_id = |line: &str| {
        line.replacen(
            r#"{"type":"message","#,
            r#"{"type":"message","id":"d1","parentId":null,"#,
            1,
        )
    };
    write(
        &lab("lab-dupid-child"),
        &[
            header(
                "lab-dupid-child",
                Some("2025-12-27T04:00:00.000Z"),
                Some(&relative("lab-root")),
            ),
            duplicate_id(&p1),
            duplicate_id(&p2),
        ],
    );

    let bad_parent = message("2025-12-27T09:00:00.000Z", UNPRICED, &tokens(21, 4, 0, 0));
    write(
        &lab("lab-badparent"),
        &[
            header("lab-badparent", day_start, Some("42")),
            bad_parent.clone(),
        ],
    );
    write(
        &lab("lab-badparent-child"),
        &[
            header(
                "lab-badparent-child",
                Some("2025-12-27T09:10:00.000Z"),
                Some(&relative("lab-badparent")),
            ),
            bad_parent,
        ],
    );

    let no_timestamp = message("2025-12-27T09:30:00.000Z", UNPRICED, &tokens(22, 5, 0, 0));
    write(
        &lab("lab-nots-root"),
        &[header("lab-nots-root", None, None), no_timestamp.clone()],
    );
    write(
        &lab("lab-nots-child"),
        &[
            header(
                "lab-nots-child",
                Some("2025-12-27T10:00:00.000Z"),
                Some(&relative("lab-nots-root")),
            ),
            no_timestamp,
        ],
    );

    write(
        &lab("lab-self"),
        &[
            header("lab-self", day_start, Some(&relative("lab-self"))),
            message("2025-12-27T10:30:00.000Z", UNPRICED, &tokens(23, 6, 0, 0)),
        ],
    );
    let cycle = message("2025-12-27T11:00:00.000Z", UNPRICED, &tokens(24, 7, 0, 0));
    write(
        &lab("lab-cycle-a"),
        &[
            header("lab-cycle-a", day_start, Some(&relative("lab-cycle-b"))),
            cycle.clone(),
        ],
    );
    write(
        &lab("lab-cycle-b"),
        &[
            header("lab-cycle-b", day_start, Some(&relative("lab-cycle-a"))),
            cycle,
        ],
    );

    seed_fork_with_parent_outside_store(root, write, day_start);

    seed_active_branch_and_partial_tree_replays(write, day_start);

    seed_fork_with_missing_and_zero_cost(write, day_start);
}

fn seed_named_store(root: &Path, write: &impl Fn(&str, &[String])) {
    let store = |name: &str| format!("{NAMED_STORE}/project-x/{name}.jsonl");
    let copied = message(
        "2025-12-30T11:00:00.000Z",
        SONNET,
        &with_cost(&tokens(1000, 100, 500, 50), "0.5"),
    );
    write(
        &store("omp-root"),
        &[
            header("omp-root", Some("2025-12-30T00:00:00.000Z"), None),
            copied.clone(),
            message("2025-12-31T18:00:00.000Z", GPT5, &tokens(200, 20, 0, 0)),
            message("2026-01-01T08:00:00.000Z", UNPRICED, &tokens(30, 3, 0, 0)),
            message(
                "2026-01-02T08:00:00.000Z",
                OPUS,
                &with_cost(&tokens(100, 10, 0, 0), "-1.0"),
            ),
        ],
    );
    write(
        &store("omp-child"),
        &[
            header(
                "omp-child",
                Some("2025-12-31T12:00:00.000Z"),
                Some(r#""omp-root.jsonl""#),
            ),
            copied,
            message("2025-12-31T12:30:00.000Z", HAIKU, &tokens(60, 6, 0, 0)),
        ],
    );
    write(
        &format!("{NAMED_STORE}/project-x/omp-root/subagent-artifacts/transcript.jsonl"),
        &[message(
            "2025-12-31T12:30:00.000Z",
            HAIKU,
            &tokens(9999, 9999, 0, 0),
        )],
    );
    write(
        NAMED_STORE_CONFIG,
        &[format!(
            r#"{{"pi":{{"stores":[{{"name":"omp","path":{}}}]}}}}"#,
            json_path(&root.join(NAMED_STORE))
        )],
    );
}

pub(crate) fn extra_cases(root: &Path) -> Vec<Case> {
    let path = |relative: &str| root.join(relative).to_string_lossy().into_owned();
    let multi = [
        path("pi/sessions"),
        path("pi/outside"),
        path("pi/missing"),
        path("pi/sessions"),
    ]
    .join(",");
    let outside = path("pi/outside");
    let missing = path("pi/missing");
    let config = path(NAMED_STORE_CONFIG);

    vec![
        Case::new(
            "daily",
            "json_pi_path_multi",
            &["pi", "daily", "--offline", "--json", "--pi-path", &multi],
        ),
        Case::new(
            "session",
            "json_pi_path_multi",
            &["pi", "session", "--offline", "--json", "--pi-path", &multi],
        ),
        Case::new(
            "session",
            "json_pi_path_outside",
            &[
                "pi",
                "session",
                "--offline",
                "--json",
                "--pi-path",
                &outside,
            ],
        ),
        Case::new(
            "daily",
            "table_pi_path_missing",
            &["pi", "daily", "--offline", "--pi-path", &missing],
        ),
        Case::new(
            "daily",
            "json_named_store",
            &["daily", "--offline", "--json", "--config", &config],
        ),
        Case::new(
            "session",
            "json_named_store_mode_calculate",
            &[
                "session",
                "--offline",
                "--json",
                "--mode",
                "calculate",
                "--config",
                &config,
            ],
        ),
    ]
}
