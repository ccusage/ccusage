use std::io::{self, BufRead, Read, Write};

use ccusage_core::{Result, cli::CodexStatuslineArgs, cli_error, sanitize_terminal_text};
use serde_json::{Value, json};

const RESOURCE_URI: &str = "ui://ccusage/codex-statusline";
const TOOL_NAME: &str = "ccusage_codex_statusline";
const MAX_MESSAGE_BYTES: u64 = 1024 * 1024;

pub(super) fn serve(args: CodexStatuslineArgs) -> Result<()> {
    let mut input = io::stdin().lock();
    let mut output = ccusage_core::stdout();
    let mut line = Vec::new();
    let mut reader = None;
    loop {
        line.clear();
        if input
            .by_ref()
            .take(MAX_MESSAGE_BYTES + 1)
            .read_until(b'\n', &mut line)?
            == 0
        {
            return Ok(());
        }
        if line.len() as u64 > MAX_MESSAGE_BYTES {
            return Err(cli_error("MCP message exceeds 1 MiB"));
        }
        let reply = match ccusage_adapter_common::json::parse::<Value>(&line) {
            Ok(request) => dispatch(&request, &args, &mut reader),
            Err(_) => Some(error(Value::Null, -32700, "Parse error")),
        };
        if let Some(reply) = reply {
            serde_json::to_writer(&mut output, &reply)?;
            writeln!(output)?;
            output.flush()?;
        }
    }
}

fn dispatch(
    request: &Value,
    args: &CodexStatuslineArgs,
    reader: &mut Option<super::NativeUsageReader>,
) -> Option<Value> {
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let Some(method) = request.get("method").and_then(Value::as_str) else {
        return Some(error(id, -32600, "Invalid request"));
    };
    if request["jsonrpc"] != "2.0" || !(id.is_null() || id.is_string() || id.is_number()) {
        return Some(error(Value::Null, -32600, "Invalid request"));
    }
    request.get("id")?;
    let result = match method {
        "initialize" => json!({
            "protocolVersion":"2025-06-18",
            "capabilities":{"tools":{},"resources":{}},
            "serverInfo":{"name":"ccusage-codex-statusline","version":env!("CARGO_PKG_VERSION")}
        }),
        "ping" => json!({}),
        "tools/list" => json!({"tools":[{
            "name":TOOL_NAME,
            "title":"ccusage Codex usage",
            "description":"Summarize local Codex token usage and estimated cost.",
            "inputSchema":{"type":"object","properties":{},"additionalProperties":false},
            "annotations":{"readOnlyHint":true,"destructiveHint":false,"openWorldHint":false},
            "_meta":{"ui":{"visibility":["app"]},"openai/ui":{"entrypoints":[{"type":"statusline","resourceUri":RESOURCE_URI}]}}
        }]}),
        "resources/list" => json!({"resources":[{
            "uri":RESOURCE_URI,"name":"Codex usage statusline","mimeType":"application/json"
        }]}),
        "resources/templates/list" => json!({"resourceTemplates":[]}),
        "resources/read" => {
            if request["params"]["uri"] != RESOURCE_URI {
                return Some(error(id, -32602, "Unknown resource URI"));
            }
            let thread =
                &request["params"]["_meta"]["com.openai/codex-statusline-state"]["thread"]["id"];
            let tree = if thread.is_null() {
                Value::Null
            } else if let Some(thread) =
                thread.as_str().filter(|id| super::super::is_codex_uuid(id))
            {
                let reader =
                    reader.get_or_insert_with(|| super::NativeUsageReader::new(args.clone()));
                Value::String(with_model_label(
                    reader.read(thread),
                    &request["params"]["_meta"]["com.openai/codex-statusline-state"]["thread"]["model"],
                ))
            } else {
                return Some(error(id, -32602, "Invalid Codex thread ID"));
            };
            json!({"contents":[{"uri":RESOURCE_URI,"mimeType":"application/json","text":tree.to_string()}]})
        }
        "tools/call" => {
            if request["params"]["name"] != TOOL_NAME {
                return Some(error(id, -32602, "Unknown tool name"));
            }
            json!({"content":[{"type":"text","text":"Select ccusage Codex usage in Codex's /statusline picker."}]})
        }
        _ => return Some(error(id, -32601, "Method not found")),
    };
    Some(json!({"jsonrpc":"2.0","id":id,"result":result}))
}

fn error(id: Value, code: i32, message: &str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})
}

fn with_model_label(line: String, model: &Value) -> String {
    if matches!(
        line.as_str(),
        "Codex | Loading usage..." | "Codex | Usage unavailable"
    ) {
        return line;
    }
    let Some(display_name) = model["display_name"]
        .as_str()
        .map(sanitize_terminal_text)
        .filter(|name| !name.trim().is_empty())
    else {
        return line;
    };
    let reasoning = model["reasoning_level"]
        .as_str()
        .map(sanitize_terminal_text)
        .filter(|effort| !effort.trim().is_empty());
    let label = match reasoning {
        Some(reasoning) => format!("🤖 {display_name} ({reasoning})"),
        None => format!("🤖 {display_name}"),
    };
    if let Some(summary) = line.strip_prefix("Codex") {
        format!("{label}{summary}")
    } else {
        line
    }
}
