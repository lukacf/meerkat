#!/usr/bin/env bash
# 010 — MCP Tool Server Integration
#
# This script shows how to register external tool servers with Meerkat
# using the MCP (Model Context Protocol).
#
# Prerequisites:
#   export ANTHROPIC_API_KEY=sk-...
#   cargo build -p meerkat-cli  # produces rkat
#   pip install mcp-server-fetch  # example MCP server (optional)

set -euo pipefail

RKAT="${RKAT:-rkat}"

echo "=== MCP Tool Server Management ==="
echo ""

# ── 1. Register a stdio-based MCP server ──
echo "--- 1. Adding a stdio MCP server ---"
echo "Command: rkat mcp add fetch -- python -m mcp_server_fetch"
# Uncomment to actually run:
# $RKAT mcp add fetch -- python -m mcp_server_fetch
echo "(Skipped — uncomment if mcp-server-fetch is installed)"

# ── 2. Register an HTTP MCP server ──
echo ""
echo "--- 2. Adding an HTTP MCP server ---"
echo "Command: rkat mcp add my-api --url http://localhost:8080/mcp"
# Uncomment to actually run:
# $RKAT mcp add my-api --url http://localhost:8080/mcp
echo "(Skipped — uncomment if you have a local MCP server)"

# ── 3. List registered servers ──
echo ""
echo "--- 3. Listing MCP servers ---"
$RKAT mcp list 2>/dev/null || echo "(No servers registered yet)"

# ── 4. Show the config file format ──
echo ""
echo "--- 4. MCP config file format (.rkat/mcp.toml) ---"
cat << 'TOML'
# Project-level MCP configuration
# File: .rkat/mcp.toml

[[servers]]
name = "filesystem"
command = "mcp-server-filesystem"
args = ["--root", "/home/user/projects"]

[[servers]]
name = "database"
command = "mcp-server-sqlite"
args = ["--db", "./data/app.db"]

[[servers]]
name = "web-search"
url = "http://localhost:9000/mcp"

[[servers]]
name = "custom-api"
command = "python"
args = ["tools/my_mcp_server.py"]
env = { API_KEY = "your-key" }
TOML

# ── 5. Run an agent with MCP tools ──
echo ""
echo "--- 5. Using MCP tools with an agent ---"
echo "Once servers are registered, tools are automatically available:"
echo ""
echo "  rkat run 'List the files in my home directory'"
echo "  rkat run 'Query the database for recent orders'"
echo "  rkat run 'Search the web for Meerkat documentation'"
echo ""
echo "The agent sees MCP tools alongside built-in tools and can call them naturally."

# ── 6. Remove a server ──
echo ""
echo "--- 6. Removing an MCP server ---"
echo "Command: rkat mcp remove fetch"
# $RKAT mcp remove fetch
echo "(Skipped)"

echo ""
echo "Done! MCP servers give agents access to external tools without writing Rust code."
