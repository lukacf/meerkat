"""Bounded public Console observations for an already-admitted animation request.

These reads never start the setup runtime, dispatch work, or publish an artifact.
Host operation admission, native admission, publication and terminality stay separate.
"""
from __future__ import annotations

import hashlib
import asyncio
import json
import math
import re

from .guide_timeline import cursor_number
from .projects import fingerprint

MAX_RECORD_BYTES = 256_000
MAX_NATIVE_PAGE_BYTES = 8_000_000
MAX_PAGE_BYTES = 64_000
MAX_TEXT_BYTES = 6_000
HASH = re.compile(r"[0-9a-f]{64}\Z")
AUTHOR_ID = re.compile(r"visualizer-request-[0-9a-f]{32}\Z")
BINDINGS = ("id", "setup_id", "stream_id", "request_fingerprint", "blueprint_hash", "model_hash", "native_request_key", "source_generation")
TERMINALS = {"interaction_complete": "completed", "interaction_failed": "failed",
             "interaction_canceled": "interrupted", "interaction_cancelled": "interrupted"}
COMPACTIONS = {"compaction_started", "compaction_completed", "compaction_failed"}
TOOLS = {"inspect_visualization", "read_visualization_examples", "publish_visualization"}


def checked_hash(value):
    if not isinstance(value, str) or not HASH.fullmatch(value):
        raise ValueError("Invalid visualization source binding.")
    return value


def checked_cursor(value):
    result = cursor_number(value)
    if result < 0:
        raise ValueError("Invalid native conversation cursor.")
    return result


def _identifier(value):
    return isinstance(value, str) and 0 < len(value) <= 256 and all(c.isprintable() for c in value)


def checked_admission(value):
    if not isinstance(value, dict) or not _identifier(value.get("interaction_id")):
        raise ValueError("Invalid saved native admission.")
    checked_cursor(value.get("cursor"))
    result = {"interaction_id": value["interaction_id"], "cursor": value["cursor"]}
    for name in ("identity", "session_id", "input_frame_id", "conversation_id"):
        if value.get(name) is not None:
            if not _identifier(value[name]):
                raise ValueError("Invalid saved native admission.")
            result[name] = value[name]
    return result


def checked_author_owner(value):
    """Keep a fresh request's native member/session separate from source identity."""
    if not isinstance(value, dict):
        raise ValueError("Invalid saved visualization author.")
    admission = checked_admission(value["admission"]) if value.get("admission") is not None else None
    identity, session = value.get("author_identity"), value.get("author_session_id")
    if identity is None:
        admitted_identity = (admission or {}).get("identity")
        if session is not None or isinstance(admitted_identity, str) and AUTHOR_ID.fullmatch(admitted_identity):
            raise ValueError("Invalid saved visualization author.")
        return {}
    if not isinstance(identity, str) or not AUTHOR_ID.fullmatch(identity):
        raise ValueError("Invalid saved visualization author.")
    result = {"author_identity": identity}
    if session is not None:
        if not _identifier(session):
            raise ValueError("Invalid saved visualization author session.")
        result["author_session_id"] = session
    if admission is not None:
        if session is None or admission.get("identity") != identity or admission.get("session_id") != session:
            raise ValueError("The saved admission does not belong to this visualization author.")
    return result


def checked_revision_binding(value):
    """Validate the versioned scope/hash of a host-frozen prior-scene context."""
    if (not isinstance(value, dict) or set(value) != {"version", "setup_id", "stream_id", "context_hash"}
            or value.get("version") != "prior-program-v1"
            or not _identifier(value.get("stream_id"))
            or (value.get("setup_id") is not None and not _identifier(value["setup_id"]))):
        raise ValueError("Invalid saved animation revision binding.")
    checked_hash(value.get("context_hash"))
    return {name: value[name] for name in ("version", "setup_id", "stream_id", "context_hash")}


