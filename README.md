# mcp-ssh-host

A Rust MVP for a **target-based MCP server** that can control the MCP host and SSH hosts through one unified tool surface.

The design follows these principles:

- one tool set for local and SSH targets;
- `target` can be explicit or omitted when an active target has been selected;
- terminal sessions are persistent and bound to their target by `terminal_id`;
- file edits use exact text replacements and optional SHA-256 compare-and-swap;
- local host control is disabled by default;
- SSH uses persistent per-target OpenSSH worker processes for exec and file operations.

This repository is intentionally small and readable. It is a foundation for later replacing the OpenSSH CLI adapter with a native `russh` / SFTP backend.

## Status

Implemented:

- minimal stdio MCP JSON-RPC server;
- optional HTTP JSON-RPC server;
- optional HTTP bearer token authentication;
- optional OAuth 2.1-style HTTP authentication for remote MCP / ChatGPT Apps,
  including protected-resource metadata, authorization-server metadata,
  dynamic client registration, authorization-code + PKCE, and opaque bearer
  tokens;
- `initialize`, `tools/list`, `tools/call`, `ping`;
- JSON Schemas for every tool input and output, with successful tool results also returned as `structuredContent`;
- target registry with `local` and `ssh:<profile>` ids;
- session-scoped active target stickiness;
- unified tools:
  - `server_info`
  - `target_list`
  - `target_current`
  - `target_select`
  - `target_connect`
  - `target_disconnect`
  - `exec`
  - `file_read`
  - `file_list`
  - `file_edit`
  - `terminal_open`
  - `terminal_send`
  - `terminal_read`
  - `terminal_resize`
  - `terminal_close`
- local backend for exec, file read/list/edit, and PTY terminal;
- SSH backend via OpenSSH CLI for exec, file read/list/edit, persistent worker connect/disconnect, and PTY terminal;
- policy checks for enable flags, file roots, write permissions, and explicit-target write requirements.

Known MVP limitations:

- active target state is process-scoped, including HTTP mode; a later daemon transport should make it per client/session;
- SSH exec and file operations run through one persistent OpenSSH worker per target; file metadata relies on remote `stat`;
- embedded OAuth state is in-memory and intended for development/testing, not as
  a replacement for a production identity provider;
- `terminal_resize` currently records the request but does not yet call a low-level PTY resize API;
- run `cargo fmt`, `cargo test`, and `cargo clippy` before production use.

## Tool model

Every operation accepts an optional `target` unless the operation is already bound to a session object.

Explicit target:

```json
{
  "target": "ssh:dev",
  "command": "uname -a"
}
```

Sticky target:

```json
{
  "target": "ssh:dev"
}
```

Then later:

```json
{
  "command": "pwd"
}
```

Terminal sessions are bound at open time:

```json
{
  "target": "ssh:dev",
  "cwd": "/home/ubuntu/app"
}
```

Subsequent terminal calls use only `terminal_id`:

```json
{
  "terminal_id": "term_1",
  "input": "ls -la\n"
}
```

All results include the resolved target when target resolution matters.

## Configuration

Copy the example config:

```bash
mkdir -p ~/.config/mcp-ssh-host
cp examples/config.toml ~/.config/mcp-ssh-host/config.toml
```

Edit it before use. Local host access is disabled by default.

Example target ids:

- `local`
- `ssh:dev`
- `ssh:prod`

The config key for `ssh:dev` is `[targets.dev]`.

## Running

```bash
cargo run -- --config examples/config.toml
```

Or rely on:

```bash
MCP_SSH_HOST_CONFIG=/path/to/config.toml cargo run
```

The default transport is stdio. To run an HTTP server instead:

```bash
cargo run -- --config examples/config.toml --http 127.0.0.1:8765
```

HTTP endpoints:

```text
GET  /health
POST /mcp
POST /
```

HTTP bearer authentication is optional. Set either `server.http_bearer_token` in the config file or the `MCP_SSH_HOST_HTTP_TOKEN` environment variable. When configured, every non-`OPTIONS` HTTP request must include:

```text
Authorization: Bearer <token>
```

Bind HTTP to `127.0.0.1` unless bearer or OAuth auth is configured, the surrounding network is trusted, and the target policies are locked down.

### OAuth for remote MCP / ChatGPT Apps

To make the HTTP endpoint discoverable as an authenticated remote MCP server,
enable OAuth and publish the service behind an HTTPS origin:

```toml
[server]
oauth_enabled = true
public_base_url = "https://mcp.example.com"
oauth_scopes = ["mcp:tools"]
oauth_authorization_password = "change-me"
oauth_allow_dynamic_client_registration = true
```

