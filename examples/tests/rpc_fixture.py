#!/usr/bin/env python3
"""Local JSONL peer for executing SDK examples without providers."""

import json
import os
from pathlib import Path
import signal
import sys

log = Path(os.environ["EXAMPLE_TEST_LOG"])
mode = os.environ["EXAMPLE_TEST_MODE"]
session_id = "019d1234-0000-7000-8000-000000000001"
summary = {
    "session_id": session_id,
    "created_at": 0,
    "updated_at": 0,
    "message_count": 8,
    "is_active": True,
    "total_tokens": 12,
    "model": "claude-sonnet-5-5",
    "provider": "anthropic",
}


def record(value):
    with log.open("a", encoding="utf-8") as stream:
        stream.write(json.dumps(value) + "\n")


def send(value):
    print(json.dumps({"jsonrpc": "2.0", **value}), flush=True)


# A synthetic mob for the mob examples (038, 039). A member run is an event
# sequence on the open mob stream. A send whose text starts with "Write " is a
# long job: its run stays open until it is stopped or cancelled, and sends
# that arrive meanwhile wait behind it. Any other send completes at once.
MEMBER_SESSION = "019d1234-0000-7000-8000-00000000000a"
mob = {
    "members": {}, "wired": {}, "stream": None, "sequence": 0,
    "runs": 0, "open_run": None, "queued": 0,
}


def member_ref(identity):
    return f"synthetic-ref-{identity}"


def mob_event(identity, payload):
    """One attributed event on the mob stream, framed as rkat-rpc frames it."""
    if mob["stream"] is None:
        return
    mob["sequence"] += 1
    send({"method": "mob/stream_event", "params": {
        "stream_id": mob["stream"], "sequence": mob["sequence"],
        "event": {
            "envelope": {
                "event_id": f"019d1234-0000-7000-8000-{0x1000 + mob['sequence']:012x}",
                "payload": payload, "seq": mob["sequence"], "timestamp_ms": 0,
                "source": {"type": "session", "session_id": MEMBER_SESSION},
            },
            "role": mob["members"][identity],
            "source": {"generation": 0, "identity": identity},
            "source_fence_token": 1,
        },
    }})


def run_ended(identity, run_id):
    mob_event(identity, {
        "type": "run_failed", "session_id": MEMBER_SESSION,
        "error_report": {"class": "cancelled", "message": "Run was cancelled"},
        "identity": {"interaction_id": run_id, "run_id": run_id},
    })
    mob["open_run"] = None


