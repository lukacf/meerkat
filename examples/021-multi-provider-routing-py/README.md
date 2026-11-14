# 021 — Multi-Provider Routing (Python SDK)

Route to different LLM providers per-session. Compare Anthropic, OpenAI,
and Gemini responses. Use provider-specific parameters.

## Concepts
- Provider auto-detection from model name
- Provider-specific parameters (thinking, reasoning effort)
- Per-session model routing across configured providers
- Typed Anthropic and OpenAI provider parameters

The runnable calls compare configured providers and demonstrate typed provider
parameters. The final printed routing guide discusses cost and fallback
patterns; it does not execute an automatic fallback chain.

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
