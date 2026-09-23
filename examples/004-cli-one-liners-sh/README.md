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
export OPENAI_API_KEY=sk-...
./scripts/repo-cargo build -p rkat --bin rkat
```

The script does not pass `--model` or `--provider`. With a fresh, unpinned
configuration it selects `gpt-6-astra`, so the OpenAI key must have access to
that model. Explicit model/provider configuration can change the credentials
needed; merely setting an API key does not select its provider. The script
also starts an `--isolated` realm, so a model pinned only in the ordinary realm
is not sufficient for every run.

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
