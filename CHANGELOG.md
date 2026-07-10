# Changelog

## 0.1.0

- Initial target-based MCP server MVP.
- Unified tools across local and SSH targets.
- Session-scoped active target stickiness.
- Local and OpenSSH CLI backends.
- Optional HTTP transport with `/health` and `/mcp`.
- GPTs Actions REST facade with a public OpenAPI 3.1 Schema at
  `/openapi.json`, explicit-target operations, response bounds, and
  consequential-operation markers.
- Added `outputSchema` declarations for every tool and `structuredContent` in successful tool results.
- Made command results sparse by omitting echoed inputs, empty streams, and inactive status flags.
