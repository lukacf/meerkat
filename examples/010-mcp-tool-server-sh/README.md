# 010 — MCP Tool Server Integration (Shell)

Run a real stdio MCP server, register it with `rkat`, inspect the generated
project-scoped config, and watch a live agent prompt use the exposed tools.

This example is deliberately self-contained: the MCP server lives in this
directory as a tiny Python script, so you can read both sides of the
integration in a few minutes.

## What This Example Teaches

- How `rkat mcp add` writes a project-scoped `.rkat/mcp.toml`
- How stdio MCP servers are registered and discovered
- How `rkat mcp list` / `get` help you debug MCP wiring
- How `--wait-for-mcp` makes the first prompt block until tools are ready
- What an agent prompt looks like when it relies on MCP tool output rather than memory

## Demo Scenario

The included MCP server, `demo_mcp_server.py`, exposes two realistic incident
operations tools:

- `incident_digest(service)` — severity, owner, customer impact, rollback command
- `release_readiness(service)` — whether the current rollout should continue or hold

The shell script registers that server under the name `incident-kit`, then runs
an on-call coordination prompt that must quote fields returned by those tools.

## Prerequisites

```bash
export ANTHROPIC_API_KEY=sk-...
cargo build -p meerkat-cli
```

If `rkat` is not on your `PATH`, the script automatically falls back to
`../../target/debug/rkat` or `../../target/release/rkat`.

## Run

```bash
./setup.sh
```

## What The Script Actually Does

1. Creates isolated CLI roots under `.work/`
2. Registers a real project-scoped stdio MCP server
3. Prints the generated `.work/project/.rkat/mcp.toml`
4. Verifies the server with `rkat mcp list` and `rkat mcp get`
5. Runs a live `rkat run --wait-for-mcp --verbose ...` prompt that should call the MCP tools
6. Prints the cleanup command

Because all roots are redirected into `.work/`, this example does not touch
your real user-level MCP config or the repo's top-level `.rkat/` state.

## Why This Example Is Useful

Most MCP demos stop at "here is the command to register a server." This one
shows the full loop:

- authoring a tiny MCP server,
- registering it,
- inspecting the resulting config,
- and proving the agent can use the tools in a real task.

That makes it a much better starting point for people building internal
incident, deployment, or ops integrations.
