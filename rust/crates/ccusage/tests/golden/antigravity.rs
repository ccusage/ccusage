use std::ffi::OsString;
use std::path::Path;

use ccusage_test_support::{Instant, instant, set_modified};

use crate::golden::Case;

const API_PROVIDER_GOOGLE_GEMINI: u64 = 24;

const SONNET: &str = "claude-sonnet-4-20250514";
const HAIKU: &str = "claude-haiku-4-5-20251001";
const UNPRICED: &str = "antigravity-mystery-1";

pub(crate) fn seed(root: &Path) -> Vec<(&'static str, Vec<OsString>)> {
    let ide = root.join("antigravity/ide");
    let conversations = ide.join("conversations");
    let cli = root.join("antigravity/cli-conversations");
    std::fs::create_dir_all(conversations.join("archive"))
        .expect("failed to create Antigravity conversations directory");
    std::fs::create_dir_all(&cli).expect("failed to create Antigravity CLI directory");

    seed_beta(&cli.join("session-beta.db"));
    seed_alpha(&conversations.join("session-alpha.db"));
    seed_gamma(&conversations.join("session-gamma.db"));
    seed_delta(&conversations.join("archive/session-delta.db"));
    seed_ignored(&ide, &conversations);

    let data_dirs = vec![ide.into_os_string(), cli.into_os_string()];
    vec![("ANTIGRAVITY_DATA_DIR", data_dirs)]
}

pub(crate) fn extra_cases(_root: &Path) -> Vec<Case> {
    vec![
        Case::new(
            "session",
            "json_debug",
            &["antigravity", "session", "--offline", "--json", "--debug"],
        ),
        Case::new(
            "session",
            "json_warn",
            &["antigravity", "session", "--offline", "--json"],
        )
        .env("LOG_LEVEL", "3"),
        Case::new(
            "session",
            "unified_json_debug_warn",
            &["session", "--offline", "--json", "--debug"],
        )
        .env("LOG_LEVEL", "3"),
    ]
}

fn seed_beta(path: &Path) {
    let generations = [
        Generation {
            model: Some(SONNET),
            timestamp: Some((instant("2025-12-31T16:00:00Z").secs(), 0)),
            usage: Some(Usage {
                input: 400,
                total_output: 90,
                visible_output: 90,
                response_id: Some("resp-alpha-2"),
                ..Usage::default()
            }),
            ..Generation::default()
        },
        Generation {
            timestamp: Some((instant("2026-01-01T01:00:00Z").secs(), 0)),
            usage: Some(Usage {
                input: 400,
                total_output: 90,
                visible_output: 90,
                response_id: Some("resp-beta-near-miss"),
                ..Usage::default()
            }),
            ..Generation::default()
        },
        Generation {
            model: Some("GPT 5"),
            timestamp: Some((instant("2026-01-01T02:00:00Z").secs(), 0)),
            usage: Some(Usage {
                input: 30,
                total_output: 7,
                visible_output: 7,
                response_id: Some("resp-bridge"),
                ..Usage::default()
            }),
            ..Generation::default()
        },
        Generation {
            timestamp: Some((instant("2026-01-01T02:05:00Z").secs(), 0)),
            usage: Some(Usage {
                input: 20,
                total_output: 5,
                visible_output: 5,
                message_id: Some("msg-bridge"),
                ..Usage::default()
            }),
            ..Generation::default()
        },
        Generation {
            timestamp: Some((instant("2026-01-01T02:10:00Z").secs(), 0)),
            usage: Some(Usage {
                input: 10,
                total_output: 9,
                visible_output: 9,
                response_id: Some("resp-bridge"),
                message_id: Some("msg-bridge"),
                ..Usage::default()
            }),
            ..Generation::default()
        },
        Generation {
            timestamp: Some((instant("2026-01-02T08:00:00Z").secs(), 0)),
            usage: Some(Usage {
                model_id: Some(1499),
                input: 60,
                total_output: 15,
                visible_output: 15,
                response_id: Some("resp-beta-placeholder"),
                ..Usage::default()
            }),
            ..Generation::default()
        },
        Generation {
            model: Some(UNPRICED),
            model_id: Some(77),
            timestamp: Some((instant("2026-01-01T12:30:00Z").secs(), 0)),
            usage: Some(Usage {
                model_id: Some(77),
                input: 300,
                total_output: 70,
                visible_output: 70,
                cache_read: 1_000,
                response_id: Some("resp-beta-mystery"),
                ..Usage::default()
            }),
            ..Generation::default()
        },
    ];
    create_database(
        path,
        instant("2026-01-02T12:00:00Z"),
        &generation_rows(&generations),
        Some(&[]),
        Some(&[]),
    );
}

