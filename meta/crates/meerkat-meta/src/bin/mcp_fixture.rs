#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::uninlined_format_args,
    clippy::collapsible_if,
    clippy::redundant_clone,
    clippy::needless_raw_string_hashes,
    clippy::single_match,
    clippy::redundant_closure_for_method_calls,
    clippy::redundant_pattern_matching,
    clippy::ignored_unit_patterns,
    clippy::clone_on_copy,
    clippy::manual_assert,
    clippy::unwrap_in_result,
    clippy::useless_vec
)]
//! Phase C binary — MCP fixture server for integration testing.

use std::env;
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::time::Duration;

use serde_json::{Value, json};

const PROTOCOL_VERSION: &str = "2024-11-05";
const SERVER_VERSION: &str = "0.1.0";
const FAIL_TOOL_ENV: &str = "META_PHASE_C_FAIL_TOOL";
const LOG_PATH_ENV: &str = "META_PHASE_C_LOG_PATH";
const ROUTER_SINK_ENV: &str = "META_PHASE_C_ROUTER_SINK";
const ROUTER_TARGET_ENV: &str = "META_PHASE_C_ROUTER_TARGET";
const DELIVERY_ADAPTER_ENV: &str = "META_PHASE_C_DELIVERY_ADAPTER";
const DELIVERY_FORCE_FAIL_ENV: &str = "META_PHASE_C_DELIVERY_FORCE_FAIL";
const MEMORY_CONFLICT_KEY_ENV: &str = "META_PHASE_C_MEMORY_CONFLICT_KEY";
const MEMORY_CONFLICT_REASON_ENV: &str = "META_PHASE_C_MEMORY_CONFLICT_REASON";
const HANG_ON_ENV: &str = "META_PHASE_C_HANG_ON";
const HANG_ON_FILE_ENV: &str = "META_PHASE_C_HANG_ON_FILE";
const DELAY_ON_ENV: &str = "META_PHASE_C_DELAY_ON";
const DELAY_MS_ENV: &str = "META_PHASE_C_DELAY_MS";
const CLOSE_DELAY_MS_ENV: &str = "META_PHASE_C_CLOSE_DELAY_MS";
const DEFAULT_CLOSE_DELAY_MS: u64 = 2_000;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let module = parse_module_from_args().unwrap_or_else(|| "router".to_string());
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    let mut reader = BufReader::new(stdin.lock());

    loop {
        let mut line = String::new();
        let bytes_read = reader.read_line(&mut line)?;
        if bytes_read == 0 {
            if should_hang("close", None) {
                append_log(&format!("{module}:hang:close"));
                let delay_ms = env::var(CLOSE_DELAY_MS_ENV)
                    .ok()
                    .and_then(|raw| raw.trim().parse::<u64>().ok())
                    .filter(|delay_ms| *delay_ms > 0)
                    .unwrap_or(DEFAULT_CLOSE_DELAY_MS);
                std::thread::sleep(Duration::from_millis(delay_ms));
            }
            break;
        }

        let line = line.trim_end_matches(['\n', '\r']).to_string();
        if line.trim().is_empty() {
            continue;
        }

        let request: Value = match serde_json::from_str(&line) {
            Ok(request) => request,
            Err(error) => {
                let response = json!({
                    "jsonrpc": "2.0",
                    "id": Value::Null,
                    "error": {
                        "code": -32700,
                        "message": format!("parse error: {error}"),
                    }
                });
                writeln!(stdout, "{response}")?;
                stdout.flush()?;
                continue;
            }
        };

        // Notifications are acknowledged silently per MCP behavior.
        if request.get("id").is_none() {
            continue;
        }

        let response = handle_request(&module, &request);
        writeln!(stdout, "{response}")?;
        stdout.flush()?;
    }

    Ok(())
}

fn parse_module_from_args() -> Option<String> {
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--module" {
            return args.next();
        }
    }
    None
}

fn handle_request(module: &str, request: &Value) -> Value {
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let method = request
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();

    match method {
        "initialize" => {
            if should_hang("initialize", None) {
                hang_forever(module, "initialize", None);
            }
            maybe_delay(module, "initialize", None);
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": {
                        "tools": {}
                    },
                    "serverInfo": {
                        "name": format!("phase-c-mcp-fixture-{module}"),
                        "version": SERVER_VERSION,
                    }
                }
            })
        }
        "ping" => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {}
        }),
        "tools/list" => {
            if should_hang("list_tools", None) {
                hang_forever(module, "list_tools", None);
            }
            maybe_delay(module, "list_tools", None);
            append_log(&format!("{module}:list_tools"));
            let tools = tool_descriptors(module)
                .into_iter()
                .map(|(name, description)| {
                    json!({
                        "name": name,
                        "description": description,
                        "inputSchema": {
                            "type": "object",
                            "properties": {},
                            "required": [],
                        }
                    })
                })
                .collect::<Vec<_>>();
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "tools": tools
                }
            })
        }
        "tools/call" => handle_tool_call(module, id, request),
        _ => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {
                "code": -32601,
                "message": format!("method not found: {method}")
            }
        }),
    }
}

