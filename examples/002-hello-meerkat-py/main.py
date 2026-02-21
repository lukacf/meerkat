#!/usr/bin/env python3
"""002 — Hello Meerkat (Python SDK)

The simplest Python agent: connect to the Meerkat runtime, create a session,
and print the response. The Python SDK communicates with the `rkat-rpc` binary
via JSON-RPC over stdio — no HTTP server needed.

What you'll learn:
- Connecting to the Meerkat runtime
- Creating a session with a prompt
- Reading the WireRunResult

Run:
    ANTHROPIC_API_KEY=sk-... python main.py
"""

import asyncio
from meerkat import MeerkatClient


async def main():
    client = MeerkatClient()
    await client.connect()

    try:
        result = await client.create_session(
            prompt="What makes Rust's ownership model unique? Answer in two sentences.",
            model="claude-sonnet-4-5",
        )
        print(result.text)
        print(f"\n--- Stats ---")
        print(f"Session:  {result.session_id}")
        print(f"Turns:    {result.turns}")
        print(f"Tokens:   {result.usage.input_tokens + result.usage.output_tokens}")
    finally:
        await client.close()


if __name__ == "__main__":
    asyncio.run(main())
