"""meta-forge units: options, setup handles, review digests, exit codes and renderers.

No Forge server runs here; HTTP answers come from httpx.MockTransport.
"""
import argparse
import hashlib
import json
import re

import httpx
import pytest

from cli_support import GUIDE_ID, OTHER_ID, cli
from launchkit.cli import build_parser, duration, global_value, requested_mode, usage_message, without_values
from launchkit.cli.client import server_url
from launchkit.cli.commands.host import child_environment, provider_variable
from launchkit.cli.commands.setup import chat_answer, option_ids, turn_body
from launchkit.cli.errors import EXIT_CODES, CliError, from_response
from launchkit.cli.render import identities, plan_summary, review_lines, tool_labels
from launchkit.cli.review import canonical, plan_document, review_digest, sha256_of
from launchkit.projects import fingerprint

LEAVES = {
    "serve", "doctor", "version", "provider status", "guide status", "guide start", "guide stop",
    "setup list", "setup show", "setup new", "setup send", "setup revisit", "setup retry", "setup timeline",
    "setup organization", "chat", "plan show", "plan accept", "plan export", "plan validate", "plan package",
    "install", "installation status", "installation start", "installation stop", "installation restart",
    "installation retry", "installation cleanup", "installation request", "installation log",
    "installation open", "installation settings show", "installation export",
}
SECRET_OPTION = re.compile(r"secret|passw|passphrase|token|credential|api.?key|^--key$|^--keys?$|private", re.I)
VERSION = "0.3.1"


def parsers(parser, path=()):
    yield path, parser
    for action in parser._actions:
        if isinstance(action, argparse._SubParsersAction):
            for name, child in action.choices.items():
                yield from parsers(child, path + (name,))


def test_command_inventory_is_exactly_slice_one():
    leaves = {" ".join(path) for path, parser in parsers(build_parser()) if parser.get_default("func")}
    assert leaves == LEAVES


def test_no_option_accepts_a_secret_value():
    seen = []
    for path, parser in parsers(build_parser()):
        for action in parser._actions:
            for option in action.option_strings:
                seen.append(option)
                if SECRET_OPTION.search(option):
                    raise AssertionError(f"{' '.join(path)} {option} names a secret")
                if "key" in option:
                    # The only key option is a flag: Forge copies its own saved keys server-side.
                    assert option == "--reuse-forge-keys" and action.nargs == 0, option
    assert "--reuse-forge-keys" in seen and "--set" in seen


@pytest.mark.parametrize("value,expected", [("90", 90.0), ("90s", 90.0), ("15m", 900.0), ("1h", 3600.0),
                                            ("250ms", 0.25), ("1.5m", 90.0)])
def test_durations(value, expected):
    assert duration(value) == expected


@pytest.mark.parametrize("value", ["", "0", "-1s", "5 minutes", "1d", "m"])
def test_invalid_durations(value):
    with pytest.raises(argparse.ArgumentTypeError):
        duration(value)


def test_server_url_is_loopback_only():
    assert server_url({}) == "http://127.0.0.1:8787"
    assert server_url({"META_FORGE_PORT": "4110"}) == "http://127.0.0.1:4110"
    assert server_url({"MOBKIT_LAUNCH_PORT": "4111"}) == "http://127.0.0.1:4111"
    assert server_url({"META_FORGE_PORT": "4110", "MOBKIT_LAUNCH_PORT": "4111"}) == "http://127.0.0.1:4110"
    assert server_url({}, "http://localhost:9000") == "http://localhost:9000"
    assert server_url({}, "http://[::1]:9000/") == "http://[::1]:9000"
    for value in ("https://127.0.0.1:8787", "http://example.com:8787", "http://10.0.0.5:8787",
                  "http://127.0.0.1:8787/api", "http://user:pw@127.0.0.1:8787", "http://127.0.0.1"):
        with pytest.raises(CliError) as refused:
            server_url({}, value)
        assert refused.value.code == "usage"
    with pytest.raises(CliError):
        server_url({"META_FORGE_PORT": "70000"})


def test_requested_mode_reads_the_output_options():
    assert requested_mode(["setup", "list"]) == "text"
    assert requested_mode(["--json", "setup", "list"]) == "json"
    assert requested_mode(["setup", "list", "-o", "jsonl"]) == "jsonl"
    assert requested_mode(["setup", "list", "--output=json"]) == "json"