fn seed_alpha(path: &Path) {
    let first = Usage {
        input: 1_000,
        total_output: 300,
        cache_creation: 50,
        cache_read: 4_000,
        reasoning: 100,
        visible_output: 200,
        message_id: Some("msg-alpha-1"),
        response_id: Some("resp-alpha-1"),
        provider_message_id: Some("provider-alpha-1"),
        ..Usage::default()
    };
    let second = Usage {
        input: 500,
        total_output: 120,
        reasoning: 20,
        response_id: Some("resp-alpha-2"),
        ..Usage::default()
    };
    let unkeyed = Usage {
        input: 800,
        total_output: 60,
        visible_output: 60,
        cache_read: 2_000,
        ..Usage::default()
    };
    let unkeyed_small = Usage {
        input: 70,
        total_output: 30,
        visible_output: 30,
        ..Usage::default()
    };

    let steps = [
        (
            1,
            Some(
                Step {
                    model: Some("gemini-3-pro-high"),
                    model_in_field_8: true,
                    provider: Some(API_PROVIDER_GOOGLE_GEMINI),
                    timestamp: Some((instant("2025-12-30T09:59:00Z").secs() + 59, 0)),
                    usage: Some(Usage {
                        cache_read: 4_500,
                        ..first
                    }),
                    ..Step::default()
                }
                .blob(),
            ),
        ),
        (2, None),
        (
            3,
            Some(
                Step {
                    model: Some("gemini-3-flash-agent"),
                    timestamp: Some((instant("2025-12-31T10:00:00Z").secs(), 0)),
                    timestamp_in_field_1: true,
                    usage: Some(unkeyed),
                    ..Step::default()
                }
                .blob(),
            ),
        ),
        (
            4,
            Some(
                Step {
                    timestamp: Some((instant("2026-01-01T09:00:00Z").secs(), 0)),
                    retries: vec![Usage {
                        input: 11,
                        total_output: 22,
                        visible_output: 22,
                        response_id: Some("resp-alpha-step-retry"),
                        ..Usage::default()
                    }],
                    ..Step::default()
                }
                .blob(),
            ),
        ),
        (
            5,
            Some(
                Step {
                    timestamp: Some((instant("2026-01-01T09:30:00Z").secs(), 0)),
                    usage: Some(Usage {
                        response_id: Some("resp-alpha-step-empty"),
                        ..Usage::default()
                    }),
                    ..Step::default()
                }
                .blob(),
            ),
        ),
    ];

    let generations = [
        Generation {
            model: Some("Gemini 3 Pro"),
            timestamp: Some((instant("2025-12-30T10:00:00Z").secs(), 250_000_000)),
            usage: Some(first),
            ..Generation::default()
        },
        Generation {
            timestamp: Some((instant("2025-12-31T15:30:00Z").secs(), 0)),
            usage: Some(second),
            retries: vec![
                second,
                Usage {
                    input: 500,
                    total_output: 10,
                    visible_output: 10,
                    response_id: Some("resp-alpha-2-attempt-1"),
                    ..Usage::default()
                },
            ],
            ..Generation::default()
        },
        Generation {
            model: Some("gemini-3-flash-agent"),
            model_in_field_21: true,
            timestamp: Some((instant("2025-12-31T10:00:00Z").secs() + 5, 0)),
            usage: Some(unkeyed),
            retries: vec![unkeyed],
            step_indices: vec![3],
            ..Generation::default()
        },
        Generation {
            timestamp: Some((instant("2025-12-31T11:00:00Z").secs(), 0)),
            usage: Some(unkeyed_small),
            step_indices: vec![7, 8],
            packed_step_indices: true,
            ..Generation::default()
        },
        Generation {
            timestamp: Some((instant("2025-12-31T11:05:00Z").secs(), 0)),
            usage: Some(unkeyed_small),
            ..Generation::default()
        },
        Generation {
            model_id: Some(246),
            timestamp: Some((instant("2026-01-01T12:00:00Z").secs(), 0)),
            usage: Some(Usage {
                model_id: Some(246),
                input: 250_000,
                total_output: 1_000,
                visible_output: 800,
                cache_read: 100_000,
                response_id: Some("resp-alpha-3"),
                ..Usage::default()
            }),
            ..Generation::default()
        },
        Generation {
            timestamp: Some((instant("2026-01-01T13:00:00Z").secs(), 0)),
            usage: Some(Usage {
                response_id: Some("resp-alpha-empty"),
                ..Usage::default()
            }),
            ..Generation::default()
        },
        Generation {
            model: Some(SONNET),
            timestamp: Some((instant("2026-01-02T08:59:00Z").secs(), 0)),
            ..Generation::default()
        },
        Generation {
            timestamp: Some((instant("2026-01-02T09:00:00Z").secs(), 0)),
            usage: Some(Usage {
                input: 600,
                cache_creation: 200,
                cache_read: 3_000,
                reasoning: 50,
                visible_output: 100,
                response_id: Some("resp-alpha-4"),
                ..Usage::default()
            }),
            ..Generation::default()
        },
        Generation {
            model_id: Some(1071),
            timestamp: Some((instant("2026-01-02T10:00:00Z").secs(), 0)),
            usage: Some(Usage {
                model_id: Some(1071),
                input: 40,
                total_output: 8,
                visible_output: 8,
                provider: Some(API_PROVIDER_GOOGLE_GEMINI),
                response_id: Some("resp-alpha-5"),
                ..Usage::default()
            }),
            ..Generation::default()
        },
    ];

    create_database(
        path,
        instant("2026-01-02T12:00:00Z"),
        &generation_rows(&generations),
        Some(&steps),
        Some(&[trajectory_blob(Some((
            instant("2025-12-30T08:00:00Z").secs(),
            0,
        )))]),
    );
    let connection = sqlite::open(path).expect("failed to reopen Antigravity database");
    connection
        .execute(
            "CREATE TABLE unknown_events (idx INTEGER PRIMARY KEY, input_tokens INTEGER);
             INSERT INTO unknown_events (idx, input_tokens) VALUES (1, 9999);",
        )
        .expect("failed to add an unknown table");
    drop(connection);
    set_modified(path, instant("2026-01-02T12:00:00Z"));
}