def mob_request(method, params):
    """Answer one mob/* request; returns (result, notifications to send after it)."""
    identity = params.get("agent_identity")
    if method == "mob/create":
        return {"mob_id": params["definition"]["id"], "claim_token": "synthetic-claim-token"}, []
    if method == "mob/spawn":
        mob["members"][identity] = params["profile"]
        return {"mob_id": params["mob_id"], "agent_identity": identity,
                "member_ref": member_ref(identity)}, []
    if method == "mob/wire":
        peer = params["peer"]["local"]
        mob["wired"].setdefault(params["member"], []).append(peer)
        mob["wired"].setdefault(peer, []).append(params["member"])
        return {"wired": True}, []
    if method == "mob/wait_kickoff":
        return {"members": [
            {"agent_identity": name, "status": "active", "output_preview": "READY",
             "tokens_used": 1, "is_final": False}
            for name in mob["members"]
        ]}, []
    if method == "mob/members":
        return {"mob_id": params["mob_id"], "members": [
            {"agent_identity": name, "member_ref": member_ref(name), "role": profile,
             "runtime_mode": "autonomous_host", "wired_to": mob["wired"].get(name, []),
             "status": "active", "is_final": False}
            for name, profile in mob["members"].items()
        ]}, []
    if method == "mob/member_history":
        return {"generation": 0, "provenance": "controlling_host_verified", "page": {
            "from_index": 0, "message_count": 3, "next_index": 1, "complete": False,
            "messages": [{"role": "system", "created_at": "1970-01-01T00:00:00+00:00", "content": (
                "Synthetic system prompt\n<skill source_uuid="
                '"00000000-0000-4b11-8111-000000000001" skill_name="mob-communication">'
            )}],
        }}, []
    if method == "mob/member_status":
        busy = mob["open_run"] is not None
        return {
            "status": "active", "member_ref": member_ref(identity),
            "output_preview": "Synthetic answer", "tokens_used": 3, "is_final": False,
            "progress": {
                "run_state": "run_open" if busy else "idle", "in_flight_work": int(busy),
                "last_progress_at_ms": 0, "last_progress_event": "unchanged",
                "health": "healthy",
            },
        }, []
    if method == "mob/stream_open":
        mob["stream"] = "synthetic-mob-stream"
        return {"opened": True, "stream_id": mob["stream"]}, []
    if method == "mob/stream_close":
        stream, mob["stream"] = mob["stream"], None
        return {"already_closed": False, "closed": True, "stream_id": stream}, [
            {"method": "mob/stream_end", "params": {
                "ended": True, "outcome": "explicit_close", "stream_id": stream,
            }},
        ]
    if method == "mob/member_send":
        receipt = {"mob_id": params["mob_id"], "agent_identity": identity,
                   "member_ref": member_ref(identity),
                   "handling_mode": params.get("handling_mode", "queue")}
        if mob["open_run"] is not None:
            mob["queued"] += 1
            return receipt, []
        mob["runs"] += 1
        run_id = f"019d1234-0000-7000-8000-{0x2000 + mob['runs']:012x}"
        run_identity = {"interaction_id": run_id, "run_id": run_id}
        mob_event(identity, {
            "type": "run_started", "session_id": MEMBER_SESSION, "identity": run_identity,
            "input": {"kind": "content", "content": params["content"]},
        })
        mob_event(identity, {"type": "text_delta", "delta": "Synthetic stream"})
        if params["content"].startswith("Write "):
            mob["open_run"] = run_id
        else:
            mob_event(identity, {
                "type": "run_completed", "session_id": MEMBER_SESSION,
                "identity": run_identity, "result": "Synthetic answer",
                "extraction_required": False,
                "usage": {"input_tokens": 1, "output_tokens": 2},
            })
        return receipt, []
    if method == "mob/stop_member_run":
        run_id = params["run_id"]
        receipt = {"outcome": "not_current", "run_id": run_id}
        if run_id == mob["open_run"]:
            run_ended(identity, run_id)
            receipt = {"outcome": "stopped", "run_id": run_id, "contributors": [{
                "input_id": "019d1234-0000-7000-8000-00000000000b",
                "completion": "cancelled", "terminal": "cancelled",
            }]}
        return {"mob_id": params["mob_id"], "agent_identity": identity, "receipt": receipt}, []
    if method == "mob/cancel_all_work":
        identity = params["member_ref"].removeprefix("synthetic-ref-")
        if mob["open_run"] is not None:
            run_ended(identity, mob["open_run"])
        record({"cancelled_queued": mob["queued"]})
        mob["queued"] = 0
        return {"mob_id": "newsdesk", "ok": True}, []
    if method == "mob/destroy":
        return {"mob_id": params["mob_id"], "ok": True, "destroy_report": {
            "remote_cleanup_deadline_exceeded": False, "metadata_scrubbed": True,
            "events_cleared": True, "namespace_cleaned": True,
        }}, []
    raise RuntimeError(f"Unexpected fixture method: {method}")


def terminate(signum, _frame):
    record({"exit_signal": signum})
    sys.exit(0)