@pytest.mark.parametrize("status,payload,code,exit_code", [
    (400, {"detail": "Invalid request length."}, "invalid_request", 2),
    (403, {"detail": "Open Meta Forge from its local setup URL."}, "invalid_request", 2),
    (404, {"detail": "This setup could not be found."}, "not_found", 81),
    (409, {"detail": "This setup changed. Reload before continuing."}, "conflict", 82),
    (409, {"detail": "Recover the selection.", "code": "build_selection_unavailable"}, "build_selection_unavailable", 82),
    (409, {"detail": "Not sent.", "code": "remote_product_action_not_sent"}, "conflict", 82),
    (413, {"detail": "This setup is too large."}, "invalid_request", 2),
    (422, {"detail": [{"loc": ["body", "version"], "msg": "Field required", "type": "missing"}]}, "invalid_request", 2),
    (500, None, "outcome_unknown", 83),
    (502, {"detail": "The setup agent could not finish."}, "outcome_unknown", 83),
    (503, {"detail": "The setup agent needs a provider key."}, "prerequisite_unavailable", 86),
    (504, {"detail": "The setup agent is still working."}, "outcome_unknown", 83),
])
def test_http_refusals_map_to_typed_exit_codes(status, payload, code, exit_code):
    error = from_response(status, payload)
    assert (error.code, error.exit_code, error.http_status) == (code, exit_code, status)
    if isinstance(payload, dict) and "code" in payload:
        assert error.server_code == payload["code"]
    if status == 422:
        assert error.detail == "body.version: Field required"


def test_exit_code_bands_do_not_collide_with_core():
    assert set(EXIT_CODES.values()) <= {1, 2, 80, 81, 82, 83, 84, 85, 86, 87, 130}
    assert not any(10 <= value <= 59 for value in EXIT_CODES.values())


def capabilities(version=VERSION):
    return {"version": version, "planner": {"available": True, "provider": "openai"},
            "agent_team_runtime": {"sdk": "meerkat-meta", "version": "0.9.6", "release": "https://example.test/r"},
            "ordinary_runtime": None, "platform": "linux"}


def mock(routes):
    """A MockTransport answering (method, path) from a table; unknown routes are 404."""
    calls = []

    def handler(request):
        calls.append(request)
        answer = routes.get((request.method, request.url.path))
        if callable(answer):
            return answer(request)
        if answer is None:
            return httpx.Response(404, json={"detail": "Not Found"})
        status, body = answer
        return httpx.Response(status, json=body)
    transport = httpx.MockTransport(handler)
    transport.calls = calls
    return transport


SETUPS = [{"id": GUIDE_ID, "name": "Weekly digest", "stage": "review", "status": "ready", "updated_at": "2026-11-09T10:00"},
          {"id": OTHER_ID, "name": "Notes", "stage": "purpose", "status": "question", "updated_at": "2026-11-08T10:00"}]


def guide_routes(state=None):
    state = state or {"id": GUIDE_ID, "version": 3, "status": "question", "stage": "purpose", "messages": []}
    return {("GET", "/api/capabilities"): (200, capabilities()),
            ("GET", "/api/guide/list"): (200, {"setups": SETUPS}),
            ("GET", f"/api/guide/{GUIDE_ID}"): (200, state),
            ("GET", f"/api/guide/{OTHER_ID}"): (200, {**state, "id": OTHER_ID})}


@pytest.mark.parametrize("handle,expected", [("last", GUIDE_ID), (GUIDE_ID, GUIDE_ID), ("67f2f55d-8f", GUIDE_ID),
                                             ("67F2F55D-8FD4", GUIDE_ID), (OTHER_ID, OTHER_ID)])
async def test_setup_handles_resolve_like_rkat_session_handles(handle, expected):
    transport = mock(guide_routes())
    run = await cli(["setup", "show", handle, "--json"], transport=transport)
    assert run.code == 0, run.stderr
    assert run.document["setup"] == expected
    assert run.document["result"]["id"] == expected


@pytest.mark.parametrize("handle,code,exit_code", [("67f2f55d", "usage", 2), ("deadbeef", "not_found", 81),
                                                    ("67f2", "usage", 2), ("../etc", "usage", 2)])
async def test_ambiguous_unknown_or_short_handles_are_refused(handle, code, exit_code):
    run = await cli(["setup", "show", handle, "--json"], transport=mock(guide_routes()))
    assert (run.code, run.document["error"]["code"]) == (exit_code, code)
    assert run.document["next"], "every failure names a next command"


