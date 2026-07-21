# mcp-target-ops

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
- GPTs Actions REST facade with a public, dynamically generated OpenAPI 3.1
  schema at `/openapi.json`;
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
- GPTs Actions API-key authentication represents one shared service identity.
  Keep the GPT private unless you add per-user identity, isolation, rate
  limiting, and audit logging;
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
mkdir -p ~/.config/mcp-target-ops
cp examples/config.toml ~/.config/mcp-target-ops/config.toml
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
MCP_TARGET_OPS_CONFIG=/path/to/config.toml cargo run
```

The default transport is stdio. To run an HTTP server instead:

```bash
cargo run -- --config examples/config.toml --http 127.0.0.1:8765
```

HTTP endpoints:

```text
GET  /health                 public health check
GET  /openapi.json           public GPTs Actions Schema
POST /mcp                    MCP JSON-RPC
POST /                       MCP JSON-RPC compatibility route
GET  /actions/v1/targets     GPTs target discovery
POST /actions/v1/*           GPTs Actions REST facade
```

HTTP bearer authentication is optional. Set either `server.http_bearer_token` in the config file or the `MCP_TARGET_OPS_HTTP_TOKEN` environment variable. When configured, protected MCP and GPTs Action requests must include:

```text
Authorization: Bearer <token>
```

The root document, health check, OpenAPI Schema, and OAuth discovery/flow
endpoints remain public. Bearer authentication protects `/mcp` and
`/actions/v1/*`.

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
MCP_TARGET_OPS_OAUTH=1 \
MCP_TARGET_OPS_PUBLIC_BASE_URL=https://mcp.example.com \
MCP_TARGET_OPS_OAUTH_PASSWORD='change-me' \
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

### GPTs Actions

GPTs Actions use an OpenAPI-described REST API; they do not call the MCP
JSON-RPC endpoint directly. In HTTP mode this server exposes a public, generated
OpenAPI 3.1 document:

```text
https://ssh.example.com/openapi.json
```

See OpenAI's [Configuring actions in GPTs](https://help.openai.com/articles/9442513)
and [GPT Actions production notes](https://developers.openai.com/api/docs/actions/production).

Set `public_base_url` to the externally reachable HTTPS origin so the
document's `servers[0].url` is stable:

```bash
TOKEN='replace-with-a-long-random-value'
MCP_TARGET_OPS_HTTP_TOKEN="$TOKEN" \
MCP_TARGET_OPS_PUBLIC_BASE_URL=https://ssh.example.com \
cargo run --release -- --config examples/config.toml --http 0.0.0.0:8765
```

Terminate TLS in a reverse proxy or hosting platform and route port 443 to the
server. GPT Actions require a valid public TLS certificate and TLS 1.2 or later;
`localhost` is useful only for local smoke tests.

In the GPT editor:

1. Open **Configure → Actions → Create new action**.
2. Choose **API key**, select **Bearer**, and save the same token used by
   `MCP_TARGET_OPS_HTTP_TOKEN`.
3. Select **Import from URL** and enter
   `https://ssh.example.com/openapi.json`.
4. Test `listTargets`, then a read-only action, in Preview.
5. Add a valid Privacy Policy URL before sharing a GPT by link or publishing it.

A GPT can use apps or actions, but not both at the same time. The generated
Schema deliberately exposes only this bounded, explicit-target surface:

| Method and path | `operationId` | Behavior |
| --- | --- | --- |
| `GET /actions/v1/targets` | `listTargets` | List target IDs and policy capabilities |
| `POST /actions/v1/commands/execute` | `executeCommand` | Run a command; always consequential |
| `POST /actions/v1/files/read` | `readFile` | Read bounded file content and SHA-256 |
| `POST /actions/v1/directories/list` | `listDirectory` | List up to 100 entries |
| `POST /actions/v1/files/edits/preview` | `previewFileEdits` | Generate a diff without writing |
| `POST /actions/v1/files/edits/apply` | `applyFileEdits` | Apply a CAS-guarded edit; always consequential |

Every target operation requires an explicit `target`. The Actions facade caps
remote operation time at 30 seconds, request bodies at 64 KiB, command output
at 24 KiB per stream, file content at 32 KiB, edit diffs at 32K characters,
and complete responses at 90K characters. These limits stay below the current GPT Actions
45-second and 100,000-character limits.

For a first deployment, use a private GPT, a dedicated low-privilege SSH
account, narrow `allowed_roots`, and read-only target policy where possible.
The embedded MCP OAuth server is not a production GPTs OAuth provider: GPTs
OAuth expects a configured client ID and client secret and supports per-user
tokens. Use an external identity provider before offering personalized or
multi-user access.

Local Schema smoke test:

```bash
curl -s http://127.0.0.1:8765/openapi.json \
  | jq '{openapi, servers, operation_ids: [.paths[][] | .operationId]}'
```

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
MCP_TARGET_OPS_HTTP_TOKEN="$TOKEN" cargo run -- --config examples/config.toml --http 127.0.0.1:8765
```

Then call the public and protected endpoints:

```bash
curl -s http://127.0.0.1:8765/health \
  -H "Authorization: Bearer $TOKEN"

curl -s http://127.0.0.1:8765/openapi.json

curl -s http://127.0.0.1:8765/actions/v1/targets \
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
MCP stdio / HTTP JSON-RPC
  -> MCP tools/call router
GPTs HTTPS
  -> OpenAPI Schema + bounded Actions REST facade
Both
  -> shared tool dispatch
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
src/protocol/mcp.rs       MCP JSON-RPC protocol
src/protocol/http.rs      HTTP, bearer auth, and OAuth routes
src/protocol/gpts.rs      GPTs Actions REST facade and OpenAPI Schema
src/tooling/tools.rs      shared tool catalog and dispatch
src/core/state.rs         config, active target, SSH and terminal registries
src/core/target.rs        TargetId and target source model
src/core/policy.rs        allow/deny checks
src/tooling/exec.rs       non-interactive command execution
src/tooling/fs.rs         file read/list/edit dispatch
src/tooling/edit.rs       text replacement, SHA-256, unified diff
src/tooling/terminal.rs   persistent PTY sessions and ring buffer
src/transport/ssh.rs      persistent OpenSSH CLI worker backend
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
gh repo create mcp-target-ops --private --source=. --remote=origin --push
```

Without GitHub CLI:

```bash
git remote add origin git@github.com:<your-user-or-org>/mcp-target-ops.git
git push -u origin main
```
