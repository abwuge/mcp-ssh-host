use crate::{error::{Error, Result}, state::AppState, tools};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{io::{self, BufRead, Write}, sync::Arc};

#[derive(Debug, Deserialize)]
struct RpcRequest {
    #[serde(default, rename = "jsonrpc")]
    _jsonrpc: Option<String>,
    #[serde(default)]
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Option<Value>,
}

#[derive(Debug, Serialize)]
struct RpcResponse {
    jsonrpc: &'static str,
    id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<RpcError>,
}

#[derive(Debug, Serialize)]
struct RpcError {
    code: i64,
    message: String,
}

pub fn serve_stdio(state: Arc<AppState>) -> Result<()> {
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }

        let response = match serde_json::from_str::<RpcRequest>(&line) {
            Ok(request) => handle_request(Arc::clone(&state), request),
            Err(err) => Some(RpcResponse {
                jsonrpc: "2.0",
                id: None,
                result: None,
                error: Some(RpcError {
                    code: -32700,
                    message: format!("parse error: {err}"),
                }),
            }),
        };

        if let Some(response) = response {
            serde_json::to_writer(&mut stdout, &response)?;
            stdout.write_all(b"\n")?;
            stdout.flush()?;
        }
    }

    Ok(())
}

fn handle_request(state: Arc<AppState>, request: RpcRequest) -> Option<RpcResponse> {
    if request.method.starts_with("notifications/") {
        return None;
    }

    let id = request.id.clone();
    let result = match request.method.as_str() {
        "initialize" => initialize(&state, request.params.unwrap_or_else(|| json!({}))),
        "tools/list" => Ok(json!({ "tools": tools::list_tools() })),
        "tools/call" => tools_call(state, request.params.unwrap_or_else(|| json!({}))),
        "ping" => Ok(json!({})),
        other => Err(Error::Tool(format!("unsupported method: {other}"))),
    };

    match result {
        Ok(value) => Some(RpcResponse {
            jsonrpc: "2.0",
            id,
            result: Some(value),
            error: None,
        }),
        Err(err) => Some(RpcResponse {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(RpcError {
                code: err.json_rpc_code(),
                message: err.to_string(),
            }),
        }),
    }
}

fn initialize(state: &AppState, params: Value) -> Result<Value> {
    let requested_protocol = params
        .get("protocolVersion")
        .and_then(|v| v.as_str())
        .unwrap_or("2025-06-18");

    Ok(json!({
        "protocolVersion": requested_protocol,
        "capabilities": {
            "tools": { "listChanged": false }
        },
        "serverInfo": {
            "name": state.config.server.name.clone(),
            "version": state.config.server.version.clone(),
        }
    }))
}

fn tools_call(state: Arc<AppState>, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct ToolCallParams {
        name: String,
        #[serde(default)]
        arguments: Option<Value>,
    }

    let params: ToolCallParams = serde_json::from_value(params)?;
    match tools::call_tool(state, &params.name, params.arguments.unwrap_or_else(|| json!({}))) {
        Ok(value) => Ok(json!({
            "content": [{
                "type": "text",
                "text": serde_json::to_string_pretty(&value)?,
            }],
            "isError": false,
        })),
        Err(err) => Ok(json!({
            "content": [{
                "type": "text",
                "text": err.to_string(),
            }],
            "isError": true,
        })),
    }
}
