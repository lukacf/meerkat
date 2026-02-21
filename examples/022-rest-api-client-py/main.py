#!/usr/bin/env python3
"""022 — REST API Client (Python)

Meerkat exposes a REST API server for HTTP-based integrations. This example
shows how to interact with it using standard HTTP requests — no SDK required.

What you'll learn:
- Starting the REST server (`rkat rest`)
- Creating sessions via POST /sessions
- Multi-turn via POST /sessions/:id/messages
- Streaming via SSE (Server-Sent Events)
- Session management via REST endpoints

Run:
    # Terminal 1: Start the REST server
    ANTHROPIC_API_KEY=sk-... rkat rest --port 8000

    # Terminal 2: Run this script
    python main.py
"""

import asyncio
import json
import os

# Using stdlib only — no external dependencies required
from urllib.request import Request, urlopen
from urllib.error import HTTPError


BASE_URL = os.environ.get("MEERKAT_REST_URL", "http://localhost:8000")


def api_request(method: str, path: str, body: dict | None = None) -> dict:
    """Make a REST API request."""
    url = f"{BASE_URL}{path}"
    data = json.dumps(body).encode() if body else None
    headers = {"Content-Type": "application/json"} if body else {}

    req = Request(url, data=data, headers=headers, method=method)
    try:
        with urlopen(req) as resp:
            return json.loads(resp.read().decode())
    except HTTPError as e:
        error_body = e.read().decode()
        print(f"  HTTP {e.code}: {error_body}")
        raise


def main():
    print("=== Meerkat REST API Client ===\n")
    print(f"Base URL: {BASE_URL}")
    print("(Start the server with: rkat rest --port 8000)\n")

    # ── 1. Create a session ──
    print("--- 1. POST /sessions (create session) ---")
    try:
        result = api_request("POST", "/sessions", {
            "prompt": "What are the three pillars of observability?",
            "model": "claude-sonnet-4-5",
            "system_prompt": "You are a DevOps expert. Be concise.",
        })
        session_id = result["session_id"]
        print(f"  Session: {session_id}")
        print(f"  Response: {result['text'][:150]}...")
        print(f"  Tokens: {result['usage']['input_tokens'] + result['usage']['output_tokens']}")
    except Exception as e:
        print(f"  Failed to connect. Is the REST server running? ({e})")
        print("\n  Start it with: ANTHROPIC_API_KEY=sk-... rkat rest --port 8000")
        show_reference()
        return

    # ── 2. Continue the session ──
    print(f"\n--- 2. POST /sessions/{session_id[:8]}../messages (continue) ---")
    result = api_request("POST", f"/sessions/{session_id}/messages", {
        "prompt": "Explain the difference between metrics and traces.",
    })
    print(f"  Response: {result['text'][:150]}...")

    # ── 3. Read session state ──
    print(f"\n--- 3. GET /sessions/{session_id[:8]}.. (read) ---")
    info = api_request("GET", f"/sessions/{session_id}")
    print(f"  Messages: {info.get('message_count', 'N/A')}")
    print(f"  Tokens: {info.get('total_tokens', 'N/A')}")

    # ── 4. List sessions ──
    print("\n--- 4. GET /sessions (list) ---")
    sessions = api_request("GET", "/sessions")
    print(f"  Active sessions: {len(sessions.get('sessions', []))}")

    # ── 5. Archive session ──
    print(f"\n--- 5. POST /sessions/{session_id[:8]}../archive ---")
    api_request("POST", f"/sessions/{session_id}/archive")
    print("  Session archived.")

    show_reference()


def show_reference():
    print("\n\n=== REST API Reference ===\n")
    print("""
Endpoints:
  POST /sessions              Create session + run first turn
  POST /sessions/:id/messages Continue session (new turn)
  GET  /sessions              List sessions
  GET  /sessions/:id          Read session state
  POST /sessions/:id/archive  Archive session
  GET  /sessions/:id/events   SSE stream (Server-Sent Events)

  POST /comms/send            Send comms message
  GET  /comms/peers           List comms peers
  POST /webhooks/comms-message  Receive external comms message

Start the server:
  rkat rest                   # Default port 8000
  rkat rest --port 3000       # Custom port
  rkat rest --host 0.0.0.0    # Listen on all interfaces

Streaming via SSE:
  curl -N http://localhost:8000/sessions/{id}/events

  Events:
    data: {"type": "text_delta", "delta": "Hello"}
    data: {"type": "tool_call_requested", "name": "search", ...}
    data: {"type": "turn_completed", ...}
""")


if __name__ == "__main__":
    main()
