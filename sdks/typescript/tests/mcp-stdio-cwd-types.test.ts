import type { McpAddParams, McpServerConfig, McpStdioConfig } from "../src/index.js";

// A stdio MCP server may name the absolute directory it is launched in.
const github: McpServerConfig = {
  name: "github",
  command: "/opt/github/bin/github-mcp-server",
  args: ["stdio"],
  cwd: "/opt/github",
};
const stdio: McpStdioConfig = { command: "npx", cwd: "/srv/project" };
const withoutCwd: McpStdioConfig = { command: "npx" };
const add: McpAddParams = { session_id: "s1", server_config: github };
const notAPath: McpStdioConfig = {
  command: "npx",
  // @ts-expect-error The working directory is a path string.
  cwd: 7,
};

void [github, stdio, withoutCwd, add, notAPath];
