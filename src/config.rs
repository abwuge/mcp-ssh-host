use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub server: ServerConfig,

    #[serde(default)]
    pub targets: BTreeMap<String, TargetConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    #[serde(default = "default_name")]
    pub name: String,

    #[serde(default = "default_version")]
    pub version: String,

    #[serde(default)]
    pub default_target: Option<String>,

    #[serde(default = "default_ring_buffer_bytes")]
    pub terminal_ring_buffer_bytes: usize,

    #[serde(default = "default_runtime_dir")]
    pub runtime_dir: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TargetConfig {
    Local(LocalTargetConfig),
    Ssh(SshTargetConfig),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalTargetConfig {
    #[serde(default)]
    pub enabled: bool,

    #[serde(default)]
    pub shell: Option<String>,

    #[serde(default)]
    pub policy: PolicyConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SshTargetConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,

    pub host: String,

    #[serde(default = "default_ssh_port")]
    pub port: u16,

    #[serde(default)]
    pub user: Option<String>,

    #[serde(default)]
    pub identity_file: Option<PathBuf>,

    #[serde(default = "default_true")]
    pub control_master: bool,

    #[serde(default = "default_control_persist_secs")]
    pub control_persist_secs: u64,

    #[serde(default)]
    pub extra_args: Vec<String>,

    #[serde(default)]
    pub shell: Option<String>,

    #[serde(default)]
    pub policy: PolicyConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyConfig {
    #[serde(default)]
    pub allow_exec: bool,

    #[serde(default)]
    pub allow_terminal: bool,

    #[serde(default)]
    pub allow_file_read: bool,

    #[serde(default)]
    pub allow_file_write: bool,

    #[serde(default)]
    pub allow_select_active: bool,

    #[serde(default = "default_true")]
    pub require_explicit_target_for_write: bool,

    #[serde(default)]
    pub allowed_roots: Vec<String>,

    #[serde(default = "default_exec_timeout_ms")]
    pub default_timeout_ms: u64,

    #[serde(default = "default_max_output_bytes")]
    pub max_output_bytes: usize,
}

impl Default for Config {
    fn default() -> Self {
        let mut targets = BTreeMap::new();
        targets.insert(
            "local".to_string(),
            TargetConfig::Local(LocalTargetConfig {
                enabled: false,
                shell: None,
                policy: PolicyConfig::default(),
            }),
        );

        Self {
            server: ServerConfig::default(),
            targets,
        }
    }
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            name: default_name(),
            version: default_version(),
            default_target: None,
            terminal_ring_buffer_bytes: default_ring_buffer_bytes(),
            runtime_dir: default_runtime_dir(),
        }
    }
}

impl Default for PolicyConfig {
    fn default() -> Self {
        Self {
            allow_exec: false,
            allow_terminal: false,
            allow_file_read: false,
            allow_file_write: false,
            allow_select_active: false,
            require_explicit_target_for_write: true,
            allowed_roots: Vec::new(),
            default_timeout_ms: default_exec_timeout_ms(),
            max_output_bytes: default_max_output_bytes(),
        }
    }
}

impl Config {
    pub fn load(path: Option<PathBuf>) -> Result<Self> {
        match path {
            Some(path) => Self::load_from_path(&path),
            None => {
                let default_path = default_config_path();
                if default_path.exists() {
                    Self::load_from_path(&default_path)
                } else {
                    Ok(Config::default())
                }
            }
        }
    }

    pub fn load_from_path(path: &Path) -> Result<Self> {
        let text = fs::read_to_string(path).map_err(|err| {
            Error::Config(format!("failed to read config {}: {err}", path.display()))
        })?;
        let mut config: Config = toml::from_str(&text)?;
        config.ensure_local_target();
        Ok(config)
    }

    pub fn ensure_runtime_dir(&self) -> Result<()> {
        fs::create_dir_all(&self.server.runtime_dir).map_err(|err| {
            Error::Config(format!(
                "failed to create runtime dir {}: {err}",
                self.server.runtime_dir.display()
            ))
        })
    }

    fn ensure_local_target(&mut self) {
        self.targets.entry("local".to_string()).or_insert_with(|| {
            TargetConfig::Local(LocalTargetConfig {
                enabled: false,
                shell: None,
                policy: PolicyConfig::default(),
            })
        });
    }
}

fn default_name() -> String {
    "mcp-ssh-host".to_string()
}

fn default_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

fn default_ring_buffer_bytes() -> usize {
    512 * 1024
}

fn default_runtime_dir() -> PathBuf {
    std::env::temp_dir().join("mcp-ssh-host")
}

fn default_ssh_port() -> u16 {
    22
}

fn default_true() -> bool {
    true
}

fn default_control_persist_secs() -> u64 {
    1800
}

fn default_exec_timeout_ms() -> u64 {
    30_000
}

fn default_max_output_bytes() -> usize {
    200_000
}

pub fn default_config_path() -> PathBuf {
    if let Ok(path) = std::env::var("MCP_SSH_HOST_CONFIG") {
        return PathBuf::from(path);
    }

    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));

    home.join(".config")
        .join("mcp-ssh-host")
        .join("config.toml")
}