fn seed_gamma(path: &Path) {
    let first = Usage {
        input: 900,
        total_output: 400,
        cache_creation: 200,
        cache_read: 7_000,
        visible_output: 400,
        response_id: Some("resp-gamma-1"),
        ..Usage::default()
    };
    let steps = [(
        0,
        Some(
            Step {
                model: Some(HAIKU),
                timestamp: Some((instant("2026-01-01T00:10:00Z").secs(), 0)),
                usage: Some(first),
                ..Step::default()
            }
            .blob(),
        ),
    )];
    let generations = [
        Generation {
            model: Some(HAIKU),
            usage: Some(first),
            ..Generation::default()
        },
        Generation {
            usage: Some(Usage {
                input: 120,
                total_output: 30,
                visible_output: 30,
                ..Usage::default()
            }),
            ..Generation::default()
        },
        Generation {
            model: Some("GPT 5"),
            timestamp: Some((instant("2026-01-01T00:20:00Z").secs(), 0)),
            usage: Some(Usage {
                input: 2_000,
                total_output: 500,
                visible_output: 500,
                cache_read: 10_000,
                response_id: Some("resp-gamma-2"),
                ..Usage::default()
            }),
            ..Generation::default()
        },
        Generation {
            timestamp: Some((instant("2025-12-30T18:00:00Z").secs(), 0)),
            retries: vec![Usage {
                input: 5,
                total_output: 1,
                visible_output: 1,
                response_id: Some("resp-gamma-retry"),
                ..Usage::default()
            }],
            ..Generation::default()
        },
    ];
    create_database(
        path,
        instant("2026-01-02T12:00:00Z"),
        &generation_rows(&generations),
        Some(&steps),
        Some(&[
            trajectory_blob(None),
            trajectory_blob(Some((instant("2025-12-31T23:50:00Z").secs(), 500_000_000))),
            trajectory_blob(Some((instant("2026-01-02T20:00:00Z").secs(), 0))),
        ]),
    );
}