async def test_last_without_setups_points_to_a_new_setup():
    routes = guide_routes()
    routes[("GET", "/api/guide/list")] = (200, {"setups": []})
    run = await cli(["setup", "show", "last", "--json"], transport=mock(routes))
    assert run.code == 81 and run.document["error"]["code"] == "not_found"
    assert any("setup new" in item["command"] or "chat --new" in item["command"] for item in run.document["next"])


async def test_transport_failures_are_typed():
    def refused(request):
        raise httpx.ConnectError("refused", request=request)

    def lost(request):
        if request.url.path == "/api/capabilities":
            return httpx.Response(200, json=capabilities())
        raise httpx.ReadTimeout("lost", request=request)

    def dropped(request):
        if request.url.path == "/api/capabilities":
            return httpx.Response(200, json=capabilities())
        raise httpx.RemoteProtocolError("dropped", request=request)

    def html(request):
        return httpx.Response(200, text="<html>not forge</html>")

    def redirect(request):
        return httpx.Response(302, headers={"location": "http://elsewhere.test/"})

    for handler, code, exit_code in ((refused, "server_unavailable", 80), (lost, "outcome_unknown", 83),
                                     (dropped, "outcome_unknown", 83), (html, "server_incompatible", 80),
                                     (redirect, "server_incompatible", 80)):
        run = await cli(["setup", "list", "--json"], transport=httpx.MockTransport(handler))
        assert (run.code, run.document["error"]["code"]) == (exit_code, code), handler.__name__


async def test_another_forge_release_is_incompatible():
    routes = guide_routes()
    routes[("GET", "/api/capabilities")] = (200, capabilities("0.2.9"))
    transport = mock(routes)
    run = await cli(["setup", "list", "--json"], transport=transport)
    assert run.code == 80 and run.document["error"]["code"] == "server_incompatible"
    assert [call.url.path for call in transport.calls] == ["/api/capabilities"]


async def test_json_mode_prints_exactly_one_document_also_on_failure():
    for argv in (["setup", "list", "--json"], ["setup", "show", "nosuchid1", "--json"]):
        run = await cli(argv, transport=mock(guide_routes()))
        document = json.loads(run.stdout)
        assert document["format"] == "meta-forge-cli/1"
        assert set(document) >= {"ok", "command", "needs", "next"}
        assert run.stdout.count("\n") == 1


async def test_jsonl_mode_ends_with_one_result_or_error():
    run = await cli(["setup", "list", "-o", "jsonl"], transport=mock(guide_routes()))
    assert run.code == 0
    assert [event["event"] for event in run.events][-1] == "result"
    assert sum(event["event"] in ("result", "error") for event in run.events) == 1
    run = await cli(["setup", "show", "nosuchid1", "-o", "jsonl"], transport=mock(guide_routes()))
    assert run.events[-1]["event"] == "error" and run.code == 2


async def test_usage_errors_follow_the_requested_mode():
    run = await cli(["--json", "nosuch"])
    assert run.code == 2 and run.document["error"]["code"] == "usage"
    run = await cli(["setup", "show"])
    assert run.code == 2 and "error:" in run.stderr and run.stdout == ""


def test_parse_errors_in_another_wording_never_show_a_value():
    # Another Python release may word argparse's messages differently; a typed value still never shows.
    parser = build_parser()
    argv = ["setup", "timeline", "last", "--limit", "sk-typed-in-the-wrong-place"]
    future = "meta-forge setup timeline: argument --limit: not a number 'sk-typed-in-the-wrong-place'"
    assert without_values(future, argv, parser) == "meta-forge setup timeline: invalid arguments (values not shown)"
    known = "meta-forge setup timeline: argument --limit: use a number from 1 to 500"
    assert without_values(known, argv, parser) == known
    assert usage_message(parser, "a new kind of message 'sk-typed'") == "invalid arguments (values not shown)"
    assert usage_message(parser, "argument --limit: a new reason 'sk-typed'") == (
        "argument --limit: invalid value (not shown)")


async def test_home_applies_only_to_serve_and_doctor():
    run = await cli(["setup", "list", "--home", "/tmp/elsewhere", "--json"], transport=mock(guide_routes()))
    assert run.code == 2 and "--server" in run.document["error"]["detail"]


async def test_global_options_before_and_after_the_command_merge():
    first, second = "sha256:" + "a" * 64, "sha256:" + "b" * 64
    namespace = build_parser().parse_args(["--approve", first, "-v", "setup", "list", "--approve", second, "-v"])
    assert global_value(namespace, "approve") == [first, second]
    assert global_value(namespace, "verbose") == 2
    run = await cli(["-v", "setup", "list", "--json"], transport=mock(guide_routes()))
    assert run.code == 0
    assert run.stderr.count("> GET /api/guide/list") == 1


