use std::path::{Path, PathBuf};

use crate::golden::Case;

const DEC27: u64 = 1_766_793_600;
const DEC30: u64 = 1_767_052_800;
const DEC31: u64 = 1_767_139_200;
const JAN01: u64 = 1_767_225_600;
const JAN02: u64 = 1_767_312_000;

const fn at(day: u64, hour: u64, minute: u64) -> u64 {
    day + hour * 3_600 + minute * 60
}

const API_PROVIDER_GOOGLE_GEMINI: u64 = 24;

const SONNET: &str = "claude-sonnet-4-20250514";
const HAIKU: &str = "claude-haiku-4-5-20251001";
const UNPRICED: &str = "antigravity-mystery-1";

pub fn seed(root: &Path) -> Vec<(&'static str, PathBuf)> {
    let ide = root.join("antigravity/ide");
    let conversations = ide.join("conversations");
    // A root without `conversations/` is itself the conversation directory.
    let cli = root.join("antigravity/cli-conversations");
    std::fs::create_dir_all(conversations.join("archive"))
        .expect("failed to create Antigravity conversations directory");
    std::fs::create_dir_all(&cli).expect("failed to create Antigravity CLI directory");

    seed_beta(&cli.join("session-beta.db"));
    seed_alpha(&conversations.join("session-alpha.db"));
    seed_gamma(&conversations.join("session-gamma.db"));
    seed_delta(&conversations.join("archive/session-delta.db"));
    seed_ignored(&ide, &conversations);

    // The paths sort `cli-conversations` before `ide`, so session-beta is
    // loaded first and is the merge target for cross-database duplicates.
    let data_dirs = format!("{},{}", ide.display(), cli.display());
    vec![("ANTIGRAVITY_DATA_DIR", PathBuf::from(data_dirs))]
}

pub fn extra_cases(_root: &Path) -> Vec<Case> {
    vec![Case::new(
        "session",
        "json_debug",
        &["antigravity", "session", "--offline", "--json", "--debug"],
    )]
}

