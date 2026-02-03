#!/usr/bin/env python3
"""
Test Gemini thought_signature placement requirements.

This script tests where thoughtSignature must be placed when returning
function responses to Gemini 3 models.
"""

import os
import json
import requests

API_KEY = os.environ.get("GEMINI_API_KEY")
if not API_KEY:
    raise ValueError("GEMINI_API_KEY not set")

BASE_URL = "https://generativelanguage.googleapis.com/v1beta"
MODEL = "gemini-3-flash-preview"  # Gemini 3 model that supports thinking with thoughtSignature

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

def test_thought_signature_placement():
    """Test where thoughtSignature needs to be placed."""

    # Define a simple tool
    tools = [{
        "functionDeclarations": [{
            "name": "get_weather",
            "description": "Get the current weather for a location",
            "parameters": {
                "type": "object",
                "properties": {
                    "location": {"type": "string", "description": "City name"}
                },
                "required": ["location"]
            }
        }]
    }]

    # Step 1: Get a function call with thinking enabled
    print("=" * 60)
    print("STEP 1: Request with thinking to get thoughtSignature")
    print("=" * 60)

    initial_contents = [{
        "role": "user",
        "parts": [{"text": "What's the weather in Tokyo?"}]
    }]

    status, response = make_request(
        initial_contents,
        tools=tools,
        thinking_config={"thinkingBudget": 1024}
    )

    print(f"Status: {status}")

    if status != 200:
        print(f"Error: {json.dumps(response, indent=2)}")
        return

    # Extract the function call and thoughtSignature
    candidates = response.get("candidates", [])
    if not candidates:
        print("No candidates returned")
        return

    model_parts = candidates[0].get("content", {}).get("parts", [])
    print(f"\nModel response parts:")
    for i, part in enumerate(model_parts):
        print(f"  Part {i}: {json.dumps(part, indent=4)}")

    # Find the function call part with thoughtSignature
    function_call_part = None
    thought_signature = None

    for part in model_parts:
        if "functionCall" in part:
            function_call_part = part
            thought_signature = part.get("thoughtSignature")
            break

    if not function_call_part:
        print("\nNo function call in response - model may have answered directly")
        return

    print(f"\nFunction call: {function_call_part.get('functionCall')}")
    print(f"Thought signature present: {thought_signature is not None}")
    if thought_signature:
        print(f"Thought signature (first 50 chars): {thought_signature[:50]}...")

    if not thought_signature:
        print("\nNo thoughtSignature returned - model may not support it or thinking wasn't triggered")
        print("Trying with a different model or prompt...")
        return

    # Step 2: Test different placements
    print("\n" + "=" * 60)
    print("STEP 2: Test signature placements")
    print("=" * 60)

    function_name = function_call_part["functionCall"]["name"]
    function_args = function_call_part["functionCall"].get("args", {})

    # Build the conversation history with model's function call
    model_message = {
        "role": "model",
        "parts": [function_call_part]  # Includes thoughtSignature
    }

    # Test A: Signature on functionCall only (what docs say)
    print("\n--- Test A: Signature on functionCall ONLY ---")
    contents_a = [
        initial_contents[0],
        model_message,  # Has thoughtSignature on functionCall
        {
            "role": "user",
            "parts": [{
                "functionResponse": {
                    "name": function_name,
                    "response": {"weather": "Sunny, 22°C"}
                }
                # NO thoughtSignature here
            }]
        }
    ]

    status_a, response_a = make_request(contents_a, tools=tools)
    print(f"Status: {status_a}")
    if status_a != 200:
        print(f"Error: {json.dumps(response_a, indent=2)}")
    else:
        print("SUCCESS - Model responded")
        answer = response_a.get("candidates", [{}])[0].get("content", {}).get("parts", [{}])[0].get("text", "")
        print(f"Answer preview: {answer[:100]}...")

    # Test B: Signature on functionResponse only
    print("\n--- Test B: Signature on functionResponse ONLY ---")
    model_message_no_sig = {
        "role": "model",
        "parts": [{
            "functionCall": function_call_part["functionCall"]
            # NO thoughtSignature
        }]
    }

    contents_b = [
        initial_contents[0],
        model_message_no_sig,  # No signature on functionCall
        {
            "role": "user",
            "parts": [{
                "functionResponse": {
                    "name": function_name,
                    "response": {"weather": "Sunny, 22°C"}
                },
                "thoughtSignature": thought_signature  # Signature here instead
            }]
        }
    ]

    status_b, response_b = make_request(contents_b, tools=tools)
    print(f"Status: {status_b}")
    if status_b != 200:
        print(f"Error: {json.dumps(response_b, indent=2)}")
    else:
        print("SUCCESS - Model responded")
        answer = response_b.get("candidates", [{}])[0].get("content", {}).get("parts", [{}])[0].get("text", "")
        print(f"Answer preview: {answer[:100]}...")

    # Test C: Signature on BOTH (current Meerkat behavior)
    print("\n--- Test C: Signature on BOTH functionCall AND functionResponse ---")
    contents_c = [
        initial_contents[0],
        model_message,  # Has thoughtSignature on functionCall
        {
            "role": "user",
            "parts": [{
                "functionResponse": {
                    "name": function_name,
                    "response": {"weather": "Sunny, 22°C"}
                },
                "thoughtSignature": thought_signature  # Also here
            }]
        }
    ]

    status_c, response_c = make_request(contents_c, tools=tools)
    print(f"Status: {status_c}")
    if status_c != 200:
        print(f"Error: {json.dumps(response_c, indent=2)}")
    else:
        print("SUCCESS - Model responded")
        answer = response_c.get("candidates", [{}])[0].get("content", {}).get("parts", [{}])[0].get("text", "")
        print(f"Answer preview: {answer[:100]}...")

    # Test D: NO signature anywhere
    print("\n--- Test D: NO signature anywhere ---")
    contents_d = [
        initial_contents[0],
        model_message_no_sig,  # No signature
        {
            "role": "user",
            "parts": [{
                "functionResponse": {
                    "name": function_name,
                    "response": {"weather": "Sunny, 22°C"}
                }
                # No signature
            }]
        }
    ]

    status_d, response_d = make_request(contents_d, tools=tools)
    print(f"Status: {status_d}")
    if status_d != 200:
        print(f"Error: {json.dumps(response_d, indent=2)}")
    else:
        print("SUCCESS - Model responded (signature may be optional for this model)")
        answer = response_d.get("candidates", [{}])[0].get("content", {}).get("parts", [{}])[0].get("text", "")
        print(f"Answer preview: {answer[:100]}...")

    # Summary
    print("\n" + "=" * 60)
    print("SUMMARY")
    print("=" * 60)
    print(f"Test A (sig on functionCall only):     {'PASS' if status_a == 200 else 'FAIL'}")
    print(f"Test B (sig on functionResponse only): {'PASS' if status_b == 200 else 'FAIL'}")
    print(f"Test C (sig on BOTH):                  {'PASS' if status_c == 200 else 'FAIL'}")
    print(f"Test D (NO sig anywhere):              {'PASS' if status_d == 200 else 'FAIL'}")

if __name__ == "__main__":
    test_thought_signature_placement()
