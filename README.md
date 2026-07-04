# mcp-ssh-host

A Rust MVP for a **target-based MCP server** that can control the MCP host and SSH hosts through one unified tool surface.

The design follows these principles:

- one tool set for local and SSH targets;
- `target` can be explicit or omitted when an active target has been selected;
- terminal sessions are persistent and bound to their target by `terminal_id`;
- file edits use exact text replacements and optional SHA-256 compare-and-swap;
- local host control is disabled by default;
- SSH uses the system OpenSSH client in this MVP, with optional ControlMaster connection reuse.

This repository is intentionally small and readable. It is a foundation for later replacing the OpenSSH CLI adapter with a native `russh` / SFTP backend.

## Status

Implemented:

- minimal stdio MCP JSON-RPC server;
- `initialize`, `tools/list`, `tools/call`, `ping`;
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
- SSH backend via OpenSSH CLI for exec, file read/list/edit, ControlMaster connect/disconnect, and PTY terminal;
- policy checks for enable flags, file roots, write permissions, and explicit-target write requirements.

Known MVP limitations:

- the stdio active target is process/session-scoped; an HTTP daemon transport should make it per client/session;
- SSH file operations rely on `python3` on the remote host;
- `terminal_resize` currently records the request but does not yet call a low-level PTY resize API;
- this repo was generated in an environment without Rust installed, so run `cargo fmt`, `cargo test`, and `cargo clippy` locally before production use.

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
src/state.rs     config, active target, terminal registry
src/target.rs    TargetId and target source model
src/policy.rs    allow/deny checks
src/exec.rs      non-interactive command execution
src/fs.rs        file read/list/edit dispatch
src/edit.rs      text replacement, sha256, unified diff
src/terminal.rs  persistent PTY sessions and ring buffer
src/ssh.rs       OpenSSH CLI backend
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
