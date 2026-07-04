use crate::{
    error::{Error, Result},
    mcp,
    state::AppState,
};
use serde_json::json;
use std::{sync::Arc, thread};
use tiny_http::{Header, Method, Request, Response, Server, StatusCode};

pub fn serve_http(state: Arc<AppState>, addr: &str) -> Result<()> {
    let server = Server::http(addr)
        .map_err(|err| Error::Config(format!("failed to bind HTTP server on {addr}: {err}")))?;

    eprintln!("mcp-ssh-host listening on http://{addr}/mcp");
    for request in server.incoming_requests() {
        let state = Arc::clone(&state);
        thread::spawn(move || {
            if let Err(err) = handle_request(state, request) {
                eprintln!("mcp-ssh-host HTTP request failed: {err}");
            }
        });
    }

    Ok(())
}

fn handle_request(state: Arc<AppState>, mut request: Request) -> Result<()> {
    let method = request.method().clone();
    let path = request.url().split('?').next().unwrap_or("/").to_string();

    match (method, path.as_str()) {
        (Method::Get, "/") => respond_json(
            request,
            200,
            json!({
                "name": state.config.server.name.clone(),
                "version": state.config.server.version.clone(),
                "endpoints": {
                    "health": "GET /health",
                    "mcp": "POST /mcp"
                }
            }),
        ),
        (Method::Get, "/health") => respond_json(
            request,
            200,
            json!({
                "ok": true,
                "name": state.config.server.name.clone(),
                "version": state.config.server.version.clone(),
            }),
        ),
        (Method::Post, "/" | "/mcp") => {
            let mut body = Vec::new();
            request.as_reader().read_to_end(&mut body)?;
            match mcp::handle_json_bytes(state, &body)? {
                Some(response) => respond_bytes(request, 200, response),
                None => respond_empty(request, 202),
            }
        }
        (Method::Options, "/" | "/mcp") => respond_empty_with_allow(request, 204),
        _ => respond_json(
            request,
            404,
            json!({
                "error": "not found",
                "endpoints": ["GET /health", "POST /mcp"]
            }),
        ),
    }
}

fn respond_json(request: Request, status: u16, value: serde_json::Value) -> Result<()> {
    respond_bytes(request, status, serde_json::to_vec(&value)?)
}

fn respond_bytes(request: Request, status: u16, body: Vec<u8>) -> Result<()> {
    let mut response = Response::from_data(body).with_status_code(StatusCode(status));
    response.add_header(header("Content-Type", "application/json"));
    request.respond(response).map_err(Error::Io)
}

fn respond_empty(request: Request, status: u16) -> Result<()> {
    request
        .respond(Response::empty(StatusCode(status)))
        .map_err(Error::Io)
}

fn respond_empty_with_allow(request: Request, status: u16) -> Result<()> {
    let mut response = Response::empty(StatusCode(status));
    response.add_header(header("Allow", "POST, OPTIONS"));
    request.respond(response).map_err(Error::Io)
}

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).expect("static header is valid")
}
