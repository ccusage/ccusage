use std::path::Path;

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct UsageFixture {
    pub(crate) model_id: Option<u64>,
    pub(crate) input_tokens: u64,
    pub(crate) total_output_tokens: u64,
    pub(crate) cache_creation_tokens: u64,
    pub(crate) cache_read_tokens: u64,
    pub(crate) reasoning_tokens: u64,
    pub(crate) visible_output_tokens: u64,
    pub(crate) provider: Option<u64>,
    pub(crate) message_id: Option<&'static str>,
    pub(crate) response_id: Option<&'static str>,
    pub(crate) provider_assigned_message_id: Option<&'static str>,
}

fn encode_varint(mut value: u64, output: &mut Vec<u8>) {
    while value >= 0x80 {
        output.push((value as u8 & 0x7f) | 0x80);
        value >>= 7;
    }
    output.push(value as u8);
}

fn field_varint(number: u64, value: u64, output: &mut Vec<u8>) {
    encode_varint(number << 3, output);
    encode_varint(value, output);
}

fn field_bytes(number: u64, value: &[u8], output: &mut Vec<u8>) {
    encode_varint((number << 3) | 2, output);
    encode_varint(value.len() as u64, output);
    output.extend_from_slice(value);
}

fn model_usage_blob(usage: UsageFixture) -> Vec<u8> {
    let mut output = Vec::new();
    if let Some(model_id) = usage.model_id {
        field_varint(1, model_id, &mut output);
    }
    for (field, value) in [
        (2, usage.input_tokens),
        (3, usage.total_output_tokens),
        (4, usage.cache_creation_tokens),
        (5, usage.cache_read_tokens),
        (9, usage.reasoning_tokens),
        (10, usage.visible_output_tokens),
    ] {
        if value > 0 {
            field_varint(field, value, &mut output);
        }
    }
    if let Some(provider) = usage.provider {
        field_varint(6, provider, &mut output);
    }
    if let Some(message_id) = usage.message_id {
        field_bytes(7, message_id.as_bytes(), &mut output);
    }
    if let Some(response_id) = usage.response_id {
        field_bytes(11, response_id.as_bytes(), &mut output);
    }
    if let Some(provider_assigned_message_id) = usage.provider_assigned_message_id {
        field_bytes(12, provider_assigned_message_id.as_bytes(), &mut output);
    }
    output
}

fn retry_info_blob(usage: UsageFixture) -> Vec<u8> {
    let usage_blob = model_usage_blob(usage);
    let mut retry_info = Vec::new();
    field_bytes(2, &usage_blob, &mut retry_info);
    retry_info
}

fn timestamp_blob(seconds: u64, nanos: u64) -> Vec<u8> {
    let mut timestamp = Vec::new();
    field_varint(1, seconds, &mut timestamp);
    field_varint(2, nanos, &mut timestamp);
    timestamp
}

pub(crate) fn metadata_blob(
    model: Option<&str>,
    usage: Option<UsageFixture>,
    timestamp: Option<(u64, u64)>,
    retries: &[UsageFixture],
) -> Vec<u8> {
    metadata_blob_with_steps(model, usage, timestamp, retries, &[])
}

pub(crate) fn metadata_blob_with_steps(
    model: Option<&str>,
    usage: Option<UsageFixture>,
    timestamp: Option<(u64, u64)>,
    retries: &[UsageFixture],
    step_indices: &[i64],
) -> Vec<u8> {
    let mut chat_model = Vec::new();
    if let Some(usage) = usage {
        if let Some(model_id) = usage.model_id {
            field_varint(3, model_id, &mut chat_model);
        }
        let usage_blob = model_usage_blob(usage);
        field_bytes(4, &usage_blob, &mut chat_model);
    }
    if let Some((seconds, nanos)) = timestamp {
        let timestamp_message = timestamp_blob(seconds, nanos);
        let mut generation_info = Vec::new();
        field_bytes(4, &timestamp_message, &mut generation_info);
        field_bytes(9, &generation_info, &mut chat_model);
    }
    for retry in retries {
        let retry_info = retry_info_blob(*retry);
        field_bytes(17, &retry_info, &mut chat_model);
    }
    if let Some(model) = model {
        field_bytes(19, model.as_bytes(), &mut chat_model);
    }

    let mut metadata = Vec::new();
    field_bytes(1, &chat_model, &mut metadata);
    for step_index in step_indices {
        let value = u64::try_from(*step_index).unwrap_or(0);
        field_varint(2, value, &mut metadata);
    }
    metadata
}