fn seed_delta(path: &Path) {
    let generations = [
        Generation {
            usage: Some(Usage {
                input: 100,
                total_output: 50,
                visible_output: 50,
                ..Usage::default()
            }),
            ..Generation::default()
        },
        Generation {
            timestamp: Some((instant("2025-12-27T23:30:00Z").secs(), 0)),
            usage: Some(Usage {
                model_id: Some(77),
                input: 10,
                total_output: 5,
                visible_output: 5,
                response_id: Some("resp-delta-2"),
                ..Usage::default()
            }),
            ..Generation::default()
        },
    ];
    create_database(
        path,
        instant("2025-12-27T22:00:00Z"),
        &generation_rows(&generations),
        None,
        None,
    );
}

fn seed_ignored(ide: &Path, conversations: &Path) {
    let leak = |tokens: u64, response_id: &'static str| {
        Generation {
            model: Some(SONNET),
            timestamp: Some((instant("2025-12-31T12:00:00Z").secs(), 0)),
            usage: Some(Usage {
                input: tokens,
                total_output: tokens,
                visible_output: tokens,
                response_id: Some(response_id),
                ..Usage::default()
            }),
            ..Generation::default()
        }
        .blob()
    };
    let mtime = instant("2026-01-02T12:00:00Z");
    let write = |name: &str, contents: &str| {
        let path = conversations.join(name);
        std::fs::write(&path, contents).expect("failed to write Antigravity fixture file");
        set_modified(&path, mtime);
    };

    create_database(
        &conversations.join("malformed-blob.db"),
        mtime,
        &[
            (0, leak(9_999, "resp-malformed-valid-row")),
            (1, vec![0x0a, 0x01, 0x80]),
        ],
        Some(&[]),
        Some(&[]),
    );
    create_database(
        &ide.join("conversation_summaries.db"),
        mtime,
        &[(0, leak(8_888, "resp-outside-conversations"))],
        None,
        None,
    );
    create_database(
        &conversations.join("session-backup.db.bak"),
        mtime,
        &[(0, leak(6_666, "resp-backup"))],
        None,
        None,
    );

    let bad_steps = conversations.join("bad-steps.db");
    let connection = sqlite::open(&bad_steps).expect("failed to create Antigravity database");
    connection
        .execute(
            "CREATE TABLE gen_metadata (idx INTEGER PRIMARY KEY, data BLOB NOT NULL);
             CREATE TABLE steps (idx INTEGER PRIMARY KEY, wrong BLOB);",
        )
        .expect("failed to create Antigravity tables");
    insert_blob(
        &connection,
        "INSERT INTO gen_metadata (idx, data) VALUES (?1, ?2)",
        0,
        &leak(7_777, "resp-bad-steps"),
    );
    drop(connection);
    set_modified(&bad_steps, mtime);

    let missing_table = conversations.join("missing-table.db");
    let connection = sqlite::open(&missing_table).expect("failed to create Antigravity database");
    connection
        .execute(
            "CREATE TABLE usage_events (idx INTEGER PRIMARY KEY, input_tokens INTEGER);
             INSERT INTO usage_events (idx, input_tokens) VALUES (1, 5555);",
        )
        .expect("failed to create an unrelated table");
    drop(connection);
    set_modified(&missing_table, mtime);

    write("empty.db", "");
    write("not-a-database.db", "not sqlite");
    write("notes.txt", "not a conversation\n");
}

