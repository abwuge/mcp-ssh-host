use crate::{
    config::{Config, SshTargetConfig},
    error::{Error, Result},
    exec::{run_program_collect, RawExecOutput},
    util::shell_quote,
};
use serde_json::{json, Value};
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

pub fn disconnect(
    config: &Config,
    ssh: &SshTargetConfig,
    timeout: Duration,
) -> Result<RawExecOutput> {
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

pub fn read_file(
    config: &Config,
    ssh: &SshTargetConfig,
    path: &str,
    timeout: Duration,
) -> Result<Vec<u8>> {
    let output = remote_sh(config, ssh, r#"cat < "$1""#, &[path], None, timeout)?;
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
    let script = r#"
p=$1
parent=${p%/*}
if [ "$parent" = "$p" ]; then
    parent=.
fi
if [ -z "$parent" ]; then
    parent=/
fi
base=${p##*/}
if [ -z "$base" ]; then
    printf "%s\n" "refusing to write directory path: $p" >&2
    exit 1
fi
mkdir -p "$parent" || exit 1
tmp=$(mktemp "$parent/.$base.XXXXXX") || exit 1
cleanup() {
    rm -f "$tmp"
}
trap cleanup EXIT HUP INT TERM
cat > "$tmp" || exit 1
mv -f "$tmp" "$p" || exit 1
"#;
    let output = remote_sh(config, ssh, script, &[path], Some(bytes.to_vec()), timeout)?;
    if output.exit_code == Some(0) {
        Ok(())
    } else {
        Err(Error::Tool(format!(
            "remote write failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )))
    }
}

pub fn list_dir(
    config: &Config,
    ssh: &SshTargetConfig,
    path: &str,
    timeout: Duration,
) -> Result<Value> {
    let script = r#"
dir=$1
if [ ! -d "$dir" ]; then
    printf "%s\n" "not a directory: $dir" >&2
    exit 1
fi
for child in "$dir"/* "$dir"/.[!.]* "$dir"/..?*; do
    [ -e "$child" ] || [ -L "$child" ] || continue
    if [ -L "$child" ]; then
        kind=symlink
    elif [ -d "$child" ]; then
        kind=dir
    elif [ -f "$child" ]; then
        kind=file
    else
        kind=other
    fi
    size=$(stat -c "%s" "$child" 2>/dev/null || stat -f "%z" "$child" 2>/dev/null || printf 0)
    modified=$(stat -c "%Y" "$child" 2>/dev/null || stat -f "%m" "$child" 2>/dev/null || printf "")
    name=${child##*/}
    printf "%s\000%s\000%s\000%s\000%s\000" "$name" "$child" "$kind" "$size" "$modified"
done
"#;
    let output = remote_sh(config, ssh, script, &[path], None, timeout)?;
    if output.exit_code != Some(0) {
        return Err(Error::Tool(format!(
            "remote list failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    parse_list_dir_output(&output.stdout)
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
        let shell = shell.or(ssh.shell.as_deref()).unwrap_or("${SHELL:-sh}");
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

fn remote_sh(
    config: &Config,
    ssh: &SshTargetConfig,
    script: &str,
    script_args: &[&str],
    stdin: Option<Vec<u8>>,
    timeout: Duration,
) -> Result<RawExecOutput> {
    let mut remote = format!("sh -c {} sh", shell_quote(script));
    for arg in script_args {
        remote.push(' ');
        remote.push_str(&shell_quote(arg));
    }

    let mut args = base_args(config, ssh);
    args.push(destination(ssh));
    args.push(remote);
    run_program_collect("ssh", &args, stdin, timeout)
}

fn parse_list_dir_output(stdout: &[u8]) -> Result<Value> {
    let mut fields: Vec<&[u8]> = stdout.split(|byte| *byte == 0).collect();
    if fields.last().is_some_and(|field| field.is_empty()) {
        fields.pop();
    }
    let chunks = fields.chunks_exact(5);
    if !chunks.remainder().is_empty() {
        return Err(Error::Tool(format!(
            "remote list returned malformed metadata: expected groups of 5 fields, got {}",
            fields.len()
        )));
    }

    let mut entries = Vec::new();
    for field in chunks {
        let name = decode_field(field[0]);
        let path = decode_field(field[1]);
        let kind = decode_field(field[2]);
        let size_text = decode_field(field[3]);
        let modified_text = decode_field(field[4]);

        let size = size_text.trim().parse::<u64>().map_err(|err| {
            Error::Tool(format!(
                "remote list returned invalid size for {path}: {size_text:?}: {err}"
            ))
        })?;
        let modified_unix = if modified_text.trim().is_empty() {
            None
        } else {
            Some(modified_text.trim().parse::<u64>().map_err(|err| {
                Error::Tool(format!(
                    "remote list returned invalid mtime for {path}: {modified_text:?}: {err}"
                ))
            })?)
        };

        entries.push(json!({
            "name": name,
            "path": path,
            "kind": kind,
            "size": size,
            "modified_unix": modified_unix,
        }));
    }

    entries.sort_by(|left, right| {
        let left = left.get("name").and_then(Value::as_str).unwrap_or_default();
        let right = right
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        left.cmp(right)
    });

    Ok(json!({ "entries": entries }))
}

fn decode_field(field: &[u8]) -> String {
    String::from_utf8_lossy(field).to_string()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_remote_list_nul_records() {
        let fields: [&[u8]; 10] = [
            b"b", b"/tmp/b", b"file", b"12", b"172", b"a", b"/tmp/a", b"dir", b"0", b"",
        ];
        let mut output = Vec::new();
        for field in fields {
            output.extend_from_slice(field);
            output.push(0);
        }

        let value = parse_list_dir_output(&output).unwrap();
        let entries = value["entries"].as_array().unwrap();

        assert_eq!(entries[0]["name"].as_str(), Some("a"));
        assert_eq!(entries[0]["modified_unix"], Value::Null);
        assert_eq!(entries[1]["name"].as_str(), Some("b"));
        assert_eq!(entries[1]["size"].as_u64(), Some(12));
        assert_eq!(entries[1]["modified_unix"].as_u64(), Some(172));
    }

    #[test]
    fn rejects_malformed_remote_list_records() {
        let err = parse_list_dir_output(b"name\0path\0").unwrap_err();
        assert!(err.to_string().contains("malformed metadata"));
    }
}
