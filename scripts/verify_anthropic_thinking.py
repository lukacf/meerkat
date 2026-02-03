#!/usr/bin/env python3
"""
Verify Anthropic thinking block API behavior.

This script tests the claims in spec section 2.1:
1. `interleaved-thinking-2025-05-14` beta header enables thinking blocks
2. Thinking blocks have `signature` field
3. `signature_delta` arrives as separate SSE event
4. Block ordering: thinking -> text -> tool_use
5. tool_use blocks have `id` not signature

Usage:
    ANTHROPIC_API_KEY=... python scripts/verify_anthropic_thinking.py
"""

import json
import os
import sys

import httpx

API_URL = "https://api.anthropic.com/v1/messages"
MODEL = "claude-sonnet-4-5"
API_KEY = os.environ.get("ANTHROPIC_API_KEY")

if not API_KEY:
    print("ERROR: ANTHROPIC_API_KEY environment variable required")
    sys.exit(1)


def make_streaming_request(
    prompt: str,
    tools: list | None = None,
    use_beta_header: bool = True,
    thinking_budget: int = 1024,
) -> list[dict]:
    """Make a streaming request and collect all SSE events."""

    headers = {
        "anthropic-version": "2023-06-01",
        "x-api-key": API_KEY,
        "content-type": "application/json",
    }

    if use_beta_header:
        headers["anthropic-beta"] = "interleaved-thinking-2025-05-14"

    body = {
        "model": MODEL,
        "max_tokens": 4096,
        "stream": True,
        "messages": [{"role": "user", "content": prompt}],
    }

    if use_beta_header:
        body["thinking"] = {
            "type": "enabled",
            "budget_tokens": thinking_budget,
        }

    if tools:
        body["tools"] = tools

    events = []

    with httpx.Client(timeout=120.0) as client:
        with client.stream("POST", API_URL, headers=headers, json=body) as response:
            if response.status_code != 200:
                error_text = response.read().decode()
                print(f"HTTP {response.status_code}: {error_text}")
                return []

            buffer = ""
            for chunk in response.iter_text():
                buffer += chunk
                while "\n\n" in buffer:
                    event_text, buffer = buffer.split("\n\n", 1)
                    lines = event_text.strip().split("\n")

                    event_type = None
                    event_data = None

                    for line in lines:
                        if line.startswith("event: "):
                            event_type = line[7:]
                        elif line.startswith("data: "):
                            try:
                                event_data = json.loads(line[6:])
                            except json.JSONDecodeError:
                                event_data = line[6:]

                    if event_type and event_data:
                        events.append({"event": event_type, "data": event_data})

    return events


def test_beta_header_enables_thinking():
    """Test 1: Beta header enables thinking blocks."""
    print("\n" + "=" * 70)
    print("TEST 1: Beta header enables thinking blocks")
    print("=" * 70)

    prompt = "Think step by step: what is 17 * 23?"

    # With beta header
    print("\n[With beta header]")
    events = make_streaming_request(prompt, use_beta_header=True)

    thinking_blocks = []
    for e in events:
        if e["event"] == "content_block_start":
            block = e["data"].get("content_block", {})
            if block.get("type") == "thinking":
                thinking_blocks.append(block)
                print(f"  Found thinking block at index {e['data'].get('index')}")

    if thinking_blocks:
        print(f"  PASS: Found {len(thinking_blocks)} thinking block(s)")
    else:
        print("  FAIL: No thinking blocks found with beta header")
        # Print all block types for debugging
        for e in events:
            if e["event"] == "content_block_start":
                block = e["data"].get("content_block", {})
                print(f"    Block type: {block.get('type')}")

    return len(thinking_blocks) > 0