The same values can be supplied without editing the config:

```bash
MCP_SSH_HOST_OAUTH=1 \
MCP_SSH_HOST_PUBLIC_BASE_URL=https://mcp.example.com \
MCP_SSH_HOST_OAUTH_PASSWORD='change-me' \
cargo run -- --config examples/config.toml --http 127.0.0.1:8765
```

OAuth adds these public endpoints:

```text
GET  /.well-known/oauth-protected-resource
GET  /.well-known/oauth-authorization-server
GET  /oauth/authorize
POST /oauth/authorize
POST /oauth/token
POST /oauth/register
```

The embedded authorization server is intentionally small: it supports dynamic
client registration, public-client `authorization_code` with PKCE `S256`, and
in-memory opaque access tokens. That is enough to exercise ChatGPT/App OAuth
linking during development. For a production app, put the MCP server behind
HTTPS, set `public_base_url` to that exact origin, and prefer an established
identity provider for durable users, token signing, revocation, and policy.

When OAuth is enabled, `tools/list` includes per-tool `securitySchemes` and the
Apps SDK compatibility mirror `_meta.securitySchemes`. Unauthenticated MCP
requests receive `401 Unauthorized` with a `WWW-Authenticate` challenge pointing
to `/.well-known/oauth-protected-resource`.

## Manual JSON-RPC smoke test

After building:

```bash
printf '%s\n' \
'{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"manual","version":"0"}}}' \
'{"jsonrpc":"2.0","method":"notifications/initialized"}' \
'{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}' \
| cargo run -- --config examples/config.toml
```

A tool call looks like:

```json
{
  "jsonrpc": "2.0",
  "id": 3,
  "method": "tools/call",
  "params": {
    "name": "target_list",
    "arguments": {}
  }
}
```

## Manual HTTP smoke test

Start the server:

```bash
TOKEN='change-me'
MCP_SSH_HOST_HTTP_TOKEN="$TOKEN" cargo run -- --config examples/config.toml --http 127.0.0.1:8765
```

Then call the MCP endpoint:

```bash
curl -s http://127.0.0.1:8765/health \
  -H "Authorization: Bearer $TOKEN"

curl -s http://127.0.0.1:8765/mcp \
  -H 'Content-Type: application/json' \
  -H "Authorization: Bearer $TOKEN" \
  -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"manual","version":"0"}}}'

curl -s http://127.0.0.1:8765/mcp \
  -H 'Content-Type: application/json' \
  -H "Authorization: Bearer $TOKEN" \
  -d '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"target_list","arguments":{}}}'
```

## Safety defaults

The default policy is deny-by-default:

- disabled targets cannot be used;
- local target is disabled unless explicitly enabled;
- exec, terminal, file read, and file write are separately gated;
- file operations require `allowed_roots`;
- `file_edit` requires an explicit target by default;
- OpenSSH host key verification is left to OpenSSH defaults and your SSH config.

## Architecture

```text
MCP stdio server
  -> tools/call router
    -> AppState
      -> Target resolver
      -> Policy checks
      -> Unified operation modules
        -> Local adapter
        -> SSH adapter
      -> Terminal registry
      -> Ring buffer
```

Important modules:

```text
src/mcp.rs       minimal MCP stdio JSON-RPC transport
src/tools.rs     tool list and dispatch
src/state.rs     config, active target, SSH and terminal registries
src/target.rs    TargetId and target source model
src/policy.rs    allow/deny checks
src/exec.rs      non-interactive command execution
src/fs.rs        file read/list/edit dispatch
src/edit.rs      text replacement, sha256, unified diff
src/terminal.rs  persistent PTY sessions and ring buffer
src/ssh.rs       persistent OpenSSH CLI worker backend
```

## Roadmap

Suggested next steps:

1. Replace the OpenSSH CLI adapter with native `russh` sessions and SFTP.
2. Add streamable HTTP daemon mode and make active target per connected client.
3. Wire actual PTY resize.
4. Add audit logging.
5. Add command deny/allow patterns and optional human confirmation for risky actions.
6. Add `file_write`, `file_search`, upload/download, and tmux-backed persistent SSH terminals.

## Pushing to GitHub

With GitHub CLI:

```bash
gh repo create mcp-ssh-host --private --source=. --remote=origin --push
```

Without GitHub CLI:

```bash
git remote add origin git@github.com:<your-user-or-org>/mcp-ssh-host.git
git push -u origin main
```
