use crate::{
    config::TargetConfig,
    error::{Error, Result},
    exec::{self, ExecRequest},
    fs::{self, FileEditRequest, FileListRequest, FileReadRequest},
    policy, ssh,
    state::AppState,
    target::{ResolvedTarget, TargetId},
    terminal::{
        TerminalCloseRequest, TerminalOpenRequest, TerminalReadRequest, TerminalResizeRequest,
        TerminalSendRequest,
    },
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{str::FromStr, sync::Arc, time::Duration};

pub fn list_tools() -> Value {
    json!([
        tool("server_info", "Return server configuration summary and active target state.", object_schema(vec![])),
        tool("target_list", "List configured local and SSH targets, including policy summaries and active marker.", object_schema(vec![])),
        tool("target_current", "Return the currently selected active target, if any.", object_schema(vec![])),
        tool("target_select", "Select a session-scoped active target. Later calls may omit target and use this sticky target.", object_schema(vec![required_string("target", "Target id: local or ssh:<profile>")])),
        tool("target_connect", "Connect or warm an SSH target persistent worker.", object_schema(vec![required_string("target", "Target id: local or ssh:<profile>")])),
        tool("target_disconnect", "Disconnect an SSH target persistent worker, or no-op for local targets.", object_schema(vec![required_string("target", "Target id: local or ssh:<profile>")])),
        tool("exec", "Run a non-interactive command on the explicit target or current active target.", object_schema(vec![
            optional_string("target", "Target id: local or ssh:<profile>. Omit to use active target."),
            required_string("command", "Shell command to execute."),
            optional_string("cwd", "Working directory."),
            optional_integer("timeout_ms", "Timeout in milliseconds."),
            optional_integer("max_output_bytes", "Maximum bytes to return for stdout and stderr."),
        ])),
        tool("file_read", "Read a UTF-8 or binary file from the explicit target or active target.", object_schema(vec![
            optional_string("target", "Target id. Omit to use active target."),
            required_string("path", "File path."),
            optional_integer("max_bytes", "Maximum bytes to return."),
        ])),
        tool("file_list", "List one directory on the explicit target or active target.", object_schema(vec![
            optional_string("target", "Target id. Omit to use active target."),
            required_string("path", "Directory path."),
        ])),
        tool("file_edit", "Apply exact text replacements with sha256 compare-and-swap support. Writes require explicit target by default.", file_edit_schema()),
        tool("terminal_open", "Open a persistent PTY terminal on the explicit target or active target.", object_schema(vec![
            optional_string("target", "Target id. Omit to use active target."),
            optional_string("cwd", "Initial working directory."),
            optional_string("shell", "Shell program to run."),
            optional_integer("rows", "PTY rows."),
            optional_integer("cols", "PTY columns."),
        ])),
        tool("terminal_send", "Send input to an existing terminal_id. The terminal is already bound to its target.", object_schema(vec![
            required_string("terminal_id", "Terminal id from terminal_open."),
            required_string("input", "Input bytes represented as UTF-8 text, usually ending in newline."),
        ])),
        tool("terminal_read", "Read incremental output from an existing terminal_id.", object_schema(vec![
            required_string("terminal_id", "Terminal id from terminal_open."),
            optional_integer("since_seq", "Last seen sequence number. Omit or 0 to read buffered output."),
            optional_integer("max_bytes", "Maximum output bytes."),
        ])),
        tool("terminal_resize", "Record a terminal resize request. Actual PTY resize is marked TODO in this MVP.", object_schema(vec![
            required_string("terminal_id", "Terminal id from terminal_open."),
            required_integer("rows", "PTY rows."),
            required_integer("cols", "PTY columns."),
        ])),
        tool("terminal_close", "Close an existing terminal session.", object_schema(vec![
            required_string("terminal_id", "Terminal id from terminal_open."),
        ])),
    ])
}

pub fn call_tool(state: Arc<AppState>, name: &str, args: Value) -> Result<Value> {
    match name {
        "server_info" => Ok(server_info(&state)),
        "target_list" => Ok(json!({ "targets": state.list_targets() })),
        "target_current" => Ok(target_current(&state)),
        "target_select" => target_select(&state, parse(args)?),
        "target_connect" => target_connect(&state, parse(args)?),
        "target_disconnect" => target_disconnect(&state, parse(args)?),
        "exec" => Ok(serde_json::to_value(exec::run(
            &state,
            parse::<ExecRequest>(args)?,
        )?)?),
        "file_read" => Ok(serde_json::to_value(fs::read(
            &state,
            parse::<FileReadRequest>(args)?,
        )?)?),
        "file_list" => Ok(serde_json::to_value(fs::list(
            &state,
            parse::<FileListRequest>(args)?,
        )?)?),
        "file_edit" => Ok(serde_json::to_value(fs::edit(
            &state,
            parse::<FileEditRequest>(args)?,
        )?)?),
        "terminal_open" => Ok(serde_json::to_value(
            state
                .terminals
                .open(&state, parse::<TerminalOpenRequest>(args)?)?,
        )?),
        "terminal_send" => Ok(serde_json::to_value(state.terminals.send(parse::<
            TerminalSendRequest,
        >(
            args
        )?)?)?),
        "terminal_read" => Ok(serde_json::to_value(state.terminals.read(parse::<
            TerminalReadRequest,
        >(
            args
        )?)?)?),
        "terminal_resize" => Ok(serde_json::to_value(state.terminals.resize(parse::<
            TerminalResizeRequest,
        >(
            args
        )?)?)?),
        "terminal_close" => Ok(serde_json::to_value(state.terminals.close(parse::<
            TerminalCloseRequest,
        >(
            args
        )?)?)?),
        other => Err(Error::Tool(format!("unknown tool: {other}"))),
    }
}

#[derive(Debug, Deserialize)]
struct TargetRequest {
    target: String,
}

fn parse<T: for<'de> Deserialize<'de>>(value: Value) -> Result<T> {
    serde_json::from_value(value).map_err(Error::Json)
}

fn server_info(state: &AppState) -> Value {
    let active_target = state.current_target().map(|target| target.to_string());
    json!({
        "name": state.config.server.name.clone(),
        "version": state.config.server.version.clone(),
        "active_target": active_target,
        "ssh_session_ids": state.ssh_sessions.ids(),
        "terminal_ids": state.terminals.ids(),
        "runtime_dir": state.config.server.runtime_dir.display().to_string(),
        "started_at_debug": format!("{:?}", state.started_at()),
        "notes": [
            "MVP stdio MCP implementation with tools/list and tools/call.",
            "The SSH backend uses persistent per-target OpenSSH worker processes for exec and file operations.",
            "target is sticky only inside this MCP server process/session."
        ]
    })
}

fn target_current(state: &AppState) -> Value {
    match state.current_target() {
        Some(target) => json!({ "active_target": target.to_string() }),
        None => json!({ "active_target": null }),
    }
}

fn target_select(state: &AppState, req: TargetRequest) -> Result<Value> {
    let target = TargetId::from_str(&req.target)?;
    let config = state.get_target_config(&target)?;
    policy::check_target_enabled(&target, config)?;
    policy::check_select_active(&target, config)?;
    let previous = state.set_active_target(target.clone());
    Ok(json!({
        "active_target": target.to_string(),
        "previous_target": previous.map(|t| t.to_string()),
    }))
}

fn target_connect(state: &AppState, req: TargetRequest) -> Result<Value> {
    let target = TargetId::from_str(&req.target)?;
    let config = state.get_target_config(&target)?;
    policy::check_target_enabled(&target, config)?;
    match (target.clone(), config) {
        (TargetId::Local, TargetConfig::Local(_)) => Ok(json!({
            "resolved_target": ResolvedTarget::new(target, crate::target::TargetSource::Explicit),
            "connected": true,
            "message": "local target is always available when enabled"
        })),
        (TargetId::Ssh(name), TargetConfig::Ssh(ssh_config)) => {
            let timeout = Duration::from_millis(policy::target_policy(config).default_timeout_ms);
            let output = ssh::connect(&state.ssh_sessions, &name, ssh_config, timeout)?;
            Ok(json!({
                "resolved_target": ResolvedTarget::new(target, crate::target::TargetSource::Explicit),
                "connected": output.exit_code == Some(0),
                "exit_code": output.exit_code,
                "stdout": String::from_utf8_lossy(&output.stdout),
                "stderr": String::from_utf8_lossy(&output.stderr),
                "timed_out": output.timed_out,
            }))
        }
        _ => Err(Error::Target(format!(
            "target {target} has mismatched config"
        ))),
    }
}

fn target_disconnect(state: &AppState, req: TargetRequest) -> Result<Value> {
    let target = TargetId::from_str(&req.target)?;
    let config = state.get_target_config(&target)?;
    match (target.clone(), config) {
        (TargetId::Local, TargetConfig::Local(_)) => Ok(json!({
            "resolved_target": ResolvedTarget::new(target, crate::target::TargetSource::Explicit),
            "disconnected": true,
            "message": "local target has no connection to close"
        })),
        (TargetId::Ssh(name), TargetConfig::Ssh(_)) => {
            let timeout = Duration::from_millis(policy::target_policy(config).default_timeout_ms);
            let output = ssh::disconnect(&state.ssh_sessions, &name, timeout)?;
            Ok(json!({
                "resolved_target": ResolvedTarget::new(target, crate::target::TargetSource::Explicit),
                "disconnected": output.exit_code == Some(0),
                "exit_code": output.exit_code,
                "stdout": String::from_utf8_lossy(&output.stdout),
                "stderr": String::from_utf8_lossy(&output.stderr),
                "timed_out": output.timed_out,
            }))
        }
        _ => Err(Error::Target(format!(
            "target {target} has mismatched config"
        ))),
    }
}

fn tool(name: &str, description: &str, input_schema: Value) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": input_schema,
    })
}

