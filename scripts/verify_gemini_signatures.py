#!/usr/bin/env python3
"""
Verify Gemini thoughtSignature claims from spec Section 2.3.

This script makes real API calls to verify:
1. thoughtSignature appears on functionCall parts
2. thoughtSignature appears on text parts for non-tool turns
3. Parallel calls: only FIRST has signature
4. Gemini 3 preview models REQUIRE signatures (400 error without)

Uses GOOGLE_API_KEY from environment.
Prints raw JSON responses for verification.
"""

import os
import json
import sys
from typing import Tuple, Any

try:
    import requests
except ImportError:
    print("ERROR: requests library required. Install with: pip install requests")
    sys.exit(1)

# API configuration
API_KEY = os.environ.get("GOOGLE_API_KEY") or os.environ.get("GEMINI_API_KEY")
if not API_KEY:
    print("ERROR: GOOGLE_API_KEY or GEMINI_API_KEY environment variable not set")
    sys.exit(1)

BASE_URL = "https://generativelanguage.googleapis.com/v1beta"
MODEL = "gemini-3-flash-preview"


def make_request(
    contents: list,
    tools: list | None = None,
    thinking_config: dict | None = None,
) -> Tuple[int, Any]:
    """Make a request to Gemini API."""
    url = f"{BASE_URL}/models/{MODEL}:generateContent?key={API_KEY}"

    body = {"contents": contents}
    if tools:
        body["tools"] = tools
    if thinking_config:
        body["generationConfig"] = {"thinkingConfig": thinking_config}

    response = requests.post(
        url, json=body, headers={"Content-Type": "application/json"}
    )
    return response.status_code, response.json()


def get_weather_tool():
    """Define a simple weather tool for testing."""
    return [
        {
            "functionDeclarations": [
                {
                    "name": "get_weather",
                    "description": "Get the current weather for a location",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "location": {"type": "string", "description": "City name"}
                        },
                        "required": ["location"],
                    },
                }
            ]
        }
    ]


def get_multi_tools():
    """Define multiple tools to trigger parallel calls."""
    return [
        {
            "functionDeclarations": [
                {
                    "name": "get_weather",
                    "description": "Get the current weather for a location",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "location": {"type": "string", "description": "City name"}
                        },
                        "required": ["location"],
                    },
                },
                {
                    "name": "get_time",
                    "description": "Get the current time for a timezone",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "timezone": {
                                "type": "string",
                                "description": "Timezone name",
                            }
                        },
                        "required": ["timezone"],
                    },
                },
                {
                    "name": "get_population",
                    "description": "Get the population of a city",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "city": {"type": "string", "description": "City name"}
                        },
                        "required": ["city"],
                    },
                },
            ]
        }
    ]


def print_raw_json(label: str, data: Any):
    """Print raw JSON with a label for verification."""
    print(f"\n--- RAW JSON: {label} ---")
    print(json.dumps(data, indent=2))


def test_signature_on_function_call() -> dict:
    """
    Test 1: Verify thoughtSignature appears on functionCall parts.

    Expected: Response contains functionCall part WITH thoughtSignature field.
    """
    print("\n" + "=" * 70)
    print("TEST 1: thoughtSignature on functionCall parts")
    print("=" * 70)

    contents = [
        {"role": "user", "parts": [{"text": "What's the weather in Tokyo?"}]}
    ]

    status, response = make_request(
        contents, tools=get_weather_tool(), thinking_config={"thinkingBudget": 1024}
    )

    print(f"\nStatus: {status}")
    print_raw_json("Full response", response)

    result = {
        "test": "signature_on_function_call",
        "status": status,
        "passed": False,
        "details": {},
    }

    if status != 200:
        result["details"]["error"] = response.get("error", {}).get("message", "Unknown")
        return result

    # Extract parts from response
    candidates = response.get("candidates", [])
    if not candidates:
        result["details"]["error"] = "No candidates in response"
        return result

    parts = candidates[0].get("content", {}).get("parts", [])
    result["details"]["part_count"] = len(parts)

    # Find functionCall parts and check for signature
    function_call_parts = []
    for i, part in enumerate(parts):
        if "functionCall" in part:
            function_call_parts.append(
                {
                    "index": i,
                    "name": part["functionCall"].get("name"),
                    "has_signature": "thoughtSignature" in part,
                    "signature_preview": (
                        part.get("thoughtSignature", "")[:50] + "..."
                        if part.get("thoughtSignature")
                        else None
                    ),
                }
            )

    result["details"]["function_calls"] = function_call_parts

    if function_call_parts and function_call_parts[0]["has_signature"]:
        result["passed"] = True
        print("\nRESULT: PASS - functionCall part has thoughtSignature")
    elif not function_call_parts:
        result["details"]["error"] = "No functionCall in response (model may have answered directly)"
        print("\nRESULT: SKIP - No function call triggered")
    else:
        print("\nRESULT: FAIL - functionCall present but NO thoughtSignature")

    return result


