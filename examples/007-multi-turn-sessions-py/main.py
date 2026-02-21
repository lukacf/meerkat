#!/usr/bin/env python3
"""007 — Multi-Turn Sessions (Python SDK)

Agents remember context across turns. This example shows how to build a
multi-turn conversation where the agent accumulates knowledge.

What you'll learn:
- Creating a session and continuing it with `start_turn()`
- Reading session state with `read_session()`
- Listing and archiving sessions
- Streaming events in real-time

Run:
    ANTHROPIC_API_KEY=sk-... python main.py
"""

import asyncio
from meerkat import MeerkatClient


async def main():
    client = MeerkatClient()
    await client.connect()

    try:
        # ── Turn 1: Establish context ──
        print("=== Turn 1: Setting context ===")
        result = await client.create_session(
            prompt=(
                "I'm building a web scraper in Python. "
                "Suggest three libraries I should consider. Be concise."
            ),
            model="claude-sonnet-4-5",
            system_prompt="You are a senior Python developer. Give practical, opinionated advice.",
        )
        session_id = result.session_id
        print(f"Session: {session_id}")
        print(f"Response: {result.text}\n")

        # ── Turn 2: Follow up (agent remembers Turn 1) ──
        print("=== Turn 2: Follow-up question ===")
        result = await client.start_turn(
            session_id=session_id,
            prompt="Which of those three would you pick for scraping JavaScript-heavy SPAs? Why?",
        )
        print(f"Response: {result.text}\n")

        # ── Turn 3: Further refinement ──
        print("=== Turn 3: Implementation advice ===")
        result = await client.start_turn(
            session_id=session_id,
            prompt="Show me a minimal code skeleton using that library with async support.",
        )
        print(f"Response: {result.text}\n")

        # ── Turn 4: Streaming response ──
        print("=== Turn 4: Streaming follow-up ===")
        async with client.start_turn_streaming(
            session_id=session_id,
            prompt="Add error handling and retry logic to the skeleton above.",
        ) as stream:
            async for event in stream:
                if event.get("type") == "text_delta":
                    print(event["delta"], end="", flush=True)
            print()  # newline after stream
            result = stream.result

        # ── Session management ──
        print("\n=== Session info ===")
        info = await client.read_session(session_id)
        print(f"Messages: {info.get('message_count', 'N/A')}")
        print(f"Tokens:   {info.get('total_tokens', 'N/A')}")

        sessions = await client.list_sessions()
        print(f"Active sessions: {len(sessions)}")

        # Clean up
        await client.archive_session(session_id)
        print(f"Session {session_id[:8]}... archived.")

    finally:
        await client.close()


if __name__ == "__main__":
    asyncio.run(main())