def blueprint():
    return {"schema_version": 2, "project": {"name": "Weekly digest", "slug": "weekly-digest", "description": "x"},
            "provider": "openai", "model": "gpt-5.5", "target": "linux",
            "agent_classes": [
                {"id": "coordinator", "name": "Coordinator", "description": "Coordinates", "instructions": "Coordinate the digest.",
                 "provider": "openai", "model": "gpt-5.5", "instances": 1,
                 "tools": {"notes": "write", "workgraph": True, "memory": True,
                           "mcp": [{"connection_id": "notes", "tool_names": ["read_note", "write_note"]}]}},
                {"id": "analyst", "name": "Analyst", "description": "Reads", "instructions": "Read project notes.",
                 "provider": "openai", "model": "gpt-5.5", "instances": 2,
                 "tools": {"workspace": True, "memory": False, "mcp": [{"connection_id": "notes", "tool_names": []}]}},
                {"id": "checker", "name": "Checker", "description": "Checks", "instructions": "Check the digest.",
                 "provider": "openai", "model": "gpt-5.5", "instances": 2,
                 "members": [{"key": "ana", "name": "Ana", "instructions": "Check facts."},
                             {"key": "bo", "name": "Bo", "instructions": "Check tone."}], "tools": {}}],
            "connections": [{"id": "notes", "name": "Notes", "transport": "stdio", "command": "npx",
                             "args": ["-y", "notes-mcp"], "url": "", "env": {"NOTES_TOKEN": "${NOTES_TOKEN}"}}],
            "topology": {"mode": "hub", "entrypoint": "coordinator:main", "edges": []},
            "flows": [{"id": "weekly", "name": "Weekly digest", "steps": [{"id": "draft", "agent": "coordinator:main", "prompt": "Draft"}]}],
            "knowledge": {"mode": "none"}}


def test_plan_summary_derives_identities_tools_and_connections_from_server_data():
    summary = plan_summary(blueprint())
    assert [item["identities"] for item in summary["classes"]] == [
        ["coordinator:main"], ["analyst:1", "analyst:2"], ["checker:ana", "checker:bo"]]
    assert summary["classes"][0]["tools"] == ["notes(write)", "memory", "workgraph", "mcp:notes(read_note, write_note)"]
    assert summary["classes"][1]["tools"] == ["workspace", "mcp:notes(all tools)"]
    assert summary["classes"][2]["tools"] == ["memory"]
    assert summary["connections"] == [{"id": "notes", "name": "Notes", "transport": "stdio",
                                       "command": ["npx", "-y", "notes-mcp"]}]
    # Connection environment references are not part of a review: they are setting names, not values.
    assert "NOTES_TOKEN" not in json.dumps(summary)
    assert identities({"id": "idle", "instances": 0}) == []
    assert tool_labels({"read_only": True, "shell": True, "memory": False}) == ["read-only tools only", "shell"]


def reviewable_state():
    return {"id": GUIDE_ID, "version": 7, "status": "review", "question": None, "proposal": blueprint(),
            "brief": {"facts": {}, "requirements": []},
            "readiness": {"context_ready": True, "decision_blockers": [], "implementation_blockers": []},
            "package_availability": {"available": True, "status": "available", "reason": "",
                                     "guide_id": GUIDE_ID, "blueprint_hash": "x"}}


def test_plan_review_document_and_digest_are_stable():
    document = plan_document(reviewable_state())
    assert document["gate"] == "accept-plan" and document["version"] == 7
    assert document["package_availability"] == {"available": True, "status": "available", "reason": ""}
    assert document["blueprint_sha256"] == fingerprint(blueprint())
    expected = "sha256:" + hashlib.sha256(json.dumps(document, sort_keys=True, separators=(",", ":"),
                                                     ensure_ascii=True).encode()).hexdigest()
    assert review_digest(document) == expected
    # Forge's own canonical fingerprint form: the digest of a review can be recomputed from its JSON.
    assert sha256_of({"b": 1, "a": "\u00e9"}) == fingerprint({"b": 1, "a": "\u00e9"})
    assert canonical({"b": [1, 2], "a": None}) == b'{"a":null,"b":[1,2]}'
    changed = reviewable_state()
    changed["version"] = 8
    assert review_digest(plan_document(changed)) != review_digest(document)