#[derive(Clone, Copy, Default)]
struct Usage {
    model_id: Option<u64>,
    input: u64,
    total_output: u64,
    cache_creation: u64,
    cache_read: u64,
    reasoning: u64,
    visible_output: u64,
    provider: Option<u64>,
    message_id: Option<&'static str>,
    response_id: Option<&'static str>,
    provider_message_id: Option<&'static str>,
}

impl Usage {
    fn blob(self) -> Vec<u8> {
        let mut out = Vec::new();
        if let Some(model_id) = self.model_id {
            put_varint(&mut out, 1, model_id);
        }
        for (field, value) in [
            (2, self.input),
            (3, self.total_output),
            (4, self.cache_creation),
            (5, self.cache_read),
            (9, self.reasoning),
            (10, self.visible_output),
        ] {
            if value > 0 {
                put_varint(&mut out, field, value);
            }
        }
        if let Some(provider) = self.provider {
            put_varint(&mut out, 6, provider);
        }
        for (field, text) in [
            (7, self.message_id),
            (11, self.response_id),
            (12, self.provider_message_id),
        ] {
            if let Some(text) = text {
                put_bytes(&mut out, field, text.as_bytes());
            }
        }
        out
    }

    fn retry_blob(self) -> Vec<u8> {
        let mut out = Vec::new();
        put_bytes(&mut out, 2, &self.blob());
        out
    }
}

#[derive(Default)]
struct Generation {
    model: Option<&'static str>,
    model_in_field_21: bool,
    model_id: Option<u64>,
    usage: Option<Usage>,
    retries: Vec<Usage>,
    timestamp: Option<(i64, u64)>,
    step_indices: Vec<u64>,
    packed_step_indices: bool,
}

impl Generation {
    fn blob(&self) -> Vec<u8> {
        let mut chat_model = Vec::new();
        if let Some(model_id) = self.model_id {
            put_varint(&mut chat_model, 3, model_id);
        }
        if let Some(usage) = self.usage {
            put_bytes(&mut chat_model, 4, &usage.blob());
        }
        if let Some(timestamp) = self.timestamp {
            let mut generation_info = Vec::new();
            put_bytes(&mut generation_info, 4, &timestamp_blob(timestamp));
            put_bytes(&mut chat_model, 9, &generation_info);
        }
        for retry in &self.retries {
            put_bytes(&mut chat_model, 17, &retry.retry_blob());
        }
        if let Some(model) = self.model {
            let field = if self.model_in_field_21 { 21 } else { 19 };
            put_bytes(&mut chat_model, field, model.as_bytes());
        }

        let mut out = Vec::new();
        put_bytes(&mut out, 1, &chat_model);
        if self.packed_step_indices {
            let mut packed = Vec::new();
            for index in &self.step_indices {
                put_raw_varint(&mut packed, *index);
            }
            put_bytes(&mut out, 2, &packed);
        } else {
            for index in &self.step_indices {
                put_varint(&mut out, 2, *index);
            }
        }
        put_unknown_fields(&mut out);
        out
    }
}

#[derive(Default)]
struct Step {
    model: Option<&'static str>,
    model_in_field_8: bool,
    provider: Option<u64>,
    usage: Option<Usage>,
    retries: Vec<Usage>,
    timestamp: Option<(i64, u64)>,
    timestamp_in_field_1: bool,
}

impl Step {
    fn blob(&self) -> Vec<u8> {
        let mut out = Vec::new();
        if let Some(timestamp) = self.timestamp {
            let field = if self.timestamp_in_field_1 { 1 } else { 8 };
            put_bytes(&mut out, field, &timestamp_blob(timestamp));
        }
        if let Some(usage) = self.usage {
            put_bytes(&mut out, 9, &usage.blob());
        }
        if self.model.is_some() || self.provider.is_some() {
            let mut model_info = Vec::new();
            if let Some(provider) = self.provider {
                put_varint(&mut model_info, 7, provider);
            }
            if let Some(model) = self.model {
                let field = if self.model_in_field_8 { 8 } else { 12 };
                put_bytes(&mut model_info, field, model.as_bytes());
            }
            put_bytes(&mut out, 24, &model_info);
        }
        for retry in &self.retries {
            put_bytes(&mut out, 28, &retry.retry_blob());
        }
        put_unknown_fields(&mut out);
        out
    }
}

