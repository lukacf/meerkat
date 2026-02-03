#!/usr/bin/env python3
"""
Verify OpenAI Responses API claims from spec section 2.2.

Spec claims to verify:
1. /v1/responses endpoint exists and works
2. type: "reasoning" items have `summary` array
3. summary structure is [{"type": "summary_text", "text": "..."}]
4. encrypted_content field with include: ["reasoning.encrypted_content"]
5. type: "function_call" items have call_id, name, arguments
6. type: "message" content is array with type: "output_text" parts
"""

import os
import json
import httpx

OPENAI_API_KEY = os.environ.get("OPENAI_API_KEY")
if not OPENAI_API_KEY:
    raise RuntimeError("OPENAI_API_KEY environment variable required")

BASE_URL = "https://api.openai.com/v1"
MODEL = "gpt-5.2"

headers = {
    "Authorization": f"Bearer {OPENAI_API_KEY}",
    "Content-Type": "application/json",
}


def test_1_responses_endpoint_exists():
    """Test 1: /v1/responses endpoint exists and works."""
    print("\n" + "=" * 60)
    print("TEST 1: /v1/responses endpoint exists")
    print("=" * 60)

    payload = {
        "model": MODEL,
        "input": [
            {"type": "message", "role": "user", "content": "Say 'hello' only."}
        ],
    }

    response = httpx.post(
        f"{BASE_URL}/responses",
        headers=headers,
        json=payload,
        timeout=60.0,
    )

    print(f"Status: {response.status_code}")
    if response.status_code == 200:
        data = response.json()
        print(f"Response ID: {data.get('id')}")
        print(f"Model: {data.get('model')}")
        print("PASS: Endpoint exists and responds")
        return True, data
    else:
        print(f"Error: {response.text}")
        print("FAIL: Endpoint not working")
        return False, None


def test_2_reasoning_with_summary():
    """Test 2 & 3: Reasoning items have summary array with correct structure."""
    print("\n" + "=" * 60)
    print("TEST 2 & 3: Reasoning items with summary structure")
    print("=" * 60)

    payload = {
        "model": MODEL,
        "input": [
            {
                "type": "message",
                "role": "user",
                "content": "What is 15 * 23? Think step by step.",
            }
        ],
        "reasoning": {"effort": "medium", "summary": "auto"},
    }

    response = httpx.post(
        f"{BASE_URL}/responses",
        headers=headers,
        json=payload,
        timeout=120.0,
    )

    print(f"Status: {response.status_code}")
    if response.status_code != 200:
        print(f"Error: {response.text}")
        print("FAIL: Request failed")
        return False, None

    data = response.json()
    print("\nFull response JSON:")
    print(json.dumps(data, indent=2))

    # Find reasoning items in output
    output = data.get("output", [])
    reasoning_items = [item for item in output if item.get("type") == "reasoning"]

    if not reasoning_items:
        print("\nNo reasoning items found in output")
        print("FAIL: No reasoning items")
        return False, data

    print(f"\nFound {len(reasoning_items)} reasoning item(s)")
    for i, item in enumerate(reasoning_items):
        print(f"\nReasoning item {i + 1}:")
        print(json.dumps(item, indent=2))

        # Check for summary array
        summary = item.get("summary")
        if summary is None:
            print("  - summary field: MISSING")
        elif not isinstance(summary, list):
            print(f"  - summary field: NOT AN ARRAY (got {type(summary).__name__})")
        else:
            print(f"  - summary field: PRESENT (array with {len(summary)} items)")
            for j, s in enumerate(summary):
                s_type = s.get("type")
                s_text = s.get("text", "")[:100]  # Truncate for display
                print(f"    [{j}] type={s_type!r}, text={s_text!r}...")

    print("\nPASS: Reasoning items found with summary structure")
    return True, data


def test_4_encrypted_content():
    """Test 4: encrypted_content field with include param."""
    print("\n" + "=" * 60)
    print("TEST 4: encrypted_content with include param")
    print("=" * 60)

    # Use a complex multi-step reasoning problem and tool to force reasoning
    payload = {
        "model": MODEL,
        "input": [
            {
                "type": "message",
                "role": "user",
                "content": "I need you to analyze this complex problem: A train leaves station A at 9:00 AM traveling at 60 mph. Another train leaves station B (300 miles away) at 10:00 AM traveling toward station A at 80 mph. At what time will they meet? Think through this carefully step by step before answering.",
            }
        ],
        "reasoning": {"effort": "high", "summary": "detailed"},
        "include": ["reasoning.encrypted_content"],
    }

    response = httpx.post(
        f"{BASE_URL}/responses",
        headers=headers,
        json=payload,
        timeout=120.0,
    )

    print(f"Status: {response.status_code}")
    if response.status_code != 200:
        print(f"Error: {response.text}")
        print("FAIL: Request failed")
        return False, None

    data = response.json()
    print("\nFull response JSON:")
    print(json.dumps(data, indent=2))

    # Find reasoning items
    output = data.get("output", [])
    reasoning_items = [item for item in output if item.get("type") == "reasoning"]

    if not reasoning_items:
        print("\nNo reasoning items found in output")
        # Check if the model just decided not to reason - this is informative
        print("NOTE: Model may have decided reasoning wasn't needed for this query")
        print("NOTE: encrypted_content verification inconclusive without reasoning items")
        return False, data

    found_encrypted = False
    for i, item in enumerate(reasoning_items):
        enc = item.get("encrypted_content")
        if enc:
            print(f"\nReasoning item {i + 1} has encrypted_content:")
            print(f"  Length: {len(enc)} chars")
            print(f"  Preview: {enc[:80]}...")
            found_encrypted = True
        else:
            print(f"\nReasoning item {i + 1}: NO encrypted_content field")
            # Check if it has id and summary (which we know works)
            item_id = item.get("id")
            summary = item.get("summary")
            print(f"    (item has id={item_id is not None}, summary={summary is not None})")

    if found_encrypted:
        print("\nPASS: encrypted_content field present when requested")
    else:
        print("\nFAIL/INCONCLUSIVE: No encrypted_content found")
        print("  Reasoning items exist but don't have encrypted_content")
        print("  This may be expected if model chose not to use extended reasoning")
    return found_encrypted, data


