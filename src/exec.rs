use crate::{
    config::TargetConfig,
    error::{Error, Result},
    policy, ssh,
    state::AppState,
    target::{ResolvedTarget, TargetId},
    util::truncate_bytes,
};
use serde::{Deserialize, Serialize};
use std::{
    io::Read,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Deserialize)]
pub struct ExecRequest {
    #[serde(default)]
    pub target: Option<String>,
    pub command: String,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub max_output_bytes: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExecResponse {
    pub resolved_target: ResolvedTarget,
    pub command: String,
    pub cwd: Option<String>,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    pub timed_out: bool,
}

#[derive(Debug, Clone)]
pub struct RawExecOutput {
    pub exit_code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub timed_out: bool,
}

pub fn run(state: &AppState, req: ExecRequest) -> Result<ExecResponse> {
    let (target, source) = state.resolve_target(req.target.as_deref())?;
    let config = state.get_target_config(&target)?;
    policy::check_exec(&target, config)?;

    let policy = policy::target_policy(config);
    let timeout_ms = req.timeout_ms.unwrap_or(policy.default_timeout_ms);
    let max_output = req.max_output_bytes.or(Some(policy.max_output_bytes));

    let raw = match (target.clone(), config) {
        (TargetId::Local, TargetConfig::Local(_)) => run_local_shell(
            &req.command,
            req.cwd.as_deref(),
            Duration::from_millis(timeout_ms),
        )?,
        (TargetId::Ssh(_), TargetConfig::Ssh(ssh_config)) => ssh::exec(
            &state.config,
            ssh_config,
            &req.command,
            req.cwd.as_deref(),
            Duration::from_millis(timeout_ms),
        )?,
        _ => {
            return Err(Error::Target(format!(
                "target {target} has mismatched config"
            )))
        }
    };

    let (stdout, stdout_truncated) = truncate_bytes(raw.stdout, max_output);
    let (stderr, stderr_truncated) = truncate_bytes(raw.stderr, max_output);

    Ok(ExecResponse {
        resolved_target: state.resolved_target_value(target, source),
        command: req.command,
        cwd: req.cwd,
        exit_code: raw.exit_code,
        stdout: String::from_utf8_lossy(&stdout).to_string(),
        stderr: String::from_utf8_lossy(&stderr).to_string(),
        stdout_truncated,
        stderr_truncated,
        timed_out: raw.timed_out,
    })
}

pub fn run_local_shell(
    command: &str,
    cwd: Option<&str>,
    timeout: Duration,
) -> Result<RawExecOutput> {
    #[cfg(windows)]
    let mut cmd = {
        let mut cmd = Command::new("cmd.exe");
        cmd.arg("/C").arg(command);
        cmd
    };

    #[cfg(not(windows))]
    let mut cmd = {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "sh".to_string());
        let mut cmd = Command::new(shell);
        cmd.arg("-lc").arg(command);
        cmd
    };

    if let Some(cwd) = cwd {
        cmd.current_dir(cwd);
    }

    run_command_collect(cmd, timeout)
}

pub fn run_program_collect(
    program: &str,
    args: &[String],
    stdin: Option<Vec<u8>>,
    timeout: Duration,
) -> Result<RawExecOutput> {
    let mut cmd = Command::new(program);
    cmd.args(args);
    if stdin.is_some() {
        cmd.stdin(Stdio::piped());
    }
    run_command_collect_with_stdin(cmd, stdin, timeout)
}

pub fn run_command_collect(cmd: Command, timeout: Duration) -> Result<RawExecOutput> {
    run_command_collect_with_stdin(cmd, None, timeout)
}

fn run_command_collect_with_stdin(
    mut cmd: Command,
    stdin_data: Option<Vec<u8>>,
    timeout: Duration,
) -> Result<RawExecOutput> {
    let mut child = cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;

    if let Some(data) = stdin_data {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| Error::Tool("failed to open command stdin".to_string()))?;
        thread::spawn(move || {
            use std::io::Write;
            let _ = stdin.write_all(&data);
        });
    }

    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| Error::Tool("failed to open command stdout".to_string()))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| Error::Tool("failed to open command stderr".to_string()))?;

    let stdout_thread = thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        buf
    });
    let stderr_thread = thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr.read_to_end(&mut buf);
        buf
    });

    let started = Instant::now();
    let mut timed_out = false;
    let exit_code = loop {
        if let Some(status) = child.try_wait()? {
            break status.code();
        }

        if started.elapsed() > timeout {
            timed_out = true;
            let _ = child.kill();
            let status = child.wait()?;
            break status.code();
        }

        thread::sleep(Duration::from_millis(20));
    };

    let stdout = stdout_thread
        .join()
        .map_err(|_| Error::Tool("stdout reader panicked".to_string()))?;
    let stderr = stderr_thread
        .join()
        .map_err(|_| Error::Tool("stderr reader panicked".to_string()))?;

    Ok(RawExecOutput {
        exit_code,
        stdout,
        stderr,
        timed_out,
    })
}