pub(crate) fn step_metadata_blob(
    model: Option<&str>,
    usage: Option<UsageFixture>,
    timestamp: Option<(u64, u64)>,
    retries: &[UsageFixture],
    provider: Option<u64>,
) -> Vec<u8> {
    let mut metadata = Vec::new();
    if let Some((seconds, nanos)) = timestamp {
        let timestamp_message = timestamp_blob(seconds, nanos);
        field_bytes(8, &timestamp_message, &mut metadata);
    }
    if let Some(usage) = usage {
        let usage_blob = model_usage_blob(usage);
        field_bytes(9, &usage_blob, &mut metadata);
    }
    if model.is_some() || provider.is_some() {
        let mut model_info = Vec::new();
        if let Some(usage) = usage
            && let Some(model_id) = usage.model_id
        {
            field_varint(1, model_id, &mut model_info);
        }
        if let Some(provider) = provider {
            field_varint(7, provider, &mut model_info);
        }
        if let Some(model) = model {
            field_bytes(12, model.as_bytes(), &mut model_info);
        }
        field_bytes(24, &model_info, &mut metadata);
    }
    for retry in retries {
        let retry_info = retry_info_blob(*retry);
        field_bytes(28, &retry_info, &mut metadata);
    }
    metadata
}

pub(crate) fn trajectory_metadata_blob(seconds: u64, nanos: u64) -> Vec<u8> {
    let timestamp = timestamp_blob(seconds, nanos);
    let mut metadata = Vec::new();
    field_bytes(2, &timestamp, &mut metadata);
    metadata
}

pub(crate) fn create_database(
    path: &Path,
    generation_rows: &[(i64, Vec<u8>)],
    step_rows: &[(i64, Vec<u8>)],
    trajectory_rows: &[Vec<u8>],
) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    let connection = sqlite::open(path).unwrap();
    connection
        .execute("CREATE TABLE gen_metadata (idx INTEGER PRIMARY KEY, data BLOB NOT NULL);")
        .unwrap();
    connection
        .execute("CREATE TABLE steps (idx INTEGER PRIMARY KEY, metadata BLOB);")
        .unwrap();
    connection
        .execute("CREATE TABLE trajectory_metadata_blob (data BLOB);")
        .unwrap();
    let mut generation_statement = connection
        .prepare("INSERT INTO gen_metadata (idx, data) VALUES (?1, ?2)")
        .unwrap();
    for (idx, data) in generation_rows {
        generation_statement.bind((1, *idx)).unwrap();
        generation_statement.bind((2, data.as_slice())).unwrap();
        generation_statement.next().unwrap();
        generation_statement.reset().unwrap();
    }
    let mut step_statement = connection
        .prepare("INSERT INTO steps (idx, metadata) VALUES (?1, ?2)")
        .unwrap();
    for (idx, data) in step_rows {
        step_statement.bind((1, *idx)).unwrap();
        step_statement.bind((2, data.as_slice())).unwrap();
        step_statement.next().unwrap();
        step_statement.reset().unwrap();
    }
    let mut trajectory_statement = connection
        .prepare("INSERT INTO trajectory_metadata_blob (data) VALUES (?1)")
        .unwrap();
    for data in trajectory_rows {
        trajectory_statement.bind((1, data.as_slice())).unwrap();
        trajectory_statement.next().unwrap();
        trajectory_statement.reset().unwrap();
    }
}
