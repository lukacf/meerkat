# 021 — Multi-Provider Routing (Python SDK)

Route to different LLM providers per-session. Compare Anthropic, OpenAI,
and Gemini responses. Set one reasoning-effort level for all of them.

## Concepts
- Provider auto-detection from model name
- One provider-neutral `reasoning_effort` level, sent to every configured
  provider, with the report of what each request carried
- What a refusal looks like: a level a model does not accept is refused with
  the levels it does accept, never lowered
- A provider-specific parameter: the Anthropic thinking budget, as a request
  of its own (a budget and a level on one request are refused)
- Per-session model routing across configured providers

The runnable calls compare configured providers, set the level and use one
typed provider parameter. The final printed routing guide discusses cost and
fallback patterns; it does not execute an automatic fallback chain.

The reasoning-effort part needs Meerkat 0.9.15 or later, in the SDK and in
the runtime it talks to: an older runtime makes the SDK raise
`CapabilityUnavailableError` before it sends anything.

## Supported Providers
| Provider | Models | Env Var |
|----------|--------|---------|
| Anthropic | `claude-opus-4-8`, `claude-sonnet-4-6` | `ANTHROPIC_API_KEY` |
| OpenAI | `gpt-5.5` | `OPENAI_API_KEY` |
| Gemini | `gemini-3.5-flash`, `gemini-3.1-pro-preview` | `GEMINI_API_KEY` |

## Run

With the released SDK, from the repository root (see
[002](../002-hello-meerkat-py/) for what it downloads and verifies on first
use):

```bash
python3 -m venv .venv
. .venv/bin/activate
python -m pip install meerkat-sdk
ANTHROPIC_API_KEY=sk-... OPENAI_API_KEY=sk-... \
  python examples/021-multi-provider-routing-py/main.py
```

To run against this checkout instead, build the SDK and the runtime first; a
source install of the SDK refuses to download a runtime, so point it at the
one you build:

```bash
python3 -m venv .venv && . .venv/bin/activate
python -m pip install --upgrade pip
python -m pip install -e sdks/python
./scripts/repo-cargo build -p meerkat-rpc --bin rkat-rpc
export MEERKAT_BIN_PATH="$(./scripts/repo-cargo --print-env | sed -n 's/^CARGO_TARGET_DIR=//p')/debug/rkat-rpc"
ANTHROPIC_API_KEY=sk-... OPENAI_API_KEY=sk-... \
  python examples/021-multi-provider-routing-py/main.py
```