def checked_author_reference_delivery(value, request_key, reference_hash):
    """Optional host image-result metadata, not provider delivery or perception."""
    try:
        if (not isinstance(value, dict)
                or set(value) != {"observation", "request_key", "source_reference_hash", "images"}
                or value["observation"] != "host_returned_tool_content"
                or checked_hash(value["request_key"]) != request_key
                or checked_hash(value["source_reference_hash"]) != reference_hash
                or not isinstance(value["images"], list) or not 1 <= len(value["images"]) <= 2):
            return None
        images, seen = [], set()
        for image in value["images"]:
            if (not isinstance(image, dict) or set(image) != {"id", "png_hash"}
                    or image["id"] not in {"household", "engineering-organisation"}
                    or image["id"] in seen):
                return None
            seen.add(image["id"])
            images.append({"id": image["id"], "png_hash": checked_hash(image["png_hash"])})
        return {"observation": value["observation"], "request_key": request_key,
                "source_reference_hash": reference_hash, "images": images}
    except (KeyError, TypeError, ValueError):
        return None


def checked_request_record(value):
    """Validate minimal host-saved source/admission evidence, never a client claim."""
    if not isinstance(value, dict):
        raise ValueError("Invalid saved visualization request.")
    record = {name: value[name] for name in ("key", "contract", "blueprint_hash", "model_hash", "reference_hash")}
    if record["contract"] != "freeform-animation-v3":
        raise ValueError("Unsupported saved visualization request.")
    for name in ("key", "blueprint_hash", "model_hash", "reference_hash"):
        checked_hash(record[name])
    if value.get("feedback_hash"):
        record["feedback_hash"] = checked_hash(value["feedback_hash"])
    if value.get("revision_binding") is not None:
        record["revision_binding"] = checked_revision_binding(value["revision_binding"])
    if fingerprint({name: item for name, item in record.items() if name != "key"}) != record["key"]:
        raise ValueError("The saved visualization source does not match its request.")
    record.update(checked_author_owner(value))
    delivery = checked_author_reference_delivery(value.get("author_reference_delivery"), record["key"], record["reference_hash"])
    if delivery is not None:
        record["author_reference_delivery"] = delivery
    admission = value.get("admission")
    if admission is not None:
        record["admission"] = checked_admission(admission)
    native_run = value.get("native_run")
    if isinstance(native_run, dict) and record.get("admission"):
        if native_run.get("interaction_id") == record["admission"]["interaction_id"] and native_run.get("status") in {"completed", "failed", "interrupted"}:
            terminal = native_run.get("terminal_cursor")
            if terminal is not None and checked_cursor(terminal) >= checked_cursor(record["admission"]["cursor"]):
                record["native_run"] = {"status": native_run["status"], "interaction_id": native_run["interaction_id"], "terminal_cursor": terminal}
    return record


def read_record(path, *, max_bytes=MAX_RECORD_BYTES):
    if path.is_symlink() or path.stat().st_size > max_bytes:
        raise ValueError("Invalid saved visualization request.")
    return json.loads(path.read_text(encoding="utf-8"))


def _text(value):
    if not isinstance(value, str):
        return None
    data = value.encode("utf-8", errors="strict")
    return data[:MAX_TEXT_BYTES].decode("utf-8", errors="ignore"), len(data) > MAX_TEXT_BYTES


