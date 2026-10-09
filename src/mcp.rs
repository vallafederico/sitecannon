//! Minimal MCP server over stdio (JSON-RPC 2.0, one message per line). Exposes the tool
//! registry so agents (Claude Code, Claude Desktop, any MCP client) can drive SiteCannon.
//! Stdout carries protocol messages only; logs and progress go to stderr.

use std::io::{BufRead, Write};

use anyhow::Result;
use serde_json::{Value, json};

use crate::tools;

const PROTOCOL_VERSION: &str = "2025-06-18";

const INSTRUCTIONS: &str = "SiteCannon analyses websites before a rebuild. Typical flow: list_projects → (crawl if the \
site isn't there yet) → summary → drill down with sitemap, templates, topics, issues, page and query → report \
(optionally with a markdown brief in `notes`). Always mention coverage caveats from `summary` (capped crawls, refused \
URLs). Only crawl sites the user asked about, and keep the default politeness settings.";

fn response(id: &Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn error(id: &Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

fn handle(msg: &Value) -> Option<Value> {
    let id = msg.get("id")?.clone(); // notifications carry no id and get no reply
    let method = msg.get("method").and_then(Value::as_str).unwrap_or_default();
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    Some(match method {
        "initialize" => {
            let version = params
                .get("protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or(PROTOCOL_VERSION);
            response(
                &id,
                json!({
                    "protocolVersion": version,
                    "capabilities": {"tools": {"listChanged": false}},
                    "serverInfo": {"name": "sitecannon", "version": env!("CARGO_PKG_VERSION")},
                    "instructions": INSTRUCTIONS,
                }),
            )
        }
        "ping" => response(&id, json!({})),
        "tools/list" => response(&id, json!({"tools": tools::list()})),
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or_default();
            let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
            eprintln!("sitecannon mcp: {name} {args}");
            match tools::call(name, &args) {
                Ok(v) => response(
                    &id,
                    json!({
                        "content": [{"type": "text", "text": serde_json::to_string_pretty(&v).unwrap_or_default()}],
                        "structuredContent": if v.is_object() { v } else { json!({"result": v}) },
                        "isError": false,
                    }),
                ),
                // Tool failures are results the model should see, not protocol errors.
                Err(e) => response(
                    &id,
                    json!({"content": [{"type": "text", "text": format!("Error: {e:#}")}], "isError": true}),
                ),
            }
        }
        _ => error(&id, -32601, &format!("method not found: {method}")),
    })
}

/// Serves until stdin closes. Must run on a blocking thread (tools may block on async work).
pub fn serve() -> Result<()> {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(Value::Array(batch)) => {
                let replies: Vec<Value> = batch.iter().filter_map(handle).collect();
                (!replies.is_empty()).then_some(Value::Array(replies))
            }
            Ok(msg) => handle(&msg),
            Err(e) => Some(error(&Value::Null, -32700, &format!("parse error: {e}"))),
        };
        if let Some(r) = reply {
            writeln!(stdout, "{}", serde_json::to_string(&r)?)?;
            stdout.flush()?;
        }
    }
    Ok(())
}
