---
name: MCP Server Setup
description: How to configure MCP servers in .rkat/mcp.toml
---

# MCP Server Setup

## Configuration File

MCP servers are configured in `.rkat/mcp.toml` (project) or `~/.config/rkat/mcp.toml` (user).

## Adding Servers

### Stdio Transport
```
rkat mcp add <name> -- <command> [args...]
```

### HTTP/SSE Transport
```
rkat mcp add <name> --url <url>
```

## Configuration Format

```toml
[servers.<name>]
command = "path/to/server"
args = ["--flag", "value"]
env = { KEY = "value" }
```

## Environment Variables

- Environment variables in server config are expanded at launch
- Use `env` table for server-specific environment
- System environment variables are inherited

## Troubleshooting

- Use `rkat mcp list` to verify server registration
- Check server command exists and is executable
- Verify network connectivity for HTTP/SSE transport
- Check logs for connection errors
