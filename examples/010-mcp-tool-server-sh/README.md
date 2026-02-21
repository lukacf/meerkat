# 010 — MCP Tool Server Integration (Shell)

Connect external tool servers to Meerkat via the Model Context Protocol (MCP).
MCP lets you give agents access to databases, APIs, filesystems, and any other
tool server — without writing Rust code.

## Concepts
- `rkat mcp add` — register a tool server (stdio or HTTP)
- `rkat mcp list` — show registered servers
- `rkat mcp remove` — unregister a server
- `.rkat/mcp.toml` — MCP configuration file
- Tool discovery — agents automatically see MCP tools

## Architecture
```
Agent ←→ Meerkat MCP Router ←→ MCP Server (stdio/HTTP)
                                    ↓
                              External Tools
                              (DB, API, FS, ...)
```

## How MCP Works
1. You register an MCP server (a process or HTTP endpoint)
2. Meerkat discovers the server's tools via the MCP protocol
3. Those tools appear alongside built-in tools in the agent's tool list
4. When the agent calls an MCP tool, Meerkat routes it to the server

## Run
```bash
chmod +x setup.sh && ./setup.sh
```