def public_frame(frame, admission, *, depth=0):
    """Allow public text/summaries and native lifecycle, excluding input and tool data.

    Generic reasoning deltas can contain raw reasoning. Only explicitly named
    Console summary events or a summary field have the public summary meaning.
    Tool arguments/results can contain the entire private frozen proposal or
    generated program; retain their native names/IDs and safe publication fields.
    """
    if depth > 2 or not isinstance(frame, dict) or frame.get("interaction_id") != admission["interaction_id"]:
        return None
    if frame.get("session_id") is not None and admission.get("session_id") is not None and frame["session_id"] != admission["session_id"]:
        return None
    if frame.get('identity') is not None and frame['identity'] != admission.get('identity', 'visualizer:main'):
        return None
    cursor = frame.get("cursor")
    if checked_cursor(cursor) < checked_cursor(admission["cursor"]):
        return None
    kind, payload = frame.get("kind"), frame.get("payload")
    if not isinstance(payload, dict):
        return None
    result = {"cursor": cursor, "kind": kind, "interaction_id": admission["interaction_id"], "payload": {}}
    for name in ("id", "session_id", "agent_id", "identity", "turn_id", "run_id", "conversation_id", "source_event_id", "parent_frame_id", "caused_by_frame_id"):
        if _identifier(frame.get(name)):
            result[name] = frame[name]
    for name in ("timestamp_ms", "sequence", "frame_version", "updated_at_ms"):
        value = frame.get(name)
        if type(value) in (int, float) and math.isfinite(value):
            result[name] = value
    if frame.get('status') in {'accepted', 'dispatching', 'delivered', 'delivery_failed', 'completed', 'redacted'}:
        result['status'] = frame['status']
    source = frame.get('source')
    if isinstance(source, dict) and source.get('kind') in {'console_event', 'session_history', 'send', 'synthetic'}:
        result['source'] = {'kind': source['kind']}
        if _identifier(source.get('source_cursor')):
            result['source']['source_cursor'] = source['source_cursor']
    output = result["payload"]
    if kind == "frame_updated":
        inner = public_frame(payload.get("frame"), admission, depth=depth + 1)
        if inner is None:
            return None
        output["frame"] = inner
    elif kind in {"reasoning_summary", "reasoning_summary_delta", "reasoning_summary_complete", "reasoning_delta", "reasoning_complete", "text_delta", "text_complete"}:
        if kind.startswith("reasoning"):
            candidates = ("summary", "text", "delta", "content") if kind.startswith("reasoning_summary") else ("summary",)
        else:
            candidates = ("delta", "text", "content")
        pair = next((_text(payload[name]) for name in candidates if isinstance(payload.get(name), str)), None)
        if pair is None:
            return None
        # Retain the actual Console text value; normalization only presents its
        # documented public summary field to the existing transcript renderer.
        field = "delta" if kind.endswith("delta") else "text"
        output[field], truncated = pair
        if kind.startswith("reasoning"):
            output["summary"] = output[field]
        if truncated:
            result["projection_truncated"] = True
    elif kind in COMPACTIONS:
        # Native markers identify context-preservation text without inspecting
        # its content. Only public lifecycle counters cross this boundary.
        for name in ("input_tokens", "estimated_history_tokens", "message_count",
                     "summary_tokens", "messages_before", "messages_after"):
            value = payload.get(name)
            if type(value) is int and value >= 0:
                output[name] = value
    elif kind in {"tool_call_requested", "tool_call", "tool_execution_started", "tool_execution_completed", "tool_result_received"}:
        name = payload.get("name", payload.get("tool_name"))
        if name not in TOOLS:
            return None
        output.update(name=name, tool_name=name)
        for key in ("tool_call_id", "id"):
            if _identifier(payload.get(key)):
                output[key] = payload[key]
        if type(payload.get("is_error")) is bool:
            output["is_error"] = payload["is_error"]
        if type(payload.get("duration_ms")) in (int, float) and math.isfinite(payload["duration_ms"]):
            output["duration_ms"] = payload["duration_ms"]
        if name == "publish_visualization" and isinstance(payload.get("result"), dict):
            published = payload["result"]
            output["result"] = {key: published[key] for key in ("request_key", "code_hash") if isinstance(published.get(key), str) and HASH.fullmatch(published[key])}
            if type(published.get("valid")) is bool:
                output["result"]["valid"] = published["valid"]
    elif kind not in {*TERMINALS, "run_started", "run_completed", "run_failed", "interaction_started"}:
        return None
    elif kind in {*TERMINALS, 'run_completed'} and isinstance(payload.get('text'), str):
        output['text'], truncated = _text(payload['text'])
        if truncated:
            result['projection_truncated'] = True
    return result


