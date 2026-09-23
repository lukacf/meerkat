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
export ANTHROPIC_API_KEY=sk-...  # Required by the script's guard
export OPENAI_API_KEY=sk-...     # Fresh unpinned run: needs gpt-6-astra access
./scripts/repo-cargo build -p rkat --bin rkat
```

The script checks `ANTHROPIC_API_KEY`, but does not pass `--model` or
`--provider` to select Anthropic. A fresh, unpinned run selects `gpt-6-astra`
and therefore needs usable OpenAI credentials and access to that model.
Explicit model/provider configuration can change the live run's credential
requirement, but it must apply within the script's redirected `.work/` roots;
do not rely on a model pin in your ordinary realm. Setting an API key alone
does not select its provider, and the Anthropic-key guard still applies.

If `rkat` is not on your `PATH`, the script automatically falls back to
repo-local binaries built by `./scripts/repo-cargo`.
Fresh turns select `claude-sonnet-4-6` explicitly, matching the Anthropic key.
`RKAT` can name an executable on `PATH`, or a literal absolute/relative path
(including spaces). Relative overrides resolve from your invocation directory,
not from the example's internal working directory. Do not include shell flags
in `RKAT`.

## Run

```bash
./examples/010-mcp-tool-server-sh/setup.sh
```

## What The Script Actually Does

1. Creates isolated CLI roots under `.work/`
2. Registers a real project-scoped stdio MCP server
3. Prints the generated `.work/project/.rkat/mcp.toml`
4. Verifies the server with `rkat mcp list` and `rkat mcp get`
5. Runs a live `rkat run --model claude-sonnet-4-6 --wait-for-mcp --verbose ...` prompt that should call the MCP tools
6. Prints a shell-escaped cleanup command and removes this run's registration on
   exit, including on a failed agent turn

Because all roots are redirected into `.work/`, this example does not touch
your real user-level MCP config or the repo's top-level `.rkat/` state.
Only a registration successfully created by this invocation is removed; existing
entries are never overwritten or deleted. Session state is kept for inspection,
and the script can be retried after a provider failure. If cleanup itself fails,
run the printed command before retrying.

`--wait-for-mcp` waits for the server handshake, not necessarily for inline tool
advertisement. With the deferred catalog, the model first uses
`tool_catalog_search` and `tool_catalog_load`; the actual MCP tools become
available on the following boundary.

## Offline Regression Tests

```bash
python3 examples/010-mcp-tool-server-sh/test_examples.py
```

These exercise malformed MCP requests followed by ping, real CLI registration
and cleanup in scratch copies, executable path handling, and bounded synthetic
Anthropic responses. The fixture follows the advertised inline or deferred
catalog and requires both real MCP results; it never substitutes a fabricated
incident answer or disables deferred discovery. These are not live-provider
tests.

## Why This Example Is Useful

Most MCP demos stop at "here is the command to register a server." This one
shows the full loop:

- authoring a tiny MCP server,
- registering it,
- inspecting the resulting config,
- and proving the agent can use the tools in a real task.

That makes it a much better starting point for people building internal
incident, deployment, or ops integrations.
