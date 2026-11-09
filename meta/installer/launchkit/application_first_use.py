"""Bounded authored first jobs from exact installed source, without execution.

These optional suggestions are presentation data. They do not reuse offline
fixtures, authorize native admission or attest a completed application outcome.
"""
from __future__ import annotations

import hashlib
import json
import math
import re

from .application_experiences import declared_application_experiences


FIRST_USE_PATH = "application/first-use.json"
MAX_METADATA_BYTES = 40_000
MAX_TASKS = 8
SCOPE = "authored-first-use-suggestions"


def _content(files):
    if isinstance(files, dict):
        value = files.get(FIRST_USE_PATH)
        if isinstance(value, tuple):
            if len(value) != 2:
                raise ValueError("First-task suggestions need ordinary source bytes.")
            value = value[0]
    else:
        items = [item for item in files if item.path == FIRST_USE_PATH]
        if len(items) > 1:
            raise ValueError("The installed source contains duplicate first-task metadata.")
        value = items[0].content if items else None
    if value is None:
        return None
    if isinstance(value, str):
        value = value.encode("utf-8")
    if not isinstance(value, bytes):
        raise ValueError("First-task suggestions need ordinary UTF-8 source bytes.")
    return value


def _json(content):
    if len(content) > MAX_METADATA_BYTES:
        raise ValueError("First-task suggestions exceed their 40 KB source limit.")
    def finite(_):
        raise ValueError("First-task suggestions must contain finite ordinary JSON.")
    def number(value):
        result = float(value)
        if not math.isfinite(result):
            finite(value)
        return result
    def unique(pairs):
        result = {}
        for key, value in pairs:
            if key in result:
                raise ValueError("First-task suggestions contain duplicate JSON fields.")
            result[key] = value
        return result
    try:
        return json.loads(content.decode("utf-8"), parse_constant=finite,
                          parse_float=number, object_pairs_hook=unique)
    except (UnicodeError, json.JSONDecodeError, RecursionError):
        raise ValueError("First-task suggestions need bounded ordinary UTF-8 JSON.") from None


def _text(value, label, maximum, *, multiline=False, normalize=True):
    if (not isinstance(value, str) or not value.strip() or len(value) > maximum
            or any(ord(char) == 127 or ord(char) < 32 and
                   (not multiline or char not in "\n\r\t") for char in value)):
        raise ValueError(f"Each first task needs {label} of 1-{maximum} plain-text characters.")
    # Escaped lone surrogates are valid JSON syntax but cannot be ordinary UTF-8
    # source text copied by the browser. Reject them without echoing the value.
    try:
        value.encode("utf-8")
    except UnicodeError:
        raise ValueError("First-task suggestions need ordinary UTF-8 text.") from None
    return value.strip() if normalize else value


def first_use_suggestions(files, definition=None):
    """Read optional source suggestions without app imports or private data.

    The caller supplies checked installed package files, either PackageFile
    objects or a deployment package mapping. Invalid optional metadata yields
    no partial choices and a safe reason; ordinary Console use remains possible.
    An optional entry_id declares support for the versioned local fragment draft
    handoff. It references this package's checked experience metadata, never an
    arbitrary URL or evidence of interface availability or native admission.
    """
    result = {"scope": SCOPE, "metadata_sha256": None, "tasks": [], "entry_id": None, "detail": None}
    try:
        content = _content(files)
        if content is None:
            result["detail"] = ("This installed application has no authored first-task suggestions. "
                                "You can describe your own job.")
            return result
        result["metadata_sha256"] = hashlib.sha256(content).hexdigest()
        value = _json(content)
        if not isinstance(value, dict) or "tasks" not in value or set(value) - {"tasks", "entry_id"}:
            raise ValueError("First-task metadata declares only tasks and an optional entry_id.")
        tasks = value["tasks"]
        if not isinstance(tasks, list) or not 1 <= len(tasks) <= MAX_TASKS:
            raise ValueError("First-task metadata needs 1-8 authored jobs.")
        parsed = []
        for item in tasks:
            if (not isinstance(item, dict) or not {"label", "request"} <= set(item)
                    or set(item) - {"label", "request", "description", "inputs", "prerequisites",
                                    "expected_result", "result_location"}):
                raise ValueError("A first task declares a label, request and optional plain walkthrough details.")
            row = {"label": _text(item["label"], "a label", 120),
                   "request": _text(item["request"], "a request", 3000, multiline=True, normalize=False)}
            if "description" in item:
                row["description"] = _text(item["description"], "a description", 400, multiline=True)
            for key in ("inputs", "prerequisites"):
                if key in item:
                    values = item[key]
                    if not isinstance(values, list) or len(values) > 8:
                        raise ValueError("A first-task walkthrough accepts up to eight plain items per list.")
                    row[key] = [_text(value, "a walkthrough item", 400, multiline=True) for value in values]
            for key, maximum in (("expected_result", 600), ("result_location", 400)):
                if key in item:
                    row[key] = _text(item[key], "a walkthrough detail", maximum, multiline=True)
            parsed.append(row)
        result["tasks"] = parsed
        result["detail"] = ("Authored first-task suggestions describe possible jobs. "
                            "They do not prove installed execution or a completed outcome.")
        if "entry_id" in value:
            try:
                identity = value["entry_id"]
                if not isinstance(identity, str) or not re.fullmatch(r"[a-z][a-z0-9_-]{0,63}", identity):
                    raise ValueError("Choose a bounded declared application entry ID.")
                if definition is not None and not isinstance(definition, dict):
                    raise ValueError("Choose the checked package definition.")
                declared = declared_application_experiences(files, definition if definition is not None else {})
                if sum(row["id"] == identity for row in declared["entries"]) != 1:
                    raise ValueError("Choose one exact entry in this package's checked interface metadata.")
                result["entry_id"] = identity
                result["detail"] += (" The author declares that this entry supports a versioned local URL fragment "
                    "draft handoff. That declaration does not prove availability, authentication or native admission.")
            except (ValueError, TypeError, IndexError, KeyError, RecursionError, UnicodeError):
                result["detail"] += (" Its draft handoff could not be matched to checked interface metadata. "
                    "Copy your task into the application manually.")
    except (ValueError, TypeError, RecursionError, UnicodeError):
        result["tasks"] = []
        result["entry_id"] = None
        result["detail"] = "First-task suggestions could not be read. You can still describe your own job."
    return result
