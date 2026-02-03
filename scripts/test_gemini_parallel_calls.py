#!/usr/bin/env python3
"""
Test Gemini thoughtSignature with parallel tool calls.

Question: Does only the first parallel functionCall have a signature,
or do all of them have one?
"""

import os
import json
import requests

API_KEY = os.environ.get("GEMINI_API_KEY")
if not API_KEY:
    raise ValueError("GEMINI_API_KEY not set")

BASE_URL = "https://generativelanguage.googleapis.com/v1beta"
MODEL = "gemini-3-flash-preview"

def make_request(contents, tools=None, thinking_config=None):
    """Make a request to Gemini API."""
    url = f"{BASE_URL}/models/{MODEL}:generateContent?key={API_KEY}"

    body = {"contents": contents}

    if tools:
        body["tools"] = tools

    if thinking_config:
        body["generationConfig"] = {"thinkingConfig": thinking_config}

    response = requests.post(url, json=body, headers={"Content-Type": "application/json"})
    return response.status_code, response.json()

def test_parallel_calls():
    """Test thoughtSignature with parallel function calls."""

    # Define multiple tools
    tools = [{
        "functionDeclarations": [
            {
                "name": "get_weather",
                "description": "Get the current weather for a location",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "location": {"type": "string", "description": "City name"}
                    },
                    "required": ["location"]
                }
            },
            {
                "name": "get_time",
                "description": "Get the current time for a timezone",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "timezone": {"type": "string", "description": "Timezone name"}
                    },
                    "required": ["timezone"]
                }
            },
            {
                "name": "get_population",
                "description": "Get the population of a city",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "city": {"type": "string", "description": "City name"}
                    },
                    "required": ["city"]
                }
            }
        ]
    }]

    # Request that should trigger multiple parallel tool calls
    print("=" * 60)
    print("Testing parallel function calls with thinking enabled")
    print("=" * 60)

    contents = [{
        "role": "user",
        "parts": [{"text": "I need to know the weather, current time, and population for Tokyo. Please get all three pieces of information."}]
    }]

    status, response = make_request(
        contents,
        tools=tools,
        thinking_config={"thinkingBudget": 2048}
    )

    print(f"Status: {status}")

    if status != 200:
        print(f"Error: {json.dumps(response, indent=2)}")
        return

    candidates = response.get("candidates", [])
    if not candidates:
        print("No candidates returned")
        return

    model_parts = candidates[0].get("content", {}).get("parts", [])

    print(f"\nNumber of parts in response: {len(model_parts)}")
    print("\nAnalyzing each part:")
    print("-" * 40)

    function_calls = []
    for i, part in enumerate(model_parts):
        has_fc = "functionCall" in part
        has_sig = "thoughtSignature" in part

        print(f"\nPart {i}:")
        print(f"  Has functionCall: {has_fc}")
        print(f"  Has thoughtSignature: {has_sig}")

        if has_fc:
            fc = part["functionCall"]
            print(f"  Function: {fc.get('name')}")
            print(f"  Args: {fc.get('args')}")
            if has_sig:
                print(f"  Signature (first 30 chars): {part['thoughtSignature'][:30]}...")
            function_calls.append({
                "name": fc.get("name"),
                "has_signature": has_sig,
                "signature": part.get("thoughtSignature")
            })

    print("\n" + "=" * 60)
    print("SUMMARY: Signature distribution across parallel calls")
    print("=" * 60)

    for i, fc in enumerate(function_calls):
        print(f"  Call {i+1} ({fc['name']}): {'HAS' if fc['has_signature'] else 'NO'} signature")

    # Count how many have signatures
    with_sig = sum(1 for fc in function_calls if fc["has_signature"])
    total = len(function_calls)

    print(f"\n  Total function calls: {total}")
    print(f"  With signature: {with_sig}")
    print(f"  Without signature: {total - with_sig}")

    if with_sig == 1 and total > 1:
        print("\n  CONCLUSION: Only FIRST parallel call has signature")
    elif with_sig == total:
        print("\n  CONCLUSION: ALL parallel calls have signatures")
    elif with_sig == 0:
        print("\n  CONCLUSION: NO calls have signatures (unexpected)")
    else:
        print(f"\n  CONCLUSION: {with_sig} of {total} calls have signatures (partial)")

if __name__ == "__main__":
    test_parallel_calls()
