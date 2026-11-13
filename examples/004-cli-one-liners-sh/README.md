# 004 — CLI One-Liners (Shell)

Common `rkat` workflows from the command line, with no application code.

## Concepts
- `rkat run` — single-turn agent execution
- `rkat run --resume last` — multi-turn session resumption
- `rkat session list` - inspect sessions created by earlier commands
- `--isolated` / `--realm` - realm selection
- `--verbose` / `--stream` - output modes
- `rkat config` - runtime configuration

## Prerequisites
```bash
export ANTHROPIC_API_KEY=sk-...
./scripts/repo-cargo build -p rkat --bin rkat
```

Every command that starts a session passes `--model claude-sonnet-4-6` (the
`MODEL` variable set just before step 1 in `examples.sh`), so the Anthropic key is the
only credential the script needs. Step 3 resumes the session from step 2
without `--model` and keeps that session's model. To run against another
provider, change `MODEL` and export that provider's key: setting an API key
alone does not select its provider, and a `rkat run` without `--model` on a
fresh configuration selects `gpt-6-astra`, which needs an OpenAI key.

## Run
```bash
./examples/004-cli-one-liners-sh/examples.sh
```

Fresh turns explicitly select `claude-sonnet-4-6` (Anthropic), matching the
credential above. Resume keeps the session's model; it does not force a new one.
State and config roots are example-local under `.work/`; no global config is
modified. Re-running creates new example sessions and resumes the one just created.
Set `RKAT="/path with spaces/to/rkat"` to use a literal executable path (not a
shell command plus flags). Relative paths are relative to your invocation directory.

Offline regression tests (also cover 010):
```bash
python3 examples/010-mcp-tool-server-sh/test_examples.py
```
