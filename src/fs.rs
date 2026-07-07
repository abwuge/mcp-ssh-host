use crate::{
    config::TargetConfig,
    edit::{apply_text_edits, EditOutcome, TextEdit},
    error::{Error, Result},
    policy::{self, FileAccess},
    ssh,
    state::AppState,
    target::{ResolvedTarget, TargetId},
    util::{sha256_hex, truncate_bytes},
};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    fs,
    io::Write,
    path::Path,
    time::{Duration, UNIX_EPOCH},
};

#[derive(Debug, Clone, Deserialize)]
pub struct FileReadRequest {
    #[serde(default)]
    pub target: Option<String>,
    pub path: String,
    #[serde(default)]
    pub max_bytes: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileReadResponse {
    pub resolved_target: ResolvedTarget,
    pub path: String,
    pub encoding: String,
    pub content: String,
    pub sha256: String,
    pub bytes: usize,
    pub truncated: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FileListRequest {
    #[serde(default)]
    pub target: Option<String>,
    pub path: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileListResponse {
    pub resolved_target: ResolvedTarget,
    pub path: String,
    pub entries: Vec<FileEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    pub name: String,
    pub path: String,
    pub kind: String,
    pub size: u64,
    pub modified_unix: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FileEditRequest {
    #[serde(default)]
    pub target: Option<String>,
    pub path: String,
    #[serde(default)]
    pub expected_sha256: Option<String>,
    pub edits: Vec<TextEdit>,
    #[serde(default)]
    pub dry_run: bool,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileEditResponse {
    pub resolved_target: ResolvedTarget,
    pub path: String,
    pub changed: bool,
    pub written: bool,
    pub old_sha256: String,
    pub new_sha256: String,
    pub diff: String,
}

pub fn read(state: &AppState, req: FileReadRequest) -> Result<FileReadResponse> {
    let (target, source) = state.resolve_target(req.target.as_deref())?;
    let config = state.get_target_config(&target)?;
    policy::check_file(&target, config, &req.path, FileAccess::Read, source)?;

    let policy = policy::target_policy(config);
    let bytes = read_bytes(
        state,
        &target,
        config,
        &req.path,
        Duration::from_millis(policy.default_timeout_ms),
    )?;
    let sha256 = sha256_hex(&bytes);
    let original_len = bytes.len();
    let (bytes, truncated) = truncate_bytes(bytes, req.max_bytes.or(Some(policy.max_output_bytes)));

    let (encoding, content) = match String::from_utf8(bytes.clone()) {
        Ok(text) => ("utf-8".to_string(), text),
        Err(_) => ("base64".to_string(), BASE64.encode(&bytes)),
    };

    Ok(FileReadResponse {
        resolved_target: state.resolved_target_value(target, source),
        path: req.path,
        encoding,
        content,
        sha256,
        bytes: original_len,
        truncated,
    })
}

pub fn list(state: &AppState, req: FileListRequest) -> Result<FileListResponse> {
    let (target, source) = state.resolve_target(req.target.as_deref())?;
    let config = state.get_target_config(&target)?;
    policy::check_file(&target, config, &req.path, FileAccess::Read, source)?;

    let entries = match (target.clone(), config) {
        (TargetId::Local, TargetConfig::Local(_)) => list_local(&req.path)?,
        (TargetId::Ssh(name), TargetConfig::Ssh(ssh_config)) => {
            let timeout = Duration::from_millis(policy::target_policy(config).default_timeout_ms);
            list_remote(state, &name, ssh_config, &req.path, timeout)?
        }
        _ => {
            return Err(Error::Target(format!(
                "target {target} has mismatched config"
            )))
        }
    };

    Ok(FileListResponse {
        resolved_target: state.resolved_target_value(target, source),
        path: req.path,
        entries,
    })
}

pub fn edit(state: &AppState, req: FileEditRequest) -> Result<FileEditResponse> {
    let (target, source) = state.resolve_target(req.target.as_deref())?;
    let config = state.get_target_config(&target)?;
    policy::check_file(&target, config, &req.path, FileAccess::Write, source)?;

    let timeout = Duration::from_millis(
        req.timeout_ms
            .unwrap_or_else(|| policy::target_policy(config).default_timeout_ms),
    );
    let original_bytes = read_bytes(state, &target, config, &req.path, timeout)?;
    let original = String::from_utf8(original_bytes).map_err(|_| {
        Error::Tool("file_edit currently supports UTF-8 text files only".to_string())
    })?;

    let EditOutcome {
        changed,
        old_sha256,
        new_sha256,
        diff,
        text,
    } = apply_text_edits(&original, req.expected_sha256.as_deref(), &req.edits)?;

    if changed && !req.dry_run {
        write_bytes(state, &target, config, &req.path, text.as_bytes(), timeout)?;
    }

    Ok(FileEditResponse {
        resolved_target: state.resolved_target_value(target, source),
        path: req.path,
        changed,
        written: changed && !req.dry_run,
        old_sha256,
        new_sha256,
        diff,
    })
}

fn read_bytes(
    state: &AppState,
    target: &TargetId,
    config: &TargetConfig,
    path: &str,
    timeout: Duration,
) -> Result<Vec<u8>> {
    match (target, config) {
        (TargetId::Local, TargetConfig::Local(_)) => Ok(fs::read(path)?),
        (TargetId::Ssh(name), TargetConfig::Ssh(ssh_config)) => {
            ssh::read_file(&state.ssh_sessions, name, ssh_config, path, timeout)
        }
        _ => Err(Error::Target(format!(
            "target {target} has mismatched config"
        ))),
    }
}

fn write_bytes(
    state: &AppState,
    target: &TargetId,
    config: &TargetConfig,
    path: &str,
    bytes: &[u8],
    timeout: Duration,
) -> Result<()> {
    match (target, config) {
        (TargetId::Local, TargetConfig::Local(_)) => write_local_atomic(path, bytes),
        (TargetId::Ssh(name), TargetConfig::Ssh(ssh_config)) => {
            ssh::write_file(&state.ssh_sessions, name, ssh_config, path, bytes, timeout)
        }
        _ => Err(Error::Target(format!(
            "target {target} has mismatched config"
        ))),
    }
}

fn write_local_atomic(path: &str, bytes: &[u8]) -> Result<()> {
    let path = Path::new(path);
    let parent = path
        .parent()
        .ok_or_else(|| Error::Tool(format!("path {} has no parent", path.display())))?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    tmp.write_all(bytes)?;
    tmp.flush()?;
    tmp.persist(path).map_err(|err| Error::Io(err.error))?;
    Ok(())
}

fn list_local(path: &str) -> Result<Vec<FileEntry>> {
    let mut entries = Vec::new();
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let meta = fs::symlink_metadata(entry.path())?;
        let kind = if meta.is_dir() {
            "dir"
        } else if meta.is_file() {
            "file"
        } else if meta.file_type().is_symlink() {
            "symlink"
        } else {
            "other"
        };
        let modified_unix = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs());
        entries.push(FileEntry {
            name: entry.file_name().to_string_lossy().to_string(),
            path: entry.path().display().to_string(),
            kind: kind.to_string(),
            size: meta.len(),
            modified_unix,
        });
    }
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(entries)
}

fn list_remote(
    state: &AppState,
    target_name: &str,
    ssh_config: &crate::config::SshTargetConfig,
    path: &str,
    timeout: Duration,
) -> Result<Vec<FileEntry>> {
    let value = ssh::list_dir(&state.ssh_sessions, target_name, ssh_config, path, timeout)?;
    let entries = value.get("entries").cloned().unwrap_or_else(|| json!([]));
    serde_json::from_value::<Vec<FileEntry>>(entries).map_err(Error::Json)
}
