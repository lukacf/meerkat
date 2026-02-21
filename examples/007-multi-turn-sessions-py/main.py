#!/usr/bin/env python3
"""007 — Multi-Turn Sessions (Python SDK)

Agents remember context across turns. This example shows how to build a
multi-turn conversation where the agent accumulates knowledge.

What you'll learn:
- Creating a session and continuing it with `session.turn()`
- Reading session state with `client.read_session()`
- Listing sessions and archiving via `session.archive()`
- Streaming events in real-time

Run:
    ANTHROPIC_API_KEY=sk-... python main.py
"""

import asyncio
from meerkat import MeerkatClient, TextDelta


async def main():
    client = MeerkatClient()
    await client.connect()

    try:
        # ── Turn 1: Establish context ──
        print("=== Turn 1: Setting context ===")
        session = await client.create_session(
            prompt=(
                "I'm building a web scraper in Python. "
                "Suggest three libraries I should consider. Be concise."
            ),
            model="claude-sonnet-4-5",
            system_prompt="You are a senior Python developer. Give practical, opinionated advice.",
        )
        print(f"Session: {session.id}")
        print(f"Response: {session.text}\n")

        # ── Turn 2: Follow up (agent remembers Turn 1) ──
        print("=== Turn 2: Follow-up question ===")
        result = await session.turn(
            "Which of those three would you pick for scraping JavaScript-heavy SPAs? Why?",
        )
        print(f"Response: {result.text}\n")

        # ── Turn 3: Further refinement ──
        print("=== Turn 3: Implementation advice ===")
        result = await session.turn(
            "Show me a minimal code skeleton using that library with async support.",
        )
        print(f"Response: {result.text}\n")

        # ── Turn 4: Streaming response ──
        print("=== Turn 4: Streaming follow-up ===")
        async with session.stream(
            "Add error handling and retry logic to the skeleton above.",
        ) as events:
            async for event in events:
                match event:
                    case TextDelta(delta=chunk):
                        print(chunk, end="", flush=True)
            print()  # newline after stream

        # ── Session management ──
        print("\n=== Session info ===")
        info = await client.read_session(session.id)
        print(f"Messages: {info.get('message_count', 'N/A')}")
        print(f"Tokens:   {info.get('total_tokens', 'N/A')}")

        sessions = await client.list_sessions()
        print(f"Active sessions: {len(sessions)}")

        # Clean up
        await session.archive()
        print(f"Session {session.id[:8]}... archived.")

    finally:
        await client.close()


if __name__ == "__main__":
    asyncio.run(main())
