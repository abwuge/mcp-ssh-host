mod config;
mod edit;
mod error;
mod exec;
mod fs;
mod http;
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

struct Args {
    config_path: Option<PathBuf>,
    http_addr: Option<String>,
}

fn main() {
    if let Err(err) = run() {
        eprintln!("mcp-ssh-host failed: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args = parse_args();
    let config = Config::load(args.config_path)?;
    let state = Arc::new(AppState::new(config)?);

    if let Some(addr) = args.http_addr {
        http::serve_http(state, &addr)
    } else {
        mcp::serve_stdio(state)
    }
}

fn parse_args() -> Args {
    let mut args = std::env::args().skip(1);
    let mut parsed = Args {
        config_path: None,
        http_addr: None,
    };

    while let Some(arg) = args.next() {
        if let Some(value) = arg.strip_prefix("--config=") {
            parsed.config_path = Some(PathBuf::from(value));
            continue;
        }
        if let Some(value) = arg
            .strip_prefix("--http=")
            .or_else(|| arg.strip_prefix("--http-addr="))
        {
            parsed.http_addr = Some(value.to_string());
            continue;
        }

        match arg.as_str() {
            "--config" | "-c" => {
                parsed.config_path = Some(PathBuf::from(next_arg(&mut args, &arg)));
            }
            "--http" | "--http-addr" => {
                parsed.http_addr = Some(next_arg(&mut args, &arg));
            }
            "--help" | "-h" => {
                println!(
                    "Usage: mcp-ssh-host [--config path/to/config.toml] [--http 127.0.0.1:8765]\n\nFlags also accept --config=PATH and --http=ADDR.\nIf --http is omitted, the server uses stdio transport.\nIf --config is omitted, MCP_SSH_HOST_CONFIG or ~/.config/mcp-ssh-host/config.toml is used when present."
                );
                std::process::exit(0);
            }
            _ => {}
        }
    }

    parsed
}

fn next_arg(args: &mut impl Iterator<Item = String>, flag: &str) -> String {
    args.next().unwrap_or_else(|| {
        eprintln!("missing value for {flag}");
        std::process::exit(2);
    })
}
