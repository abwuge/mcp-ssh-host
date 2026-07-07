use crate::core::{
    config::{PolicyConfig, TargetConfig},
    error::{Error, Result},
    target::{TargetId, TargetSource},
};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Copy)]
pub enum FileAccess {
    Read,
    Write,
}

pub fn target_policy(config: &TargetConfig) -> &PolicyConfig {
    match config {
        TargetConfig::Local(local) => &local.policy,
        TargetConfig::Ssh(ssh) => &ssh.policy,
    }
}

pub fn target_enabled(config: &TargetConfig) -> bool {
    match config {
        TargetConfig::Local(local) => local.enabled,
        TargetConfig::Ssh(ssh) => ssh.enabled,
    }
}

pub fn check_target_enabled(target: &TargetId, config: &TargetConfig) -> Result<()> {
    if target_enabled(config) {
        Ok(())
    } else {
        Err(Error::Policy(format!("target {target} is disabled")))
    }
}

pub fn check_select_active(target: &TargetId, config: &TargetConfig) -> Result<()> {
    let policy = target_policy(config);
    if policy.allow_select_active {
        Ok(())
    } else {
        Err(Error::Policy(format!(
            "target {target} is not allowed to become active"
        )))
    }
}

pub fn check_exec(target: &TargetId, config: &TargetConfig) -> Result<()> {
    check_target_enabled(target, config)?;
    let policy = target_policy(config);
    if policy.allow_exec {
        Ok(())
    } else {
        Err(Error::Policy(format!("exec is disabled for {target}")))
    }
}

pub fn check_terminal(target: &TargetId, config: &TargetConfig) -> Result<()> {
    check_target_enabled(target, config)?;
    let policy = target_policy(config);
    if policy.allow_terminal {
        Ok(())
    } else {
        Err(Error::Policy(format!("terminal is disabled for {target}")))
    }
}

pub fn check_file(
    target: &TargetId,
    config: &TargetConfig,
    path: &str,
    access: FileAccess,
    source: TargetSource,
) -> Result<()> {
    check_target_enabled(target, config)?;
    let policy = target_policy(config);

    match access {
        FileAccess::Read if !policy.allow_file_read => {
            return Err(Error::Policy(format!("file read is disabled for {target}")));
        }
        FileAccess::Write if !policy.allow_file_write => {
            return Err(Error::Policy(format!(
                "file write is disabled for {target}"
            )));
        }
        _ => {}
    }

    if matches!(access, FileAccess::Write)
        && policy.require_explicit_target_for_write
        && source != TargetSource::Explicit
    {
        return Err(Error::Policy(format!(
            "file write on {target} requires an explicit target argument"
        )));
    }

    if policy.allowed_roots.is_empty() {
        return Err(Error::Policy(format!(
            "no allowed_roots configured for {target}; refusing path access"
        )));
    }

    match target {
        TargetId::Local => check_local_path(path, &policy.allowed_roots),
        TargetId::Ssh(_) => check_remote_path(path, &policy.allowed_roots),
    }
}

fn check_local_path(path: &str, allowed_roots: &[String]) -> Result<()> {
    let candidate = canonicalize_best_effort(Path::new(path))?;

    for root in allowed_roots {
        let root_path = canonicalize_best_effort(Path::new(root))?;
        if candidate.starts_with(&root_path) {
            return Ok(());
        }
    }

    Err(Error::Policy(format!(
        "path {} is outside allowed_roots",
        candidate.display()
    )))
}

fn check_remote_path(path: &str, allowed_roots: &[String]) -> Result<()> {
    if path.contains('\0') {
        return Err(Error::Policy("path contains NUL byte".to_string()));
    }

    let normalized = normalize_remote_path(path);
    for root in allowed_roots {
        let root = normalize_remote_path(root);
        if normalized == root
            || normalized.starts_with(&(root.trim_end_matches('/').to_string() + "/"))
        {
            return Ok(());
        }
    }

    Err(Error::Policy(format!(
        "remote path {normalized} is outside allowed_roots"
    )))
}

fn canonicalize_best_effort(path: &Path) -> Result<PathBuf> {
    if path.exists() {
        return fs::canonicalize(path).map_err(Error::Io);
    }

    let parent = path
        .parent()
        .ok_or_else(|| Error::Policy(format!("path {} has no parent", path.display())))?;
    let file_name = path
        .file_name()
        .ok_or_else(|| Error::Policy(format!("path {} has no file name", path.display())))?;
    let canonical_parent = fs::canonicalize(parent).map_err(Error::Io)?;
    Ok(canonical_parent.join(file_name))
}

fn normalize_remote_path(path: &str) -> String {
    let mut parts = Vec::new();
    let absolute = path.starts_with('/');

    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }

    let mut normalized = parts.join("/");
    if absolute {
        normalized.insert(0, '/');
    }
    if normalized.is_empty() {
        normalized.push('/');
    }
    normalized
}