#[derive(Debug, Clone)]
struct Prop {
    name: &'static str,
    value: Value,
    required: bool,
}

fn object_schema(props: Vec<Prop>) -> Value {
    let mut properties = serde_json::Map::new();
    let mut required = Vec::new();
    for prop in props {
        properties.insert(prop.name.to_string(), prop.value);
        if prop.required {
            required.push(Value::String(prop.name.to_string()));
        }
    }
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}

fn required_string(name: &'static str, description: &'static str) -> Prop {
    Prop {
        name,
        required: true,
        value: json!({ "type": "string", "description": description }),
    }
}

fn optional_string(name: &'static str, description: &'static str) -> Prop {
    Prop {
        name,
        required: false,
        value: json!({ "type": "string", "description": description }),
    }
}

fn required_integer(name: &'static str, description: &'static str) -> Prop {
    Prop {
        name,
        required: true,
        value: json!({ "type": "integer", "description": description }),
    }
}

fn optional_integer(name: &'static str, description: &'static str) -> Prop {
    Prop {
        name,
        required: false,
        value: json!({ "type": "integer", "description": description }),
    }
}

fn file_edit_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "target": { "type": "string", "description": "Target id. For writes this is required by default policy." },
            "path": { "type": "string", "description": "UTF-8 text file path." },
            "expected_sha256": { "type": "string", "description": "Optional CAS guard from file_read." },
            "dry_run": { "type": "boolean", "description": "Return diff without writing." },
            "timeout_ms": { "type": "integer", "description": "Timeout for remote read/write." },
            "edits": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "old": { "type": "string" },
                        "new": { "type": "string" },
                        "replace_all": { "type": "boolean" }
                    },
                    "required": ["old", "new"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["path", "edits"],
        "additionalProperties": false
    })
}
