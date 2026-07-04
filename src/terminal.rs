use crate::{
    config::TargetConfig,
    error::{Error, Result},
    policy, ssh,
    state::AppState,
    target::{ResolvedTarget, TargetId},
};
use portable_pty::{native_pty_system, Child, CommandBuilder, PtySize};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, VecDeque},
    io::{Read, Write},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    thread,
};

static TERMINAL_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Deserialize)]
pub struct TerminalOpenRequest {
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub shell: Option<String>,
    #[serde(default = "default_rows")]
    pub rows: u16,
    #[serde(default = "default_cols")]
    pub cols: u16,
}

#[derive(Debug, Clone, Serialize)]
pub struct TerminalOpenResponse {
    pub resolved_target: ResolvedTarget,
    pub terminal_id: String,
    pub rows: u16,
    pub cols: u16,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TerminalSendRequest {
    pub terminal_id: String,
    pub input: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct TerminalSendResponse {
    pub terminal_id: String,
    pub bytes_written: usize,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TerminalReadRequest {
    pub terminal_id: String,
    #[serde(default)]
    pub since_seq: Option<u64>,
    #[serde(default)]
    pub max_bytes: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TerminalReadResponse {
    pub terminal_id: String,
    pub target: String,
    pub from_seq: u64,
    pub next_seq: u64,
    pub output: String,
    pub truncated: bool,
    pub eof: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TerminalResizeRequest {
    pub terminal_id: String,
    pub rows: u16,
    pub cols: u16,
}

#[derive(Debug, Clone, Serialize)]
pub struct TerminalResizeResponse {
    pub terminal_id: String,
    pub rows: u16,
    pub cols: u16,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TerminalCloseRequest {
    pub terminal_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct TerminalCloseResponse {
    pub terminal_id: String,
    pub closed: bool,
}

pub struct TerminalRegistry {
    sessions: Mutex<HashMap<String, Arc<TerminalSession>>>,
    default_buffer_bytes: usize,
}

pub struct TerminalSession {
    target: TargetId,
    writer: Mutex<Box<dyn Write + Send>>,
    child: Mutex<Box<dyn Child + Send>>,
    buffer: Arc<RingBuffer>,
    eof: Arc<Mutex<bool>>,
}

pub struct RingBuffer {
    inner: Mutex<RingBufferInner>,
    max_bytes: usize,
}

struct RingBufferInner {
    chunks: VecDeque<OutputChunk>,
    next_seq: u64,
    current_bytes: usize,
}

struct OutputChunk {
    seq: u64,
    bytes: Vec<u8>,
}

impl TerminalRegistry {
    pub fn new(default_buffer_bytes: usize) -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            default_buffer_bytes,
        }
    }

    pub fn open(&self, state: &AppState, req: TerminalOpenRequest) -> Result<TerminalOpenResponse> {
        let (target, source) = state.resolve_target(req.target.as_deref())?;
        let config = state.get_target_config(&target)?;
        policy::check_terminal(&target, config)?;

        let (program, args, cwd) = match (target.clone(), config) {
            (TargetId::Local, TargetConfig::Local(local)) => {
                let shell = req
                    .shell
                    .clone()
                    .or_else(|| local.shell.clone())
                    .or_else(|| std::env::var("SHELL").ok())
                    .unwrap_or_else(|| "sh".to_string());
                (shell, Vec::new(), req.cwd.clone())
            }
            (TargetId::Ssh(_), TargetConfig::Ssh(ssh_config)) => {
                let (program, args) = ssh::terminal_program_and_args(
                    &state.config,
                    ssh_config,
                    req.cwd.as_deref(),
                    req.shell.as_deref(),
                );
                (program, args, None)
            }
            _ => {
                return Err(Error::Target(format!(
                    "target {target} has mismatched config"
                )))
            }
        };

        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: req.rows,
                cols: req.cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|err| Error::Terminal(err.to_string()))?;

        let mut cmd = CommandBuilder::new(program);
        for arg in args {
            cmd.arg(arg);
        }
        if let Some(cwd) = cwd {
            cmd.cwd(cwd);
        }

        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|err| Error::Terminal(err.to_string()))?;
        drop(pair.slave);

        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|err| Error::Terminal(err.to_string()))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|err| Error::Terminal(err.to_string()))?;

        let id = format!("term_{}", TERMINAL_COUNTER.fetch_add(1, Ordering::Relaxed));
        let buffer = Arc::new(RingBuffer::new(self.default_buffer_bytes));
        let eof = Arc::new(Mutex::new(false));
        let reader_buffer = Arc::clone(&buffer);
        let reader_eof = Arc::clone(&eof);

        thread::spawn(move || {
            let mut scratch = [0_u8; 8192];
            loop {
                match reader.read(&mut scratch) {
                    Ok(0) => {
                        *reader_eof.lock().unwrap() = true;
                        break;
                    }
                    Ok(n) => reader_buffer.push(&scratch[..n]),
                    Err(_) => {
                        *reader_eof.lock().unwrap() = true;
                        break;
                    }
                }
            }
        });

        let session = Arc::new(TerminalSession {
            target: target.clone(),
            writer: Mutex::new(writer),
            child: Mutex::new(child),
            buffer,
            eof,
        });

        self.sessions.lock().unwrap().insert(id.clone(), session);

        Ok(TerminalOpenResponse {
            resolved_target: state.resolved_target_value(target, source),
            terminal_id: id,
            rows: req.rows,
            cols: req.cols,
        })
    }

    pub fn send(&self, req: TerminalSendRequest) -> Result<TerminalSendResponse> {
        let session = self.get(&req.terminal_id)?;
        let mut writer = session.writer.lock().unwrap();
        writer.write_all(req.input.as_bytes())?;
        writer.flush()?;
        Ok(TerminalSendResponse {
            terminal_id: req.terminal_id,
            bytes_written: req.input.len(),
        })
    }

    pub fn read(&self, req: TerminalReadRequest) -> Result<TerminalReadResponse> {
        let session = self.get(&req.terminal_id)?;
        let from_seq = req.since_seq.unwrap_or(0);
        let (output, next_seq, truncated) = session
            .buffer
            .read_since(from_seq, req.max_bytes.unwrap_or(64 * 1024));
        let eof = *session.eof.lock().unwrap();
        Ok(TerminalReadResponse {
            terminal_id: req.terminal_id,
            target: session.target.to_string(),
            from_seq,
            next_seq,
            output: String::from_utf8_lossy(&output).to_string(),
            truncated,
            eof,
        })
    }

    pub fn resize(&self, req: TerminalResizeRequest) -> Result<TerminalResizeResponse> {
        let session = self.get(&req.terminal_id)?;
        session.buffer.push(
            format!(
                "\r\n[mcp-ssh-host: resize requested to {}x{}; PTY resize is not yet wired]\r\n",
                req.rows, req.cols
            )
            .as_bytes(),
        );
        Ok(TerminalResizeResponse {
            terminal_id: req.terminal_id,
            rows: req.rows,
            cols: req.cols,
        })
    }

    pub fn close(&self, req: TerminalCloseRequest) -> Result<TerminalCloseResponse> {
        let session = self.sessions.lock().unwrap().remove(&req.terminal_id);
        if let Some(session) = session {
            let _ = session.child.lock().unwrap().kill();
            Ok(TerminalCloseResponse {
                terminal_id: req.terminal_id,
                closed: true,
            })
        } else {
            Ok(TerminalCloseResponse {
                terminal_id: req.terminal_id,
                closed: false,
            })
        }
    }

    pub fn ids(&self) -> Vec<String> {
        self.sessions.lock().unwrap().keys().cloned().collect()
    }

    fn get(&self, id: &str) -> Result<Arc<TerminalSession>> {
        self.sessions
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or_else(|| Error::Terminal(format!("terminal {id} not found")))
    }
}

impl RingBuffer {
    pub fn new(max_bytes: usize) -> Self {
        Self {
            inner: Mutex::new(RingBufferInner {
                chunks: VecDeque::new(),
                next_seq: 1,
                current_bytes: 0,
            }),
            max_bytes,
        }
    }

