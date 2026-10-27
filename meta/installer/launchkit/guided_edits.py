"""Bounded transient edits over a complete, host-bound guided design.

These inputs are authoring shorthand. Only the ordinary Blueprint and brief
survive validation; edits do not define another configuration or authority.
"""
from __future__ import annotations

import copy
import json
import re

from .blueprint import Blueprint
from .guide_models import DesignBrief, GuidePublication, GuideQuestion
from .guide_store import MAX_STATE_BYTES
from .projects import fingerprint

MAX_GUIDED_EDITS = 128
TOKEN_CONTEXT_FIELDS = (
    "guide_id", "guide_version", "action", "current_stage", "current_status",
    "current_question", "saved_plan_available", "manual_plan_submitted",
    "current_user_message", "evidence_messages", "manual_edits", "reopened_topics",
)


def guided_document(blueprint, brief):
    return {"blueprint": Blueprint.model_validate(blueprint).model_dump(mode="json"),
            "brief": DesignBrief.model_validate({} if brief is None else brief).model_dump(mode="json")}


def guided_edit_token(context, blueprint):
    """Conflict binding, not authorization, a signature or a saved revision."""
    if (not isinstance(context.get("guide_id"), str) or not context["guide_id"]
            or type(context.get("guide_version")) is not int or context["guide_version"] < 1
            or not context.get("current_user_message", {}).get("id")):
        raise ValueError("A grounded edit needs the current saved guide version and user message.")
    return fingerprint({"contract": "guided-edit-v1",
        "context": {key: context.get(key) for key in TOKEN_CONTEXT_FIELDS},
        "document": guided_document(blueprint, context.get("brief"))})


def guided_edit_schema():
    question = GuideQuestion.model_json_schema()
    definitions = question.pop("$defs", {})
    fields = GuidePublication.model_json_schema()["properties"]
    properties = {key: fields[key] for key in ("publication_mode", "message", "summary", "visualization_feedback", "warnings")}
    properties.update(base_token={"type": "string", "pattern": "^[a-f0-9]{64}$"},
        question={"anyOf": [question, {"type": "null"}]},
        edits={"type": "array", "maxItems": MAX_GUIDED_EDITS, "items": {"oneOf": [
            {"type": "object", "properties": {"op": {"enum": ["add", "replace"]},
                "path": {"type": "string", "minLength": 1}, "value": {}},
             "required": ["op", "path", "value"], "additionalProperties": False},
            {"type": "object", "properties": {"op": {"const": "remove"},
                "path": {"type": "string", "minLength": 1}},
             "required": ["op", "path"], "additionalProperties": False}]}})
    return {"type": "object", "properties": properties, "$defs": definitions,
            "required": ["base_token", "edits", "question", "message"], "additionalProperties": False}


def _bounded_json(value):
    try:
        encoded = json.dumps(value, ensure_ascii=False, allow_nan=False).encode("utf-8")
    except (TypeError, ValueError, RecursionError):
        raise ValueError("Guided edits must contain finite JSON values.") from None
    if len(encoded) > MAX_STATE_BYTES:
        raise ValueError("The guided edit is too large. Use a smaller update or full authoring.")


def _path(value):
    if not isinstance(value, str) or not value.startswith("/"):
        raise ValueError("Use a JSON pointer within /blueprint or /brief.")
    parts = value[1:].split("/")
    if any(re.search(r"~(?![01])", part) for part in parts):
        raise ValueError("Use only ~0 and ~1 escapes in JSON pointers.")
    parts = [part.replace("~1", "/").replace("~0", "~") for part in parts]
    if parts[0] not in {"blueprint", "brief"}:
        raise ValueError("Guided edits may change only /blueprint or /brief.")
    return parts


def _index(part, length, *, add=False):
    if add and part == "-":
        return length
    if not re.fullmatch(r"0|[1-9][0-9]*", part):
        raise ValueError("Use an exact array index, or - to append with add.")
    # Avoid converting an unbounded digit sequence before checking its range.
    if len(part) > len(str(length)):
        raise ValueError("The guided edit array index is outside the current document.")
    index = int(part)
    limit = length if add else length - 1
    if index > limit:
        raise ValueError("The guided edit array index is outside the current document.")
    return index


def apply_guided_edits(document, edits):
    """Apply ordered add/replace/remove on a copy; never infer missing parents."""
    if not isinstance(edits, list) or len(edits) > MAX_GUIDED_EDITS:
        raise ValueError(f"Use at most {MAX_GUIDED_EDITS} guided edit operations.")
    _bounded_json(edits)
    candidate = copy.deepcopy(document)
    for edit in edits:
        if (not isinstance(edit, dict) or not isinstance(edit.get("op"), str)
                or edit["op"] not in {"add", "replace", "remove"}):
            raise ValueError("Use add, replace or remove guided edits.")
        op = edit["op"]
        if set(edit) != ({"op", "path"} if op == "remove" else {"op", "path", "value"}):
            raise ValueError("Supply exactly op/path and a value only for add or replace.")
        parts = _path(edit["path"])
        parent = candidate
        for part in parts[:-1]:
            if isinstance(parent, dict) and part in parent:
                parent = parent[part]
            elif isinstance(parent, list):
                parent = parent[_index(part, len(parent))]
            else:
                raise ValueError("The guided edit parent path does not exist.")
        key = parts[-1]
        if isinstance(parent, dict):
            if op != "add" and key not in parent:
                raise ValueError("The guided edit target does not exist.")
            if op == "remove":
                del parent[key]
            else:
                parent[key] = copy.deepcopy(edit["value"])
        elif isinstance(parent, list):
            index = _index(key, len(parent), add=op == "add")
            if op == "add":
                parent.insert(index, copy.deepcopy(edit["value"]))
            elif op == "replace":
                parent[index] = copy.deepcopy(edit["value"])
            else:
                parent.pop(index)
        else:
            raise ValueError("The guided edit target parent is not an object or array.")
    if set(candidate) != {"blueprint", "brief"}:
        raise ValueError("Keep the complete Blueprint and brief in the guided document.")
    if not isinstance(candidate["brief"], dict):
        raise ValueError("Keep the guided brief as an object.")
    _bounded_json(candidate)
    return guided_document(candidate["blueprint"], candidate["brief"])
