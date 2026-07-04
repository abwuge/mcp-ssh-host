use crate::{
    config::{Config, SshTargetConfig},
    error::{Error, Result},
    exec::{run_program_collect, RawExecOutput},
    util::shell_quote,
};
use serde_json::Value;
use std::{path::Path, time::Duration};

pub fn connect(config: &Config, ssh: &SshTargetConfig, timeout: Duration) -> Result<RawExecOutput> {
    if !ssh.control_master {
        return exec(config, ssh, "true", None, timeout);
    }

    let mut check_args = base_args(config, ssh);
    check_args.push("-O".to_string());
    check_args.push("check".to_string());
    check_args.push(destination(ssh));
    let check = run_program_collect("ssh", &check_args, None, timeout)?;
    if check.exit_code == Some(0) {
        return Ok(check);
    }

    let mut args = base_args(config, ssh);
    args.push("-MNf".to_string());
    args.push(destination(ssh));
    run_program_collect("ssh", &args, None, timeout)
}

pub fn disconnect(config: &Config, ssh: &SshTargetConfig, timeout: Duration) -> Result<RawExecOutput> {
    if !ssh.control_master {
        return Ok(RawExecOutput {
            exit_code: Some(0),
            stdout: b"control_master disabled; nothing to disconnect\n".to_vec(),
            stderr: Vec::new(),
            timed_out: false,
        });
    }

    let mut args = base_args(config, ssh);
    args.push("-O".to_string());
    args.push("exit".to_string());
    args.push(destination(ssh));
    run_program_collect("ssh", &args, None, timeout)
}

pub fn exec(
    config: &Config,
    ssh: &SshTargetConfig,
    command: &str,
    cwd: Option<&str>,
    timeout: Duration,
) -> Result<RawExecOutput> {
    let remote_command = with_cwd(command, cwd);
    let mut args = base_args(config, ssh);
    args.push(destination(ssh));
    args.push(remote_command);
    run_program_collect("ssh", &args, None, timeout)
}

pub fn read_file(config: &Config, ssh: &SshTargetConfig, path: &str, timeout: Duration) -> Result<Vec<u8>> {
    let code = r#"import pathlib, sys
p = pathlib.Path(sys.argv[1])
sys.stdout.buffer.write(p.read_bytes())
"#;
    let output = remote_python(config, ssh, code, &[path], None, timeout)?;
    if output.exit_code == Some(0) {
        Ok(output.stdout)
    } else {
        Err(Error::Tool(format!(
            "remote read failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )))
    }
}

pub fn write_file(
    config: &Config,
    ssh: &SshTargetConfig,
    path: &str,
    bytes: &[u8],
    timeout: Duration,
) -> Result<()> {
    let code = r#"import os, pathlib, sys, tempfile
p = pathlib.Path(sys.argv[1])
parent = p.parent if str(p.parent) else pathlib.Path('.')
parent.mkdir(parents=True, exist_ok=True)
fd, tmp = tempfile.mkstemp(prefix='.' + p.name + '.', suffix='.tmp', dir=str(parent))
try:
    with os.fdopen(fd, 'wb') as f:
        f.write(sys.stdin.buffer.read())
        f.flush()
        os.fsync(f.fileno())
    os.replace(tmp, str(p))
finally:
    try:
        if os.path.exists(tmp):
            os.unlink(tmp)
    except Exception:
        pass
"#;
    let output = remote_python(config, ssh, code, &[path], Some(bytes.to_vec()), timeout)?;
    if output.exit_code == Some(0) {
        Ok(())
    } else {
        Err(Error::Tool(format!(
            "remote write failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )))
    }
}

pub fn list_dir(config: &Config, ssh: &SshTargetConfig, path: &str, timeout: Duration) -> Result<Value> {
    let code = r#"import json, pathlib, stat, sys
root = pathlib.Path(sys.argv[1])
entries = []
for child in sorted(root.iterdir(), key=lambda p: p.name):
    st = child.lstat()
    if stat.S_ISDIR(st.st_mode):
        kind = 'dir'
    elif stat.S_ISLNK(st.st_mode):
        kind = 'symlink'
    elif stat.S_ISREG(st.st_mode):
        kind = 'file'
    else:
        kind = 'other'
    entries.append({
        'name': child.name,
        'path': str(child),
        'kind': kind,
        'size': st.st_size,
        'modified_unix': int(st.st_mtime),
    })
print(json.dumps({'entries': entries}, ensure_ascii=False))
"#;
    let output = remote_python(config, ssh, code, &[path], None, timeout)?;
    if output.exit_code != Some(0) {
        return Err(Error::Tool(format!(
            "remote list failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(serde_json::from_slice(&output.stdout)?)
}

pub fn terminal_program_and_args(
    config: &Config,
    ssh: &SshTargetConfig,
    cwd: Option<&str>,
    shell: Option<&str>,
) -> (String, Vec<String>) {
    let mut args = base_args(config, ssh);
    args.push("-tt".to_string());
    args.push(destination(ssh));

    if cwd.is_some() || shell.is_some() {
        let shell = shell
            .or(ssh.shell.as_deref())
            .unwrap_or("${SHELL:-sh}");
        let mut remote = String::new();
        if let Some(cwd) = cwd {
            remote.push_str("cd ");
            remote.push_str(&shell_quote(cwd));
            remote.push_str(" && ");
        }
        remote.push_str("exec ");
        remote.push_str(shell);
        args.push(remote);
    }

    ("ssh".to_string(), args)
}

pub fn base_args(config: &Config, ssh: &SshTargetConfig) -> Vec<String> {
    let mut args = Vec::new();
    args.push("-p".to_string());
    args.push(ssh.port.to_string());

    if let Some(identity_file) = &ssh.identity_file {
        args.push("-i".to_string());
        args.push(identity_file.display().to_string());
    }

    if ssh.control_master {
        args.push("-o".to_string());
        args.push("ControlMaster=auto".to_string());
        args.push("-o".to_string());
        args.push(format!("ControlPersist={}s", ssh.control_persist_secs));
        args.push("-o".to_string());
        args.push(format!(
            "ControlPath={}",
            control_path(&config.server.runtime_dir).display()
        ));
    }

    args.extend(ssh.extra_args.clone());
    args
}

fn remote_python(
    config: &Config,
    ssh: &SshTargetConfig,
    code: &str,
    code_args: &[&str],
    stdin: Option<Vec<u8>>,
    timeout: Duration,
) -> Result<RawExecOutput> {
    let mut remote = format!("python3 -c {}", shell_quote(code));
    for arg in code_args {
        remote.push(' ');
        remote.push_str(&shell_quote(arg));
    }

    let mut args = base_args(config, ssh);
    args.push(destination(ssh));
    args.push(remote);
    run_program_collect("ssh", &args, stdin, timeout)
}

fn with_cwd(command: &str, cwd: Option<&str>) -> String {
    match cwd {
        Some(cwd) => format!("cd {} && {}", shell_quote(cwd), command),
        None => command.to_string(),
    }
}

fn destination(ssh: &SshTargetConfig) -> String {
    match &ssh.user {
        Some(user) if !user.is_empty() => format!("{user}@{}", ssh.host),
        _ => ssh.host.clone(),
    }
}

fn control_path(runtime_dir: &Path) -> std::path::PathBuf {
    runtime_dir.join("cm-%C")
}
