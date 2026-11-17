#!/usr/bin/env python3
"""021 — Multi-Provider Routing (Python SDK)

Use different LLM providers for different tasks. Meerkat supports Anthropic,
OpenAI, and Gemini — you can switch between them per-session.

What you'll learn:
- Routing to different providers (Anthropic, OpenAI, Gemini)
- One reasoning-effort setting for every provider, and what a refusal says
- A provider-specific parameter (Anthropic adaptive thinking)
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
            models.append("claude-sonnet-5-5")
        if os.environ.get("OPENAI_API_KEY"):
            models.append("gpt-6-astra")
        if os.environ.get("GEMINI_API_KEY"):
            models.append("gemini-4-hydrogen")

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
        # refused before any provider request, and the refusal says which
        # levels the model accepts when a source states them.
        for model in models:
            print(f"--- {model} with reasoning_effort=high ---")
            try:
                session = await client.create_session(
                    prompt="What is the optimal data structure for a LRU cache?",
                    model=model,
                    reasoning_effort="high",
                )
            except MeerkatError as error:
                # Two different things can go wrong. A refusal is Meerkat
                # declining the level for this model, with a typed reason. Any
                # other error (the provider, the key, the network) is not about
                # the level at all.
                refusal = reasoning_effort_refusal(error)
                if refusal is None:
                    print(f"  Error ({type(error).__name__}, code {error.code}): {error}")
                elif refusal.supported is None:
                    # No source states the levels for this model, or the
                    # source cannot be read now. Only the second can succeed
                    # later with no change (`refusal.transient`).
                    later = "; try again later" if refusal.transient else ""
                    print(f"  Refused: {refusal.reason}; the levels this model accepts are not known{later}")
                elif not refusal.supported:
                    print(f"  Refused: {refusal.reason}; this model takes no reasoning-effort level")
                else:
                    accepted = ", ".join(refusal.supported)
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
            print("--- Anthropic with adaptive thinking ---")
            session = await client.create_session(
                prompt="What is the optimal data structure for a LRU cache? Think step by step.",
                model="claude-sonnet-5-5",
                provider_params={
                    # Provider-specific settings travel under the typed
                    # provider_tag. This one asks Anthropic for adaptive
                    # thinking: the model decides how much to think. Current
                    # Claude models take a thinking type like this one, not a
                    # token budget (a `thinking_budget_tokens` value is
                    # refused for them).
                    "provider_tag": {"provider": "anthropic", "thinking": {"type": "adaptive"}}
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
   - Simple tasks → claude-sonnet-5-5 or gemini-4-hydrogen
   - Complex tasks → claude-opus-5-5 or gpt-6-astra

2. CAPABILITY-BASED
   - Code generation → claude-sonnet-5-5 (strong at code)
   - Reasoning → claude-opus-5-5 or gpt-6-astra with reasoning_effort set to high
   - Speed → gemini-4-hydrogen

3. FALLBACK CHAIN
   - Try primary provider
   - On failure, fall back to secondary
   - Meerkat's retry policy handles transient failures

4. PER-AGENT ROUTING (in mobs)
   - Orchestrator: claude-opus-5-5 (complex planning)
   - Workers: claude-sonnet-5-5 (execution)
   - Validators: gemini-4-hydrogen (fast checks)
""")

    finally:
        await client.close()


if __name__ == "__main__":
    asyncio.run(main())
