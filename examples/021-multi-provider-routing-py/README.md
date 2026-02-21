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
| Anthropic | `claude-opus-4-6`, `claude-sonnet-4-5` | `ANTHROPIC_API_KEY` |
| OpenAI | `gpt-5.2` | `OPENAI_API_KEY` |
| Gemini | `gemini-3-flash-preview`, `gemini-3-pro-preview` | `GEMINI_API_KEY` |

## Run
```bash
ANTHROPIC_API_KEY=sk-... OPENAI_API_KEY=sk-... python main.py
```
