#!/usr/bin/env python3
"""021 — Multi-Provider Routing (Python SDK)

Use different LLM providers for different tasks. Meerkat supports Anthropic,
OpenAI, and Gemini — you can switch between them per-session.

What you'll learn:
- Routing to different providers (Anthropic, OpenAI, Gemini)
- Provider-specific parameters (thinking, reasoning effort)
- Comparing responses across providers
- Model selection strategies

Run:
    ANTHROPIC_API_KEY=sk-... OPENAI_API_KEY=sk-... python3 main.py
"""

import asyncio
import os
from meerkat import MeerkatClient


async def query_provider(client: MeerkatClient, model: str, prompt: str) -> dict:
    """Run a prompt on a specific model and return stats."""
    try:
        session = await client.create_session(prompt=prompt, model=model)
        return {
            "model": model,
            "text": session.text,
            "tokens": session.usage.input_tokens + session.usage.output_tokens,
            "error": None,
        }
    except Exception as e:
        return {
            "model": model,
            "text": None,
            "tokens": 0,
            "error": str(e),
        }


async def main():
    client = MeerkatClient()
    await client.connect()

    try:
        prompt = (
            "Explain the CAP theorem in distributed systems. "
            "Give a practical example. Be concise (under 150 words)."
        )

        # ── Route to different providers ──
        models = []

        if os.environ.get("ANTHROPIC_API_KEY"):
            models.append("claude-sonnet-4-6")
        if os.environ.get("OPENAI_API_KEY"):
            models.append("gpt-5.5")
        if os.environ.get("GEMINI_API_KEY"):
            models.append("gemini-3.5-flash")

        if not models:
            print("Set at least one API key: ANTHROPIC_API_KEY, OPENAI_API_KEY, GEMINI_API_KEY")
            return

        print(f"=== Multi-Provider Comparison ({len(models)} providers) ===\n")
        print(f"Prompt: {prompt}\n")

        for model in models:
            print(f"--- {model} ---")
            result = await query_provider(client, model, prompt)
            if result["error"]:
                print(f"  Error: {result['error']}")
            else:
                print(f"  Response: {result['text'][:200]}...")
                print(f"  Tokens: {result['tokens']}")
            print()

        # ── Provider-specific parameters ──
        print("=== Provider-Specific Parameters ===\n")

        if os.environ.get("ANTHROPIC_API_KEY"):
            print("--- Anthropic with extended thinking ---")
            session = await client.create_session(
                prompt="What is the optimal data structure for a LRU cache? Think step by step.",
                model="claude-sonnet-4-6",
                provider_params={
                    # Top-level typed knob: enables Anthropic extended thinking
                    # with the given token budget (ProviderParamsOverride).
                    "thinking_budget_tokens": 5000
                },
            )
            print(f"  Response: {session.text[:200]}...")
            print(f"  Tokens: {session.usage.input_tokens + session.usage.output_tokens}")
            print()

        if os.environ.get("OPENAI_API_KEY"):
            print("--- OpenAI with reasoning effort ---")
            session = await client.create_session(
                prompt="What is the optimal data structure for a LRU cache?",
                model="gpt-5.5",
                provider_params={
                    # OpenAI-specific knobs travel under the typed provider_tag,
                    # discriminated by the snake_case provider name "open_ai".
                    "provider_tag": {"provider": "open_ai", "reasoning_effort": "high"}
                },
            )
            print(f"  Response: {session.text[:200]}...")
            print(f"  Tokens: {session.usage.input_tokens + session.usage.output_tokens}")
            print()

        # ── Routing strategy reference ──
        print("=== Routing Strategies ===\n")
        print("""
Model selection strategies:

1. COST OPTIMIZATION
   - Simple tasks → claude-sonnet-4-6 or gemini-3.5-flash
   - Complex tasks → claude-opus-4-8 or gpt-5.5

2. CAPABILITY-BASED
   - Code generation → claude-sonnet-4-6 (strong at code)
   - Reasoning → claude-opus-4-8 with thinking, or gpt-5.5 high effort
   - Speed → gemini-3.5-flash

3. FALLBACK CHAIN
   - Try primary provider
   - On failure, fall back to secondary
   - Meerkat's retry policy handles transient failures

4. PER-AGENT ROUTING (in mobs)
   - Orchestrator: claude-opus-4-8 (complex planning)
   - Workers: claude-sonnet-4-6 (execution)
   - Validators: gemini-3.5-flash (fast checks)
""")

    finally:
        await client.close()


if __name__ == "__main__":
    asyncio.run(main())