def test_distinct_review_documents_read_distinctly():
    # The review a person reads stands for exactly one document: list boundaries, null, empty values and
    # escaped characters stay distinguishable from the text that looks like them.
    documents = [{"items": ["a, b"]}, {"items": ["a", "b"]}, {"items": "a, b"}, {"items": ["a"]}, {"items": "a"},
                 {"value": None}, {"value": "None"}, {"value": "null"}, {"value": []}, {"value": "[]"},
                 {"value": {}}, {"value": ""}, {"value": "none"}, {"value": True}, {"value": "true"},
                 {"value": 1}, {"value": "1"}, {"value": "x\ny"}, {"value": "x\\ny"}, {"value": "x\x1by"},
                 {"value": "x\\x1by"}, {"value": " padded"}, {"value": "padded"}, {"a_b": 1}, {"a b": 1}]
    readings = [tuple(review_lines({"title": "t", **document}, None)) for document in documents]
    assert len(set(readings)) == len(documents), [reading for reading in readings if readings.count(reading) > 1]
    for reading in readings:
        assert all(line.isprintable() for line in reading), reading


def question():
    return {"stage": "purpose", "id": "audience", "title": "Who reads it?", "description": "Pick one.",
            "multiple": False, "allow_text": True,
            "options": [{"id": "me", "label": "Just me", "description": "Personal"},
                        {"id": "team", "label": "A small team", "description": "Shared"},
                        {"id": "many", "label": "Many people", "description": "Customers"}]}


def test_chat_answers_parse_numbers_and_context():
    assert chat_answer(question(), "2") == (["team"], "", False)
    assert chat_answer(question(), "1,3") == (["me", "many"], "", True)
    assert chat_answer(question(), "2 we also share it weekly") == (["team"], "we also share it weekly", False)
    assert chat_answer({**question(), "multiple": True}, "1, 2") == (["me", "team"], "", False)
    assert chat_answer(question(), "everyone in the office") == ([], "everyone in the office", False)
    assert chat_answer({**question(), "options": []}, "2 people") == ([], "2 people", False)
    for line in ("4", "0", "1,1"):
        with pytest.raises(CliError):
            chat_answer(question(), line)


def test_turn_bodies_match_the_browser_requests():
    state = {"id": GUIDE_ID, "version": 4, "question": question()}
    assert turn_body(state, selected=["team"], text="", combine=False, attachments=[]) == {
        "version": 4, "action": "answer", "selected": ["team"], "text": ""}
    assert turn_body(state, selected=["me", "many"], text="", combine=True, attachments=[]) == {
        "version": 4, "action": "answer", "selected": ["me", "many"], "text": "", "combine_options": True}
    reference = {"id": "a" * 64, "filename": "notes.md", "media_type": "text/markdown", "size": 3,
                 "sha256": "b" * 64, "url": "/x"}
    assert turn_body(state, selected=[], text="", combine=False, attachments=[reference]) == {
        "version": 4, "action": "chat", "selected": [], "text": "Consider the attached evidence.",
        "attachments": [reference]}
    open_chat = {"id": GUIDE_ID, "version": 5, "question": None}
    assert turn_body(open_chat, selected=[], text="Make it weekly", combine=False, attachments=[]) == {
        "version": 5, "action": "chat", "selected": [], "text": "Make it weekly"}
    for kwargs in ({"selected": [], "text": " ", "combine": False, "attachments": []},
                   {"selected": ["team"], "text": "", "combine": True, "attachments": []}):
        with pytest.raises(CliError):
            turn_body(state, **kwargs)
    assert option_ids(question(), ["2", "many"]) == ["team", "many"]
    with pytest.raises(CliError):
        option_ids(question(), ["nobody"])


def test_child_environment_withholds_provider_credentials():
    environ = {"PATH": "/usr/bin", "OPENAI_API_KEY": "x", "RKAT_GEMINI_API_KEY": "y", "GOOGLE_API_KEY": "z",
               "GITHUB_TOKEN": "t", "GH_TOKEN": "t", "COPILOT_GITHUB_TOKEN": "t", "META_FORGE_HOME": "/s",
               "MY_SETTING": "kept"}
    assert child_environment(environ, inherit_provider=False) == {"PATH": "/usr/bin", "META_FORGE_HOME": "/s",
                                                                 "MY_SETTING": "kept"}
    assert child_environment(environ, inherit_provider=True) == environ
    assert provider_variable("ANTHROPIC_API_KEY") and not provider_variable("API_KEY_FILE")