fn handle_tool_call(module: &str, id: Value, request: &Value) -> Value {
    let params = request.get("params").cloned().unwrap_or_else(|| json!({}));
    let tool_name = params
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let args = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    if should_hang("call_tool", Some(tool_name)) {
        hang_forever(module, "call_tool", Some(tool_name));
    }
    maybe_delay(module, "call_tool", Some(tool_name));
    append_log(&format!("{module}:call:{tool_name}:{args}"));

    if env::var(FAIL_TOOL_ENV).ok().as_deref() == Some(tool_name) {
        return json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "content": [{
                    "type": "text",
                    "text": format!("forced failure for {tool_name}"),
                }],
                "isError": true,
            }
        });
    }

    let payload = match (module, tool_name) {
        ("router", "routing.resolve") => {
            let sink = env::var(ROUTER_SINK_ENV).unwrap_or_else(|_| "email".to_string());
            let target_module =
                env::var(ROUTER_TARGET_ENV).unwrap_or_else(|_| "delivery".to_string());
            json!({
                "sink": sink,
                "target_module": target_module,
            })
        }
        ("delivery", "delivery.send") => {
            let adapter =
                env::var(DELIVERY_ADAPTER_ENV).unwrap_or_else(|_| "smtp-mock".to_string());
            let force_fail = env::var(DELIVERY_FORCE_FAIL_ENV).ok().as_deref() == Some("1")
                || args
                    .get("payload")
                    .and_then(Value::as_object)
                    .and_then(|payload| payload.get("force_adapter_fail"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
            json!({
                "adapter": adapter,
                "force_fail": force_fail,
            })
        }
        ("memory", "memory.conflict_read") => {
            let entity = args
                .get("entity")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase();
            let topic = args
                .get("topic")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase();
            let requested_key = format!("{entity}:{topic}");
            let configured_key = env::var(MEMORY_CONFLICT_KEY_ENV).ok();
            let conflict_active = configured_key
                .as_deref()
                .is_some_and(|candidate| candidate.eq_ignore_ascii_case(&requested_key));
            if conflict_active {
                let reason = env::var(MEMORY_CONFLICT_REASON_ENV)
                    .unwrap_or_else(|_| "fixture_memory_conflict".to_string());
                json!({
                    "conflict": {
                        "entity": entity,
                        "topic": topic,
                        "store": "knowledge_graph",
                        "reason": reason,
                        "updated_at_ms": 42,
                    }
                })
            } else {
                json!({
                    "conflict": Value::Null
                })
            }
        }
        _ => {
            return json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {
                    "code": -32601,
                    "message": format!("unknown tool: {tool_name}")
                }
            });
        }
    };

    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "content": [{
                "type": "text",
                "text": payload.to_string()
            }]
        }
    })
}

fn tool_descriptors(module: &str) -> Vec<(&'static str, &'static str)> {
    match module {
        "router" => vec![("routing.resolve", "Resolve routing overrides")],
        "delivery" => vec![("delivery.send", "Dispatch delivery metadata")],
        "memory" => vec![("memory.conflict_read", "Read memory conflict signals")],
        _ => Vec::new(),
    }
}

fn append_log(line: &str) {
    let Some(path) = env::var(LOG_PATH_ENV).ok() else {
        return;
    };
    let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) else {
        return;
    };
    let _ = writeln!(file, "{line}");
}

fn should_hang(operation: &str, tool_name: Option<&str>) -> bool {
    let raw = env::var(HANG_ON_ENV)
        .ok()
        .and_then(|raw| {
            let trimmed = raw.trim();
            if trimmed.is_empty() { None } else { Some(raw) }
        })
        .or_else(|| {
            let path = env::var(HANG_ON_FILE_ENV).ok()?;
            let raw = fs::read_to_string(path).ok()?;
            let trimmed = raw.trim();
            if trimmed.is_empty() { None } else { Some(raw) }
        });
    let Some(raw) = raw else {
        return false;
    };
    raw.split(',').any(|candidate| {
        let candidate = candidate.trim();
        if candidate.is_empty() {
            return false;
        }
        if candidate.eq_ignore_ascii_case("all") || candidate.eq_ignore_ascii_case(operation) {
            return true;
        }
        if operation != "call_tool" {
            return false;
        }
        let Some(tool_name) = tool_name else {
            return false;
        };
        candidate.eq_ignore_ascii_case(&format!("call_tool:{tool_name}"))
            || candidate.eq_ignore_ascii_case(&format!("call:{tool_name}"))
    })
}

fn hang_forever(module: &str, operation: &str, tool_name: Option<&str>) -> ! {
    if let Some(tool_name) = tool_name {
        append_log(&format!("{module}:hang:{operation}:{tool_name}"));
    } else {
        append_log(&format!("{module}:hang:{operation}"));
    }
    loop {
        std::thread::sleep(Duration::from_mins(1));
    }
}

fn maybe_delay(module: &str, operation: &str, tool_name: Option<&str>) {
    if !matches_delay_target(operation, tool_name) {
        return;
    }
    let delay_ms = env::var(DELAY_MS_ENV)
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .filter(|delay_ms| *delay_ms > 0)
        .unwrap_or(0);
    if delay_ms == 0 {
        return;
    }
    if let Some(tool_name) = tool_name {
        append_log(&format!(
            "{module}:delay:{operation}:{tool_name}:{delay_ms}"
        ));
    } else {
        append_log(&format!("{module}:delay:{operation}:{delay_ms}"));
    }
    std::thread::sleep(Duration::from_millis(delay_ms));
}

fn matches_delay_target(operation: &str, tool_name: Option<&str>) -> bool {
    let Some(raw) = env::var(DELAY_ON_ENV).ok() else {
        return false;
    };
    raw.split(',').any(|candidate| {
        let candidate = candidate.trim();
        if candidate.is_empty() {
            return false;
        }
        if candidate.eq_ignore_ascii_case("all") || candidate.eq_ignore_ascii_case(operation) {
            return true;
        }
        if operation != "call_tool" {
            return false;
        }
        let Some(tool_name) = tool_name else {
            return false;
        };
        candidate.eq_ignore_ascii_case(&format!("call_tool:{tool_name}"))
            || candidate.eq_ignore_ascii_case(&format!("call:{tool_name}"))
    })
}
