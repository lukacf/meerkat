# 021 — Multi-Provider Routing (Python SDK)

Route to different LLM providers per-session. Compare Anthropic, OpenAI,
and Gemini responses. Set one reasoning-effort level for all of them.

## Concepts
- Provider auto-detection from model name
- One provider-neutral `reasoning_effort` level, sent to every configured
  provider, with the report of what each request carried
- What a refusal looks like: a level a model does not accept is refused,
  never lowered, and the refusal names the levels the model accepts when a
  source states them
- A provider-specific parameter under the typed `provider_tag`: Anthropic
  adaptive thinking, as a request of its own
- Per-session model routing across configured providers

The runnable calls compare configured providers, set the level and use one
typed provider parameter. The final printed routing guide discusses cost and
fallback patterns; it does not execute an automatic fallback chain.

The reasoning-effort part needs Meerkat 0.9.15 or later, in the SDK and in
the runtime it talks to: an older runtime makes the SDK raise
`CapabilityUnavailableError` before it sends anything.

The OpenAI path uses `gpt-6.2-astra`, added to the runtime catalog in 0.9.17.

## Supported Providers
| Provider | Models | Env Var |
|----------|--------|---------|
| Anthropic | `claude-opus-5-5`, `claude-sonnet-5-5` | `ANTHROPIC_API_KEY` |
| OpenAI | `gpt-6.2-astra` | `OPENAI_API_KEY` |
| Gemini | `gemini-4-hydrogen`, `gemini-4-argon` | `GEMINI_API_KEY` |

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