    pub fn push(&self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        let mut inner = self.inner.lock().unwrap();
        let seq = inner.next_seq;
        inner.next_seq += 1;
        inner.current_bytes += bytes.len();
        inner.chunks.push_back(OutputChunk {
            seq,
            bytes: bytes.to_vec(),
        });

        while inner.current_bytes > self.max_bytes {
            if let Some(old) = inner.chunks.pop_front() {
                inner.current_bytes = inner.current_bytes.saturating_sub(old.bytes.len());
            } else {
                break;
            }
        }
    }

    pub fn read_since(&self, since_seq: u64, max_bytes: usize) -> (Vec<u8>, u64, bool) {
        let inner = self.inner.lock().unwrap();
        let mut out = Vec::new();
        let mut truncated = false;

        for chunk in inner.chunks.iter().filter(|chunk| chunk.seq > since_seq) {
            if out.len() + chunk.bytes.len() > max_bytes {
                let remaining = max_bytes.saturating_sub(out.len());
                out.extend_from_slice(&chunk.bytes[..remaining.min(chunk.bytes.len())]);
                truncated = true;
                break;
            }
            out.extend_from_slice(&chunk.bytes);
        }

        (out, inner.next_seq.saturating_sub(1), truncated)
    }
}

fn default_rows() -> u16 {
    30
}

fn default_cols() -> u16 {
    120
}