def test_thinking_signature_field():
    """Test 2: Thinking blocks have signature field."""
    print("\n" + "=" * 70)
    print("TEST 2: Thinking blocks have signature field")
    print("=" * 70)

    prompt = "Think carefully: what day comes after Tuesday?"
    events = make_streaming_request(prompt, use_beta_header=True)

    # Track signatures from signature_delta events
    signature_deltas = []
    current_thinking_index = None

    for e in events:
        if e["event"] == "content_block_start":
            block = e["data"].get("content_block", {})
            if block.get("type") == "thinking":
                current_thinking_index = e["data"].get("index")
                # Check if signature is in start block (unlikely)
                if block.get("signature"):
                    print(f"  Signature in start block: {block['signature'][:50]}...")

        if e["event"] == "content_block_delta":
            delta = e["data"].get("delta", {})
            if delta.get("type") == "signature_delta":
                sig = delta.get("signature", "")
                signature_deltas.append(sig)
                print(f"  Found signature_delta at index {e['data'].get('index')}")
                print(f"    Signature preview: {sig[:80]}...")

    if signature_deltas:
        print(f"  PASS: Found {len(signature_deltas)} signature(s)")
        return True
    else:
        print("  FAIL: No signatures found")
        # Dump all delta types for debugging
        print("  Delta types seen:")
        for e in events:
            if e["event"] == "content_block_delta":
                delta = e["data"].get("delta", {})
                print(f"    {delta.get('type')}")
        return False


def test_signature_delta_event():
    """Test 3: signature_delta arrives as separate SSE event."""
    print("\n" + "=" * 70)
    print("TEST 3: signature_delta arrives as separate SSE event")
    print("=" * 70)

    prompt = "Think about this: is 7 a prime number?"
    events = make_streaming_request(prompt, use_beta_header=True)

    # Look for signature_delta events
    signature_delta_events = []
    for e in events:
        if e["event"] == "content_block_delta":
            delta = e["data"].get("delta", {})
            if delta.get("type") == "signature_delta":
                signature_delta_events.append(e)
                print(f"  Found signature_delta event:")
                print(f"    Index: {e['data'].get('index')}")
                print(f"    Delta type: {delta.get('type')}")
                print(f"    Has signature: {'signature' in delta}")

    if signature_delta_events:
        print(f"  PASS: signature_delta is separate event type")
        return True
    else:
        print("  FAIL: No signature_delta events found")
        return False


def test_block_ordering():
    """Test 4: Block ordering is thinking -> text -> tool_use."""
    print("\n" + "=" * 70)
    print("TEST 4: Block ordering (thinking -> text -> tool_use)")
    print("=" * 70)

    tools = [
        {
            "name": "get_weather",
            "description": "Get current weather for a location",
            "input_schema": {
                "type": "object",
                "properties": {
                    "location": {"type": "string", "description": "City name"}
                },
                "required": ["location"],
            },
        }
    ]

    prompt = "What's the weather in Tokyo? Think about it first."
    events = make_streaming_request(prompt, tools=tools, use_beta_header=True)

    block_order = []
    for e in events:
        if e["event"] == "content_block_start":
            block = e["data"].get("content_block", {})
            block_type = block.get("type")
            index = e["data"].get("index")
            block_order.append((index, block_type))
            print(f"  Block {index}: {block_type}")
            if block_type == "tool_use":
                print(f"    Tool name: {block.get('name')}")
                print(f"    Tool ID: {block.get('id')}")

    # Check order
    types_in_order = [t for _, t in sorted(block_order)]
    print(f"\n  Block types in order: {types_in_order}")

    # Verify: thinking should come before text, text before tool_use
    thinking_idx = next((i for i, t in enumerate(types_in_order) if t == "thinking"), -1)
    text_idx = next((i for i, t in enumerate(types_in_order) if t == "text"), -1)
    tool_idx = next((i for i, t in enumerate(types_in_order) if t == "tool_use"), -1)

    order_correct = True
    if thinking_idx >= 0 and text_idx >= 0:
        if thinking_idx > text_idx:
            print("  WARNING: thinking came after text")
            order_correct = False
    if text_idx >= 0 and tool_idx >= 0:
        if text_idx > tool_idx:
            print("  WARNING: text came after tool_use")
            order_correct = False
    if thinking_idx >= 0 and tool_idx >= 0:
        if thinking_idx > tool_idx:
            print("  WARNING: thinking came after tool_use")
            order_correct = False

    if order_correct and (thinking_idx >= 0 or tool_idx >= 0):
        print("  PASS: Block ordering is correct")
        return True
    elif thinking_idx == -1 and tool_idx == -1:
        print("  INCONCLUSIVE: No thinking or tool_use blocks to verify order")
        return False
    else:
        print("  FAIL: Block ordering incorrect")
        return False