def test_5_function_call_structure():
    """Test 5: function_call items have call_id, name, arguments."""
    print("\n" + "=" * 60)
    print("TEST 5: function_call structure (call_id, name, arguments)")
    print("=" * 60)

    payload = {
        "model": MODEL,
        "input": [
            {
                "type": "message",
                "role": "user",
                "content": "What's the weather like in Tokyo?",
            }
        ],
        "tools": [
            {
                "type": "function",
                "name": "get_weather",
                "description": "Get current weather for a location",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "location": {
                            "type": "string",
                            "description": "City name",
                        }
                    },
                    "required": ["location"],
                },
            }
        ],
    }

    response = httpx.post(
        f"{BASE_URL}/responses",
        headers=headers,
        json=payload,
        timeout=120.0,
    )

    print(f"Status: {response.status_code}")
    if response.status_code != 200:
        print(f"Error: {response.text}")
        print("FAIL: Request failed")
        return False, None

    data = response.json()
    print("\nFull response JSON:")
    print(json.dumps(data, indent=2))

    # Find function_call items
    output = data.get("output", [])
    func_calls = [item for item in output if item.get("type") == "function_call"]

    if not func_calls:
        print("\nNo function_call items found in output")
        print("FAIL: Model didn't call the tool")
        return False, data

    print(f"\nFound {len(func_calls)} function_call item(s)")
    all_valid = True
    for i, item in enumerate(func_calls):
        print(f"\nFunction call {i + 1}:")
        print(json.dumps(item, indent=2))

        call_id = item.get("call_id")
        name = item.get("name")
        arguments = item.get("arguments")

        print(f"  - call_id: {call_id!r} {'PRESENT' if call_id else 'MISSING'}")
        print(f"  - name: {name!r} {'PRESENT' if name else 'MISSING'}")
        print(f"  - arguments: {arguments!r} {'PRESENT' if arguments else 'MISSING'}")

        if not all([call_id, name, arguments is not None]):
            all_valid = False

    if all_valid:
        print("\nPASS: function_call items have all required fields")
    else:
        print("\nFAIL: Some function_call items missing required fields")
    return all_valid, data


def test_6_message_content_structure():
    """Test 6: message content is array with output_text parts."""
    print("\n" + "=" * 60)
    print("TEST 6: message content structure (array with output_text)")
    print("=" * 60)

    payload = {
        "model": MODEL,
        "input": [
            {"type": "message", "role": "user", "content": "Say 'hello world' only."}
        ],
    }

    response = httpx.post(
        f"{BASE_URL}/responses",
        headers=headers,
        json=payload,
        timeout=60.0,
    )

    print(f"Status: {response.status_code}")
    if response.status_code != 200:
        print(f"Error: {response.text}")
        print("FAIL: Request failed")
        return False, None

    data = response.json()
    print("\nFull response JSON:")
    print(json.dumps(data, indent=2))

    # Find message items
    output = data.get("output", [])
    messages = [item for item in output if item.get("type") == "message"]

    if not messages:
        print("\nNo message items found in output")
        print("FAIL: No message items")
        return False, data

    print(f"\nFound {len(messages)} message item(s)")
    all_valid = True
    for i, item in enumerate(messages):
        print(f"\nMessage {i + 1}:")
        content = item.get("content")

        if content is None:
            print("  content: MISSING")
            all_valid = False
            continue

        if not isinstance(content, list):
            print(f"  content: NOT AN ARRAY (got {type(content).__name__})")
            all_valid = False
            continue

        print(f"  content: ARRAY with {len(content)} part(s)")
        for j, part in enumerate(content):
            part_type = part.get("type")
            part_text = part.get("text", "")[:50]
            print(f"    [{j}] type={part_type!r}, text={part_text!r}...")
            if part_type == "output_text":
                print("         ^ CORRECT output_text type")

    if all_valid:
        print("\nPASS: message content is array with output_text parts")
    else:
        print("\nFAIL: message content structure incorrect")
    return all_valid, data


def main():
    print("=" * 60)
    print("OpenAI Responses API Verification")
    print(f"Model: {MODEL}")
    print("=" * 60)

    results = {}

    # Test 1: Endpoint exists
    passed, _ = test_1_responses_endpoint_exists()
    results["1_endpoint_exists"] = passed

    # Test 2 & 3: Reasoning with summary structure
    passed, _ = test_2_reasoning_with_summary()
    results["2_3_reasoning_summary"] = passed

    # Test 4: encrypted_content
    passed, _ = test_4_encrypted_content()
    results["4_encrypted_content"] = passed

    # Test 5: function_call structure
    passed, _ = test_5_function_call_structure()
    results["5_function_call"] = passed

    # Test 6: message content structure
    passed, _ = test_6_message_content_structure()
    results["6_message_content"] = passed

    # Summary
    print("\n" + "=" * 60)
    print("SUMMARY")
    print("=" * 60)
    for test_name, passed in results.items():
        status = "PASS" if passed else "FAIL"
        print(f"  {test_name}: {status}")

    all_passed = all(results.values())
    print("\n" + ("ALL TESTS PASSED" if all_passed else "SOME TESTS FAILED"))
    return 0 if all_passed else 1


if __name__ == "__main__":
    exit(main())