/// Counted: 2,594 tokens in five events.
fn seed_beta(path: &Path) {
    let generations = [
        // Replays `resp-alpha-2` from session-alpha with smaller counts: one
        // event with the per-field maximum (620), kept in this session and
        // under this model, at session-alpha's earlier timestamp.
        Generation {
            model: Some(SONNET),
            timestamp: Some((at(DEC31, 16, 0), 0)),
            usage: Some(Usage {
                input: 400,
                total_output: 90,
                visible_output: 90,
                response_id: Some("resp-alpha-2"),
                ..Usage::default()
            }),
            ..Generation::default()
        },
        // Near miss: the same counts under another response ID are counted.
        Generation {
            timestamp: Some((at(JAN01, 1, 0), 0)),
            usage: Some(Usage {
                input: 400,
                total_output: 90,
                visible_output: 90,
                response_id: Some("resp-beta-near-miss"),
                ..Usage::default()
            }),
            ..Generation::default()
        },
        // Three rows that share no single identity: the third carries both IDs
        // and folds the first two into one event (30 in, 9 out).
        Generation {
            model: Some("GPT 5"),
            timestamp: Some((at(JAN01, 2, 0), 0)),
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
            timestamp: Some((at(JAN01, 2, 5), 0)),
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
            timestamp: Some((at(JAN01, 2, 10), 0)),
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
        // Unmapped model ID with no model name: stays a placeholder.
        Generation {
            timestamp: Some((at(JAN02, 8, 0), 0)),
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
        // Unmapped model ID with a model name on the same row: the name wins.
        Generation {
            model: Some(UNPRICED),
            model_id: Some(77),
            timestamp: Some((at(JAN01, 12, 30), 0)),
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
        at(JAN02, 12, 0),
        &generation_rows(&generations),
        Some(&[]),
        Some(&[]),
    );
}

/// Counted: 364,451 tokens in nine events (the `resp-alpha-2` primary is
/// counted under session-beta).
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
        // Step copy of the first generation with a larger cache read and an
        // earlier timestamp; the merge keeps both.
        (
            1,
            Some(
                Step {
                    model: Some("gemini-3-pro-high"),
                    model_in_field_8: true,
                    provider: Some(API_PROVIDER_GOOGLE_GEMINI),
                    timestamp: Some((at(DEC30, 9, 59) + 59, 0)),
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
                    timestamp: Some((at(DEC31, 10, 0), 0)),
                    timestamp_in_field_1: true,
                    usage: Some(unkeyed),
                    ..Step::default()
                }
                .blob(),
            ),
        ),
        // Retry-only step without model info: takes the database's last
        // generation model.
        (
            4,
            Some(
                Step {
                    timestamp: Some((at(JAN01, 9, 0), 0)),
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
                    timestamp: Some((at(JAN01, 9, 30), 0)),
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
            timestamp: Some((at(DEC30, 10, 0), 250_000_000)),
            usage: Some(first),
            ..Generation::default()
        },
        // Continuation row after 15:00Z. The retry list repeats the primary
        // (dropped) and holds one failed attempt under its own ID (counted).
        Generation {
            timestamp: Some((at(DEC31, 15, 30), 0)),
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
        // Unkeyed mirror of step 3, linked through its step index.
        Generation {
            model: Some("gemini-3-flash-agent"),
            model_in_field_21: true,
            timestamp: Some((at(DEC31, 10, 0) + 5, 0)),
            usage: Some(unkeyed),
            retries: vec![unkeyed],
            step_indices: vec![3],
            ..Generation::default()
        },
        // Near misses: unkeyed usage linked to steps that do not exist, and
        // the same counts again with no step link at all.
        Generation {
            timestamp: Some((at(DEC31, 11, 0), 0)),
            usage: Some(unkeyed_small),
            step_indices: vec![7, 8],
            packed_step_indices: true,
            ..Generation::default()
        },
        Generation {
            timestamp: Some((at(DEC31, 11, 5), 0)),
            usage: Some(unkeyed_small),
            ..Generation::default()
        },
        // Model known only by ID, with more than 200k input tokens.
        Generation {
            model_id: Some(246),
            timestamp: Some((at(JAN01, 12, 0), 0)),
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
            timestamp: Some((at(JAN01, 13, 0), 0)),
            usage: Some(Usage {
                response_id: Some("resp-alpha-empty"),
                ..Usage::default()
            }),
            ..Generation::default()
        },
        // A row without usage still switches the model for later rows.
        Generation {
            model: Some(SONNET),
            timestamp: Some((at(JAN02, 8, 59), 0)),
            ..Generation::default()
        },
        Generation {
            timestamp: Some((at(JAN02, 9, 0), 0)),
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
            timestamp: Some((at(JAN02, 10, 0), 0)),
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
        at(JAN02, 12, 0),
        &generation_rows(&generations),
        Some(&steps),
        Some(&[trajectory_blob(Some((at(DEC30, 8, 0), 0)))]),
    );
    let connection = sqlite::open(path).expect("failed to reopen Antigravity database");
    connection
        .execute(
            "CREATE TABLE unknown_events (idx INTEGER PRIMARY KEY, input_tokens INTEGER);
             INSERT INTO unknown_events (idx, input_tokens) VALUES (1, 9999);",
        )
        .expect("failed to add an unknown table");
    drop(connection);
    crate::golden::set_modified(path, at(JAN02, 12, 0));
}

/// Counted: 21,156 tokens in four events.
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
                timestamp: Some((at(JAN01, 0, 10), 0)),
                usage: Some(first),
                ..Step::default()
            }
            .blob(),
        ),
    )];
    let generations = [
        // No timestamp: inherits the step copy's through the shared identity.
        Generation {
            model: Some(HAIKU),
            usage: Some(first),
            ..Generation::default()
        },
        // No timestamp and no identity: falls back to the trajectory timestamp.
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
            timestamp: Some((at(JAN01, 0, 20), 0)),
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
            timestamp: Some((at(DEC30, 18, 0), 0)),
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
        at(JAN02, 12, 0),
        &generation_rows(&generations),
        Some(&steps),
        // The first row that carries a timestamp wins.
        Some(&[
            trajectory_blob(None),
            trajectory_blob(Some((at(DEC31, 23, 50), 500_000_000))),
            trajectory_blob(Some((at(JAN02, 20, 0), 0))),
        ]),
    );
}

