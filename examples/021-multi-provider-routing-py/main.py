#!/usr/bin/env python3
"""021 — Multi-Provider Routing (Python SDK)

Use different LLM providers for different tasks. Meerkat supports Anthropic,
OpenAI, and Gemini — you can switch between them per-session.

What you'll learn:
- Routing to different providers (Anthropic, OpenAI, Gemini)
- One reasoning-effort setting for every provider, and what a refusal says
- A provider-specific parameter (Anthropic thinking budget)
- Comparing responses across providers
- Model selection strategies

Needs Meerkat 0.9.15 or later for the reasoning-effort part.

Run:
    ANTHROPIC_API_KEY=sk-... OPENAI_API_KEY=sk-... python3 main.py
"""

import asyncio
import os
from meerkat import MeerkatClient, MeerkatError, reasoning_effort_refusal


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

        # ── One reasoning-effort setting for every provider ──
        print("=== Reasoning Effort: One Setting, Every Provider ===\n")

        # `reasoning_effort` is provider-neutral. Meerkat checks the level
        # against the model, writes the provider field itself and reports what
        # it sent. It never lowers a level: one the model does not accept is
        # refused before any provider request, and the refusal lists the
        # levels the model does accept.
        for model in models:
            print(f"--- {model} with reasoning_effort=high ---")
            try:
                session = await client.create_session(
                    prompt="What is the optimal data structure for a LRU cache?",
                    model=model,
                    reasoning_effort="high",
                )
            except MeerkatError as error:
                refusal = reasoning_effort_refusal(error)
                if refusal is None:
                    raise
                accepted = ", ".join(refusal.supported) if refusal.supported else "no level"
                print(f"  Refused: {refusal.reason}; this model accepts: {accepted}")
                print()
                continue
            print(f"  Response: {session.text[:200]}...")
            print(f"  Tokens: {session.usage.input_tokens + session.usage.output_tokens}")
            report = session.last_result.reasoning_effort
            for sent in report.sent if report else ():
                fields = ", ".join(sent.fields)
                print(f"  Sent to {sent.provider} as {sent.level} ({fields})")
            print()

        # ── A provider-specific parameter ──
        print("=== Provider-Specific Parameter ===\n")

        if os.environ.get("ANTHROPIC_API_KEY"):
            print("--- Anthropic with a thinking budget ---")
            session = await client.create_session(
                prompt="What is the optimal data structure for a LRU cache? Think step by step.",
                model="claude-sonnet-4-6",
                provider_params={
                    # A token budget for Anthropic extended thinking. It is a
                    # different control from the level above: a request that
                    # carries both is refused, so use one or the other.
                    "thinking_budget_tokens": 5000
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
   - Reasoning → claude-opus-4-8 or gpt-5.5 with reasoning_effort set to high
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
