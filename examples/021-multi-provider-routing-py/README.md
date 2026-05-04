# 021 — Multi-Provider Routing (Python SDK)

Route to different LLM providers per-session. Compare Anthropic, OpenAI,
and Gemini responses. Use provider-specific parameters.

## Concepts
- Provider auto-detection from model name
- Provider-specific parameters (thinking, reasoning effort)
- Cost optimization by routing to cheaper models
- Fallback chains for resilience

## Supported Providers
| Provider | Models | Env Var |
|----------|--------|---------|
| Anthropic | `claude-opus-4-7`, `claude-sonnet-4-6` | `ANTHROPIC_API_KEY` |
| OpenAI | `gpt-5.5` | `OPENAI_API_KEY` |
| Gemini | `gemini-3-flash-preview`, `gemini-3.1-pro-preview` | `GEMINI_API_KEY` |

## Run
```bash
# From the repository root, first build/install the local Python SDK runtime:
# python3 -m venv .venv && . .venv/bin/activate
# python -m pip install --upgrade pip
# python -m pip install -e sdks/python
# ./scripts/repo-cargo build -p meerkat-rpc --bin rkat-rpc
# export MEERKAT_BIN_PATH="$(./scripts/repo-cargo --print-env | sed -n 's/^CARGO_TARGET_DIR=//p')/debug/rkat-rpc"
ANTHROPIC_API_KEY=sk-... OPENAI_API_KEY=sk-... python3 main.py
```