/// Counted: 165 tokens in two events. Only `gen_metadata` exists, and no row
/// names a model.
fn seed_delta(path: &Path) {
    let generations = [
        // No timestamp anywhere: falls back to the file mtime.
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
            timestamp: Some((at(DEC27, 23, 30), 0)),
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
        at(DEC27, 22, 0),
        &generation_rows(&generations),
        None,
        None,
    );
}

/// Files that must contribute nothing. Their token counts are 9999, 8888,
/// 7777, 6666, and 5555 so a leak is visible.
fn seed_ignored(ide: &Path, conversations: &Path) {
    let leak = |tokens: u64, response_id: &'static str| {
        Generation {
            model: Some(SONNET),
            timestamp: Some((at(DEC31, 12, 0), 0)),
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
    let mtime = at(JAN02, 12, 0);
    let write = |name: &str, contents: &str| {
        let path = conversations.join(name);
        std::fs::write(&path, contents).expect("failed to write Antigravity fixture file");
        crate::golden::set_modified(&path, mtime);
    };

    // One undecodable row discards the whole database, valid rows included.
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
    // With `conversations/` present, databases beside it are not scanned.
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
    crate::golden::set_modified(&bad_steps, mtime);

    let missing_table = conversations.join("missing-table.db");
    let connection = sqlite::open(&missing_table).expect("failed to create Antigravity database");
    connection
        .execute(
            "CREATE TABLE usage_events (idx INTEGER PRIMARY KEY, input_tokens INTEGER);
             INSERT INTO usage_events (idx, input_tokens) VALUES (1, 5555);",
        )
        .expect("failed to create an unrelated table");
    drop(connection);
    crate::golden::set_modified(&missing_table, mtime);

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

/// One `gen_metadata.data` blob (GeneratorMetadata).
#[derive(Default)]
struct Generation {
    model: Option<&'static str>,
    /// Writes the model name to field 21 instead of field 19.
    model_in_field_21: bool,
    model_id: Option<u64>,
    usage: Option<Usage>,
    retries: Vec<Usage>,
    timestamp: Option<(u64, u64)>,
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

/// One `steps.metadata` blob (CortexStepMetadata).
#[derive(Default)]
struct Step {
    model: Option<&'static str>,
    /// Writes the model name to field 8 of the model info instead of field 12.
    model_in_field_8: bool,
    provider: Option<u64>,
    usage: Option<Usage>,
    retries: Vec<Usage>,
    timestamp: Option<(u64, u64)>,
    /// Writes the timestamp to field 1 instead of field 8.
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

fn timestamp_blob((seconds, nanos): (u64, u64)) -> Vec<u8> {
    let mut out = Vec::new();
    put_varint(&mut out, 1, seconds);
    put_varint(&mut out, 2, nanos);
    out
}

fn trajectory_blob(timestamp: Option<(u64, u64)>) -> Vec<u8> {
    let mut out = Vec::new();
    if let Some(timestamp) = timestamp {
        put_bytes(&mut out, 2, &timestamp_blob(timestamp));
    }
    put_unknown_fields(&mut out);
    out
}

/// Fields the adapter does not know, in the two fixed-width wire types the
/// known fields never use.
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

/// A `steps` row; `None` stores a NULL metadata blob.
type StepRow = (i64, Option<Vec<u8>>);

/// Creates a conversation database. `None` for `steps` or `trajectories`
/// leaves that table out entirely.
fn create_database(
    path: &Path,
    mtime: u64,
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
    crate::golden::set_modified(path, mtime);
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