def test_tool_use_has_id_not_signature():
    """Test 5: tool_use blocks have id, not signature."""
    print("\n" + "=" * 70)
    print("TEST 5: tool_use blocks have id (not signature)")
    print("=" * 70)

    tools = [
        {
            "name": "calculate",
            "description": "Perform a calculation",
            "input_schema": {
                "type": "object",
                "properties": {
                    "expression": {"type": "string", "description": "Math expression"}
                },
                "required": ["expression"],
            },
        }
    ]

    prompt = "Calculate 42 * 17 for me."
    events = make_streaming_request(prompt, tools=tools, use_beta_header=True)

    tool_use_blocks = []
    for e in events:
        if e["event"] == "content_block_start":
            block = e["data"].get("content_block", {})
            if block.get("type") == "tool_use":
                tool_use_blocks.append(block)
                print(f"  Found tool_use block:")
                print(f"    id: {block.get('id')}")
                print(f"    name: {block.get('name')}")
                print(f"    has signature: {'signature' in block}")
                print(f"    Raw block: {json.dumps(block, indent=6)}")

    if not tool_use_blocks:
        print("  INCONCLUSIVE: No tool_use blocks found")
        return False

    all_have_id = all(b.get("id") for b in tool_use_blocks)
    none_have_signature = all("signature" not in b for b in tool_use_blocks)

    if all_have_id and none_have_signature:
        print(f"  PASS: All {len(tool_use_blocks)} tool_use block(s) have id, none have signature")
        return True
    else:
        print(f"  FAIL: id present: {all_have_id}, signature absent: {none_have_signature}")
        return False


def dump_raw_events():
    """Dump raw SSE events for manual inspection."""
    print("\n" + "=" * 70)
    print("RAW EVENT DUMP (for manual verification)")
    print("=" * 70)

    tools = [
        {
            "name": "get_time",
            "description": "Get current time",
            "input_schema": {
                "type": "object",
                "properties": {},
            },
        }
    ]

    prompt = "Think briefly about time zones, then tell me the current time."
    events = make_streaming_request(prompt, tools=tools, use_beta_header=True, thinking_budget=1024)

    print(f"\nTotal events: {len(events)}")
    print("\nKey events (content_block_start and content_block_delta with signatures):\n")

    for i, e in enumerate(events):
        if e["event"] == "content_block_start":
            print(f"[{i}] {e['event']}:")
            print(f"     {json.dumps(e['data'], indent=4)}")
        elif e["event"] == "content_block_delta":
            delta = e["data"].get("delta", {})
            delta_type = delta.get("type", "")
            if delta_type in ("signature_delta", "thinking_delta"):
                print(f"[{i}] {e['event']} ({delta_type}):")
                # Truncate long content for display
                data_copy = e["data"].copy()
                if "delta" in data_copy:
                    delta_copy = data_copy["delta"].copy()
                    if "thinking" in delta_copy and len(delta_copy["thinking"]) > 100:
                        delta_copy["thinking"] = delta_copy["thinking"][:100] + "..."
                    if "signature" in delta_copy and len(delta_copy["signature"]) > 100:
                        delta_copy["signature"] = delta_copy["signature"][:100] + "..."
                    data_copy["delta"] = delta_copy
                print(f"     {json.dumps(data_copy, indent=4)}")
        elif e["event"] == "content_block_stop":
            print(f"[{i}] {e['event']}: index={e['data'].get('index')}")


def main():
    print("=" * 70)
    print("ANTHROPIC THINKING BLOCK API VERIFICATION")
    print(f"Model: {MODEL}")
    print("=" * 70)

    results = {}

    results["beta_header"] = test_beta_header_enables_thinking()
    results["signature_field"] = test_thinking_signature_field()
    results["signature_delta_event"] = test_signature_delta_event()
    results["block_ordering"] = test_block_ordering()
    results["tool_use_id_not_signature"] = test_tool_use_has_id_not_signature()

    dump_raw_events()

    print("\n" + "=" * 70)
    print("SUMMARY")
    print("=" * 70)
    for test_name, passed in results.items():
        status = "PASS" if passed else "FAIL"
        print(f"  {test_name}: {status}")

    all_passed = all(results.values())
    print("\n" + ("ALL TESTS PASSED" if all_passed else "SOME TESTS FAILED"))

    return 0 if all_passed else 1


if __name__ == "__main__":
    sys.exit(main())