def test_signature_on_text_parts() -> dict:
    """
    Test 2: Verify thoughtSignature can appear on text parts for non-tool turns.

    Expected: For continuity, Gemini may put signature on trailing text part.
    """
    print("\n" + "=" * 70)
    print("TEST 2: thoughtSignature on text parts (non-tool response)")
    print("=" * 70)

    # Simple question that shouldn't need tools
    contents = [
        {"role": "user", "parts": [{"text": "What is 2 + 2? Just give the number."}]}
    ]

    # Enable thinking to potentially get signatures on text parts
    status, response = make_request(
        contents, tools=None, thinking_config={"thinkingBudget": 512}
    )

    print(f"\nStatus: {status}")
    print_raw_json("Full response", response)

    result = {
        "test": "signature_on_text_parts",
        "status": status,
        "passed": False,
        "details": {},
    }

    if status != 200:
        result["details"]["error"] = response.get("error", {}).get("message", "Unknown")
        return result

    candidates = response.get("candidates", [])
    if not candidates:
        result["details"]["error"] = "No candidates in response"
        return result

    parts = candidates[0].get("content", {}).get("parts", [])
    result["details"]["part_count"] = len(parts)

    # Check text parts for signature
    text_parts_with_sig = []
    for i, part in enumerate(parts):
        if "text" in part:
            has_sig = "thoughtSignature" in part
            text_parts_with_sig.append(
                {
                    "index": i,
                    "text_preview": part["text"][:50] + "..." if len(part["text"]) > 50 else part["text"],
                    "has_signature": has_sig,
                    "signature_preview": (
                        part.get("thoughtSignature", "")[:50] + "..."
                        if part.get("thoughtSignature")
                        else None
                    ),
                }
            )

    result["details"]["text_parts"] = text_parts_with_sig

    # According to spec: "Non-tool turns: Signature may appear on text part (even empty text)"
    # This is optional behavior, so we mark as passed if we got text parts
    # and note whether signature was present
    has_text = len(text_parts_with_sig) > 0
    has_sig_on_text = any(p["has_signature"] for p in text_parts_with_sig)

    if has_sig_on_text:
        result["passed"] = True
        print("\nRESULT: PASS - text part has thoughtSignature (confirms spec claim)")
    elif has_text:
        result["passed"] = True  # Still pass - signature on text is optional per spec
        result["details"]["note"] = "Text parts present but no signature (signature on text is optional)"
        print("\nRESULT: PASS (with note) - Text returned, signature optional for non-tool turns")
    else:
        result["details"]["error"] = "No text parts in response"
        print("\nRESULT: FAIL - No text parts returned")

    return result


