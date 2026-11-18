use std::io::{self, BufRead, Read, Write};

use ccusage_core::{PricingMap, Result, cli::CodexStatuslineArgs, cli_error};
use serde_json::{Value, json};

const RESOURCE_URI: &str = "ui://ccusage/codex-statusline";
const TOOL_NAME: &str = "ccusage_codex_statusline";
const MAX_MESSAGE_BYTES: u64 = 1024 * 1024;

pub(super) fn serve(args: CodexStatuslineArgs) -> Result<()> {
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    let mut line = Vec::new();
    let mut pricing = None;
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
        let reply = match serde_json::from_slice::<Value>(&line) {
            Ok(request) => dispatch(&request, &args, &mut pricing),
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
    pricing: &mut Option<PricingMap>,
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
                let pricing = pricing.get_or_insert_with(|| super::load_pricing(args));
                match super::summarize(args, pricing, Some(thread)) {
                    Ok(summary) => {
                        Value::String(super::format_summary(&summary, args.shared.no_cost))
                    }
                    Err(_) => return Some(error(id, -32603, "Unable to read Codex usage")),
                }
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