fn generation_rows(generations: &[Generation]) -> Vec<(i64, Vec<u8>)> {
    (0_i64..)
        .zip(generations)
        .map(|(idx, generation)| (idx, generation.blob()))
        .collect()
}

fn timestamp_blob((seconds, nanos): (i64, u64)) -> Vec<u8> {
    let mut out = Vec::new();
    put_varint(
        &mut out,
        1,
        u64::try_from(seconds).expect("fixture seconds are non-negative"),
    );
    put_varint(&mut out, 2, nanos);
    out
}

fn trajectory_blob(timestamp: Option<(i64, u64)>) -> Vec<u8> {
    let mut out = Vec::new();
    if let Some(timestamp) = timestamp {
        put_bytes(&mut out, 2, &timestamp_blob(timestamp));
    }
    put_unknown_fields(&mut out);
    out
}

fn put_unknown_fields(out: &mut Vec<u8>) {
    put_raw_varint(out, (98 << 3) | 5);
    out.extend_from_slice(&9_999_u32.to_le_bytes());
    put_raw_varint(out, (99 << 3) | 1);
    out.extend_from_slice(&9_999_u64.to_le_bytes());
}

fn put_raw_varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push((value.to_le_bytes()[0] & 0x7f) | 0x80);
        value >>= 7;
    }
    out.push(value.to_le_bytes()[0]);
}

fn put_varint(out: &mut Vec<u8>, field: u64, value: u64) {
    put_raw_varint(out, field << 3);
    put_raw_varint(out, value);
}

fn put_bytes(out: &mut Vec<u8>, field: u64, value: &[u8]) {
    put_raw_varint(out, (field << 3) | 2);
    put_raw_varint(out, value.len() as u64);
    out.extend_from_slice(value);
}

type StepRow = (i64, Option<Vec<u8>>);

fn create_database(
    path: &Path,
    mtime: Instant,
    generations: &[(i64, Vec<u8>)],
    steps: Option<&[StepRow]>,
    trajectories: Option<&[Vec<u8>]>,
) {
    let connection = sqlite::open(path).expect("failed to create Antigravity database");
    connection
        .execute("CREATE TABLE gen_metadata (idx INTEGER PRIMARY KEY, data BLOB NOT NULL)")
        .expect("failed to create gen_metadata");
    for (idx, data) in generations {
        insert_blob(
            &connection,
            "INSERT INTO gen_metadata (idx, data) VALUES (?1, ?2)",
            *idx,
            data,
        );
    }
    if let Some(steps) = steps {
        connection
            .execute("CREATE TABLE steps (idx INTEGER PRIMARY KEY, metadata BLOB)")
            .expect("failed to create steps");
        for (idx, metadata) in steps {
            match metadata {
                Some(metadata) => insert_blob(
                    &connection,
                    "INSERT INTO steps (idx, metadata) VALUES (?1, ?2)",
                    *idx,
                    metadata,
                ),
                None => connection
                    .execute(format!(
                        "INSERT INTO steps (idx, metadata) VALUES ({idx}, NULL)"
                    ))
                    .expect("failed to insert a NULL step"),
            }
        }
    }
    if let Some(trajectories) = trajectories {
        connection
            .execute("CREATE TABLE trajectory_metadata_blob (data BLOB)")
            .expect("failed to create trajectory_metadata_blob");
        for (rowid, data) in (1_i64..).zip(trajectories) {
            insert_blob(
                &connection,
                "INSERT INTO trajectory_metadata_blob (rowid, data) VALUES (?1, ?2)",
                rowid,
                data,
            );
        }
    }
    drop(connection);
    set_modified(path, mtime);
}

fn insert_blob(connection: &sqlite::Connection, sql: &str, idx: i64, blob: &[u8]) {
    let mut statement = connection
        .prepare(sql)
        .expect("failed to prepare Antigravity insert");
    statement
        .bind((1, idx))
        .expect("failed to bind Antigravity row index");
    statement
        .bind((2, blob))
        .expect("failed to bind Antigravity blob");
    statement
        .next()
        .expect("failed to insert Antigravity fixture row");
}