def publication(receipt):
    artifact = receipt.get("artifact")
    if not isinstance(artifact, dict):
        return {"available": False}
    evidence = artifact.get("publication_receipt")
    if (not isinstance(evidence, dict) or artifact.get("key") != receipt.get("native_request_key")
            or any(artifact.get(name) != receipt.get(name) for name in ("blueprint_hash", "model_hash"))
            or evidence.get("authority") not in {"publish_visualization", "native_publish_acknowledgment"}
            or evidence.get("request_key") != artifact["key"]
            or any(evidence.get(name) != artifact.get(name) for name in ("blueprint_hash", "model_hash", "reference_hash", "code_hash"))
            or evidence.get("revision_binding") != artifact.get("revision_binding")
            or artifact.get("revision_binding") != receipt.get("revision_binding")
            or not isinstance(artifact.get("code"), str)
            or hashlib.sha256(artifact["code"].encode()).hexdigest() != artifact.get("code_hash")):
        return {"available": False}
    if artifact.get("revision_binding") is not None:
        try:
            binding = checked_revision_binding(artifact["revision_binding"])
        except ValueError:
            return {"available": False}
        if any(binding.get(name) != receipt.get(name) for name in ("setup_id", "stream_id")):
            return {"available": False}
    result = {"available": True, "code_hash": artifact["code_hash"], "authority": evidence["authority"]}
    delivery = checked_author_reference_delivery(artifact.get("author_reference_delivery"), artifact["key"], artifact["reference_hash"])
    if delivery is not None:
        result["author_reference_delivery"] = delivery
    return result


def activity_request(receipt, service):
    """Join an operation's actual attempt, never the latest equal-content turn."""
    artifact = receipt.get("artifact") or {}
    if artifact.get("cached") is True and not publication(receipt)["available"]:
        return None
    admission = receipt.get("native_admission")
    if admission is None:
        # A checked artifact explicitly reused by this operation is historical
        # activity. Equal content alone is not an admission or cache decision.
        if artifact.get("cached") is not True or not publication(receipt)["available"]:
            return None
        admission = artifact.get("native_admission")
    if admission is None:
        return None
    admission = checked_admission(admission)
    record = service.saved_request(receipt["native_request_key"], interaction_id=admission["interaction_id"])
    if record is None or record.get("admission") != admission:
        return None
    binding = record.get("revision_binding")
    if (binding != receipt.get("revision_binding") or binding is not None and
            any(binding.get(name) != receipt.get(name) for name in ("setup_id", "stream_id"))):
        return None
    return record


