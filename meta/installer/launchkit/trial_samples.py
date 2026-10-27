"""Read bounded authored trial suggestions without importing application code."""
from __future__ import annotations

import json
import re

MAX_SAMPLE_BYTES = 100_000
MAX_SAMPLE_CASES = 12
SAMPLE_PATH = "application/trial.json"


def trial_samples(files, source_digest):
    """Project only suggestions from this exact checked source snapshot.

    Labels, requests and fixtures are authored sample data. They neither confer
    tool authority nor attest execution, acceptance or a connected service.
    """
    if not isinstance(source_digest, str) or not re.fullmatch(r"[0-9a-f]{64}", source_digest):
        raise ValueError("Trial samples need the exact checked source digest.")
    content = next((item.content for item in files if item.path == SAMPLE_PATH), None)
    result = {"source_digest": source_digest, "cases": [], "detail": None,
              "scope": "Authored sample suggestions from checked source, not execution or acceptance evidence."}
    if content is None:
        result["detail"] = "This application has no authored sample choices. You can enter a request or ask the builder to prepare useful samples."
        return result
    try:
        if len(content.encode("utf-8")) > MAX_SAMPLE_BYTES:
            raise ValueError("Application sample choices must fit within 100 KB.")
        value = json.loads(content, parse_constant=lambda name: (_ for _ in ()).throw(ValueError("Sample values must be finite JSON.")))
        cases = value.get("cases") if isinstance(value, dict) else None
        if not isinstance(cases, list) or not 1 <= len(cases) <= MAX_SAMPLE_CASES:
            raise ValueError("Application sample choices need 1-12 cases.")
        for case in cases:
            if not isinstance(case, dict):
                raise ValueError("Each application sample must be an ordinary object.")
            label, request, fixtures = case.get("label"), case.get("request"), case.get("fixtures", {})
            if not isinstance(label, str) or not label.strip() or len(label) > 160 or any(ord(char) < 32 for char in label):
                raise ValueError("Each sample needs a plain-language label of 1-160 visible characters.")
            if not isinstance(request, str) or not request.strip() or len(request) > 6000:
                raise ValueError("Each sample needs a realistic request of 1-6000 characters.")
            if not isinstance(fixtures, dict):
                raise ValueError("Sample fixtures must be an ordinary JSON object.")
            # Also reject numeric overflow, which json.loads represents as
            # infinity even though the original number token was valid JSON.
            encoded = json.dumps(fixtures, ensure_ascii=True, allow_nan=False)
            if len(encoded.encode()) > MAX_SAMPLE_BYTES:
                raise ValueError("Sample fixtures must fit within 100 KB.")
            result["cases"].append({"label": label.strip(), "request": request.strip(), "fixtures": json.loads(encoded)})
    except (ValueError, TypeError, RecursionError, UnicodeError) as error:
        result["cases"] = []
        result["detail"] = str(error)[:600]
    return result