signal.signal(signal.SIGTERM, terminate)
record({"pid": os.getpid(), "args": sys.argv[1:]})
for line in sys.stdin:
    request = json.loads(line)
    record(request)
    method = request["method"]
    params = request.get("params", {})
    after = []
    if method == "initialize":
        if mode == "exit-before-reply":
            sys.exit(13)
        if mode == "never-reply":
            continue
        result = {
            "contract_version": (
                "99.0.0" if mode == "mismatch" else os.environ["EXAMPLE_TEST_VERSION"]
            ),
            # The SDKs read the mob capability from the method list.
            "methods": ["mob/create"],
        }
    elif method == "capabilities/get":
        result = {"capabilities": [
            {"id": "skills", "description": "Synthetic skill", "status": "Available"}
        ]}
    elif method == "config/get":
        result = {
            "config": {}, "generation": 0,
            "realm_id": "example-test", "instance_id": "example-test",
        }
    elif method in {"session/create", "turn/start"}:
        refuse = mode == "create-error" or (
            mode == "comparison-error"
            and "provider_params" not in params
            and "reasoning_effort" not in params
        )
        if method == "session/create" and refuse:
            send({"id": request["id"], "error": {
                "code": -32000, "message": "Synthetic session refusal",
            }})
            continue
        if mode == "effort-error" and "reasoning_effort" in params:
            send({"id": request["id"], "error": {
                "code": -32000, "message": "Synthetic provider failure",
            }})
            continue
        effort_refusals = {
            # mode: (reason, supported, facts, retryable)
            "effort-refusal": ("unsupported_level", ["low", "medium"], "catalog", False),
            "effort-refusal-no-level": ("unsupported_level", [], "catalog", False),
            "effort-refusal-unknown": ("supported_levels_unknown", None, None, False),
            "effort-refusal-unavailable": ("supported_levels_unavailable", None, None, True),
        }
        if mode in effort_refusals and "reasoning_effort" in params:
            reason, supported, facts, retryable = effort_refusals[mode]
            send({"id": request["id"], "error": {
                "code": -32602, "message": "Synthetic reasoning effort refusal",
                "data": {
                    "kind": "reasoning_effort_refused",
                    "retryable": retryable,
                    "reasoning_effort_refusal": {
                        "requested": params["reasoning_effort"],
                        "provider": "synthetic",
                        "model": params.get("model", "synthetic"),
                        "route": None,
                        "reason": reason,
                        "supported": supported,
                        "facts": facts,
                    },
                },
            }})
            continue
        result = {
            "session_id": session_id, "text": "Synthetic answer",
            "turns": 1, "tool_calls": 0,
            "usage": {"input_tokens": 1, "output_tokens": 2, "total_tokens": 3},
        }
        if "reasoning_effort" in params:
            result["reasoning_effort"] = {
                "requested": params["reasoning_effort"],
                "sent": [{
                    "level": params["reasoning_effort"],
                    "provider": "synthetic",
                    "model": params.get("model", "synthetic"),
                    "route": None,
                    "fields": ["synthetic.effort"],
                    "facts": "catalog",
                    "decided_by": "request",
                    "requests": 1,
                }],
            }
        if "output_schema" in params:
            result["structured_output"] = {
                "sentiment": "positive", "confidence": 0.5,
                "key_phrases": ["synthetic"], "summary": "Synthetic sentiment",
            }
        if method == "turn/start":
            send({"method": "session/event", "params": {
                "session_id": session_id,
                "event": {
                    "event_id": "019d1234-0000-7000-8000-000000000002",
                    "source": {"type": "session", "session_id": session_id},
                    "seq": 1, "timestamp_ms": 0,
                    "payload": {"type": "text_delta", "delta": "Synthetic stream"},
                },
            }})
    elif method == "session/read":
        result = summary
    elif method == "session/list":
        result = {"sessions": [summary]}
    elif method == "session/archive":
        result = {"archived": True}
    elif method == "mob/create" and mode == "create-error":
        send({"id": request["id"], "error": {
            "code": -32602, "message": "Synthetic mob refusal",
        }})
        continue
    elif method.startswith("mob/"):
        result, after = mob_request(method, params)
    else:
        raise RuntimeError(f"Unexpected fixture method: {method}")
    send({"id": request["id"], "result": result})
    for notification in after:
        send(notification)
record({"eof": True})