async def read_activity(operations, service, identity, expected, *, after=None, limit=100):
    if type(limit) is not int or not 1 <= limit <= 200:
        raise ValueError("The activity page must contain 1-200 frames.")
    checked_hash(identity)
    for name in ("request_fingerprint", "blueprint_hash", "model_hash", "native_request_key"):
        checked_hash(expected.get(name))
    for name in ("stream_id", "setup_id"):
        if expected.get(name) is not None and not _identifier(expected[name]):
            raise ValueError("Invalid visualization scope.")
    if type(expected.get('source_generation')) is not int or expected['source_generation'] < 1:
        raise ValueError('Invalid visualization source generation.')
    receipt = operations.get(identity)
    if receipt is None:
        raise LookupError("This scene request is no longer available.")
    keys = ("setup_id", "stream_id", "request_fingerprint", "blueprint_hash", "model_hash", "native_request_key", "source_generation")
    if any(receipt.get(name) != expected.get(name) for name in keys):
        raise ValueError("The scene activity does not match this proposal and request.")
    binding = {name: receipt.get(name) for name in BINDINGS}
    response = {**binding, "scope": "saved-native-request", "status": receipt["status"],
                "activity_origin": "retained" if (receipt.get("artifact") or {}).get("cached") else "request-bound",
                "publication": publication(receipt), "native": {"admission_confirmed": False, "status": "unknown", "interaction_id": None, "session_id": None, "terminal_cursor": None},
                "observation": {"status": "unavailable", "detail": "No matching native admission has been observed for this scene request."},
                "frames": [], "next_cursor": None, "latest_cursor": None, "exhausted": True}
    try:
        record = activity_request(receipt, service)
    except (OSError, ValueError, KeyError, TypeError):
        record = None
    if record is None or any(record.get(name) != expected[name] for name in ("blueprint_hash", "model_hash")) or not record.get("admission"):
        return response
    admission = record["admission"]
    native = response["native"]
    native.update(admission_confirmed=True, identity=admission.get("identity", "visualizer:main"),
                  interaction_id=admission["interaction_id"], session_id=admission.get("session_id"))
    native.update({key: value for key, value in (record.get("native_run") or {}).items() if key in {"status", "terminal_cursor"}})
    cursor = max(checked_cursor(admission["cursor"]), checked_cursor(after)) if after is not None else checked_cursor(admission["cursor"])
    response["next_cursor"] = f"console:{cursor}"
    runtime = getattr(service.architect, "runtime", None)
    base = getattr(runtime, "rust_http_base_url", None)
    if not base:
        response["observation"]["detail"] = "The saved native request is available; its runtime is not connected for activity observation."
        return response
    try:
        import httpx
        # Direct bounded GET: no dispatch retry helper, start, status RPC or native
        # mutation. A read failure cannot release ownership or invent terminality.
        async with asyncio.timeout(6), httpx.AsyncClient(timeout=5, trust_env=False) as client:
            async with client.stream('GET', base + "/console/timeline", params={"identity": admission.get("identity", "visualizer:main"), "after": f"console:{cursor}", "limit": limit}) as reply:
                reply.raise_for_status()
                data = bytearray()
                async for chunk in reply.aiter_bytes():
                    data.extend(chunk)
                    # Native reference-image tool frames exceed the public
                    # page budget. Read them within a separate bounded input
                    # budget, then omit their private payload in public_frame.
                    if len(data) > MAX_NATIVE_PAGE_BYTES:
                        raise ValueError("Oversized native activity page.")
                page = json.loads(data)
        if service.architect.runtime is not runtime or any(operations.get(identity).get(name) != binding.get(name) for name in BINDINGS) or activity_request(operations.get(identity), service).get("admission") != admission:
            raise ValueError("The native activity binding changed during observation.")
        frames = page.get("frames")
        if not isinstance(frames, list) or len(frames) > limit:
            raise ValueError("Invalid native activity page.")
        used, last = 0, cursor
        for frame in frames:
            position = checked_cursor(frame.get("cursor"))
            if position <= last:
                raise ValueError("Invalid native activity cursor order.")
            projected = public_frame(frame, admission)
            size = len(json.dumps(projected, ensure_ascii=False, allow_nan=False).encode()) if projected else 0
            if used + size > MAX_PAGE_BYTES:
                break
            last = position
            if projected:
                response["frames"].append(projected)
                used += size
                if projected["kind"] in TERMINALS:
                    native.update(status=TERMINALS[projected["kind"]], terminal_cursor=projected["cursor"])
        response.update(next_cursor=f"console:{last}", latest_cursor=page.get("latest_cursor"), exhausted=not frames or (len(frames) < limit and last == checked_cursor(frames[-1]["cursor"])))
        if response["latest_cursor"] is not None:
            checked_cursor(response["latest_cursor"])
        response["observation"] = {"status": "available", "detail": None}
    except (httpx.HTTPError, OSError, ValueError, KeyError, TypeError, AttributeError, TimeoutError):
        response["frames"] = []
        response["next_cursor"] = f"console:{cursor}"
        response["latest_cursor"] = None
        response["exhausted"] = True
        native.update(status=(record.get('native_run') or {}).get('status', 'unknown'), terminal_cursor=(record.get('native_run') or {}).get('terminal_cursor'))
        response["observation"] = {"status": "unavailable", "detail": "Native activity could not be observed. Reconnect to the same scene request; this does not establish a native failure."}
    return response