def test_parallel_calls_signature() -> dict:
    """
    Test 3: Verify only FIRST parallel functionCall has signature.

    Expected: When model makes multiple parallel calls, only the first has thoughtSignature.
    """
    print("\n" + "=" * 70)
    print("TEST 3: Parallel calls - only FIRST has signature")
    print("=" * 70)

    contents = [
        {
            "role": "user",
            "parts": [
                {
                    "text": "I need to know the weather, current time, and population for Tokyo. Please get all three pieces of information at once."
                }
            ],
        }
    ]

    status, response = make_request(
        contents, tools=get_multi_tools(), thinking_config={"thinkingBudget": 2048}
    )

    print(f"\nStatus: {status}")
    print_raw_json("Full response", response)

    result = {
        "test": "parallel_calls_first_only",
        "status": status,
        "passed": False,
        "details": {},
    }

    if status != 200:
        result["details"]["error"] = response.get("error", {}).get("message", "Unknown")
        return result

    candidates = response.get("candidates", [])
    if not candidates:
        result["details"]["error"] = "No candidates in response"
        return result

    parts = candidates[0].get("content", {}).get("parts", [])
    result["details"]["part_count"] = len(parts)

    # Analyze all functionCall parts
    function_calls = []
    for i, part in enumerate(parts):
        if "functionCall" in part:
            function_calls.append(
                {
                    "index": i,
                    "name": part["functionCall"].get("name"),
                    "has_signature": "thoughtSignature" in part,
                }
            )

    result["details"]["function_calls"] = function_calls
    result["details"]["total_calls"] = len(function_calls)

    if len(function_calls) < 2:
        result["details"]["error"] = f"Need 2+ parallel calls, got {len(function_calls)}"
        print(f"\nRESULT: SKIP - Need multiple parallel calls, got {len(function_calls)}")
        return result

    # Check pattern: first has sig, rest don't
    first_has_sig = function_calls[0]["has_signature"]
    others_have_sig = any(fc["has_signature"] for fc in function_calls[1:])
    calls_with_sig = sum(1 for fc in function_calls if fc["has_signature"])

    result["details"]["first_has_signature"] = first_has_sig
    result["details"]["others_have_signature"] = others_have_sig
    result["details"]["calls_with_signature"] = calls_with_sig

    print("\nSignature distribution:")
    for i, fc in enumerate(function_calls):
        print(f"  Call {i + 1} ({fc['name']}): {'HAS' if fc['has_signature'] else 'NO'} signature")

    if first_has_sig and not others_have_sig:
        result["passed"] = True
        print("\nRESULT: PASS - Only FIRST parallel call has signature (confirms spec)")
    elif not first_has_sig:
        print("\nRESULT: FAIL - First call doesn't have signature")
    else:
        print(f"\nRESULT: FAIL - {calls_with_sig} of {len(function_calls)} calls have signature")

    return result


