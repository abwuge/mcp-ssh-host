mod config;
mod edit;
mod error;
mod exec;
mod fs;
mod mcp;
mod policy;
mod ssh;
mod state;
mod target;
mod terminal;
mod tools;
mod util;

use crate::{config::Config, error::Result, state::AppState};
use std::{path::PathBuf, sync::Arc};

fn main() {
    if let Err(err) = run() {
        eprintln!("mcp-ssh-host failed: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let config_path = parse_config_path();
    let config = Config::load(config_path)?;
    let state = Arc::new(AppState::new(config)?);
    mcp::serve_stdio(state)
}

fn parse_config_path() -> Option<PathBuf> {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--config" | "-c" => return args.next().map(PathBuf::from),
            "--help" | "-h" => {
                println!("Usage: mcp-ssh-host [--config path/to/config.toml]\n\nIf --config is omitted, MCP_SSH_HOST_CONFIG or ~/.config/mcp-ssh-host/config.toml is used when present.");
                std::process::exit(0);
            }
            _ => {}
        }
    }
    None
}