def test_signature_required_for_gemini3() -> dict:
    """
    Test 4: Verify Gemini 3 models REQUIRE signature (400 error without).

    This test creates a conversation with thinking, gets a function call with signature,
    then attempts to continue WITHOUT the signature - should fail with 400.
    """
    print("\n" + "=" * 70)
    print("TEST 4: Gemini 3 REQUIRES signature (400 without)")
    print("=" * 70)

    # Step 1: Get a function call with signature
    print("\nStep 1: Getting initial function call with signature...")
    contents = [
        {"role": "user", "parts": [{"text": "What's the weather in Paris?"}]}
    ]

    status, response = make_request(
        contents, tools=get_weather_tool(), thinking_config={"thinkingBudget": 1024}
    )

    if status != 200:
        print(f"Setup failed with status {status}")
        return {
            "test": "signature_required",
            "status": status,
            "passed": False,
            "details": {"error": "Setup request failed"},
        }

    candidates = response.get("candidates", [])
    if not candidates:
        return {
            "test": "signature_required",
            "status": status,
            "passed": False,
            "details": {"error": "No candidates in setup response"},
        }

    parts = candidates[0].get("content", {}).get("parts", [])

    # Find function call with signature
    fc_part = None
    signature = None
    for part in parts:
        if "functionCall" in part:
            fc_part = part
            signature = part.get("thoughtSignature")
            break

    if not fc_part:
        return {
            "test": "signature_required",
            "status": status,
            "passed": False,
            "details": {"error": "No function call in setup response"},
        }

    if not signature:
        print("WARNING: Setup response had no signature - model may not have triggered thinking")
        return {
            "test": "signature_required",
            "status": status,
            "passed": False,
            "details": {"error": "No signature in setup response"},
        }

    print(f"Got function call: {fc_part['functionCall']['name']}")
    print(f"Got signature: {signature[:50]}...")

    # Step 2: Try to continue WITHOUT signature (should fail)
    print("\nStep 2: Attempting to continue WITHOUT signature (should fail)...")

    # Build conversation WITHOUT signature on model's function call
    contents_no_sig = [
        {"role": "user", "parts": [{"text": "What's the weather in Paris?"}]},
        {
            "role": "model",
            "parts": [
                {
                    "functionCall": fc_part["functionCall"]
                    # NO thoughtSignature here
                }
            ],
        },
        {
            "role": "user",
            "parts": [
                {
                    "functionResponse": {
                        "name": fc_part["functionCall"]["name"],
                        "response": {"weather": "Sunny, 18C"},
                    }
                }
            ],
        },
    ]

    status_no_sig, response_no_sig = make_request(
        contents_no_sig, tools=get_weather_tool()
    )

    print(f"\nStatus without signature: {status_no_sig}")
    print_raw_json("Response without signature", response_no_sig)

    # Step 3: Try WITH signature (should succeed)
    print("\nStep 3: Attempting to continue WITH signature (should succeed)...")

    contents_with_sig = [
        {"role": "user", "parts": [{"text": "What's the weather in Paris?"}]},
        {
            "role": "model",
            "parts": [fc_part],  # Includes thoughtSignature
        },
        {
            "role": "user",
            "parts": [
                {
                    "functionResponse": {
                        "name": fc_part["functionCall"]["name"],
                        "response": {"weather": "Sunny, 18C"},
                    }
                }
            ],
        },
    ]

    status_with_sig, response_with_sig = make_request(
        contents_with_sig, tools=get_weather_tool()
    )

    print(f"\nStatus with signature: {status_with_sig}")
    print_raw_json("Response with signature", response_with_sig)

    result = {
        "test": "signature_required",
        "status": status,
        "passed": False,
        "details": {
            "status_without_sig": status_no_sig,
            "status_with_sig": status_with_sig,
        },
    }

    # Expected: without sig = 400, with sig = 200
    if status_no_sig == 400 and status_with_sig == 200:
        result["passed"] = True
        print("\nRESULT: PASS - Signature REQUIRED (400 without, 200 with)")
    elif status_no_sig == 200 and status_with_sig == 200:
        result["details"]["note"] = "Both succeeded - signature may be optional for this model/config"
        print("\nRESULT: FAIL - Both requests succeeded (signature not required?)")
    else:
        print(f"\nRESULT: UNEXPECTED - without={status_no_sig}, with={status_with_sig}")

    return result


def main():
    """Run all verification tests."""
    print("=" * 70)
    print("GEMINI THOUGHT SIGNATURE VERIFICATION")
    print(f"Model: {MODEL}")
    print("=" * 70)

    results = []

    # Run all tests
    results.append(test_signature_on_function_call())
    results.append(test_signature_on_text_parts())
    results.append(test_parallel_calls_signature())
    results.append(test_signature_required_for_gemini3())

    # Final summary
    print("\n" + "=" * 70)
    print("FINAL SUMMARY")
    print("=" * 70)

    for r in results:
        status = "PASS" if r["passed"] else "FAIL"
        print(f"\n{r['test']}: {status}")
        if r.get("details"):
            if "error" in r["details"]:
                print(f"  Error: {r['details']['error']}")
            if "note" in r["details"]:
                print(f"  Note: {r['details']['note']}")

    passed = sum(1 for r in results if r["passed"])
    total = len(results)
    print(f"\n{'=' * 70}")
    print(f"TOTAL: {passed}/{total} tests passed")
    print("=" * 70)

    # Exit with non-zero if any failed
    sys.exit(0 if passed == total else 1)


if __name__ == "__main__":
    main()
