"""New inert wire-decoder controls, not native or installed-host acceptance."""
from __future__ import annotations

import copy
import json
from pathlib import Path
import time
import unittest

from launchkit.templates_v2.console_connect import (
    ConsoleConnect, _answer, _bootstrap, _decode, _json, _prompt, _state,
)


ORIGIN = "http://127.0.0.1:4110"
ORDINARY_PROMPTS = ["forge_recovery", "forge_origin", "forge_password", "approval"]
SERVICE_PROMPTS = ["approval", "provider_target", "provider_action", "provider_api_key",
                   "provider_browser", "provider_device"]


def bootstrap(version=4):
    value = {
        "protocol_version": version,
        "application": {"application_id": "custom-app", "instance_id": "inert-lifetime", "origin": ORIGIN},
        "viewer": {"principal_id": "fixture-viewer", "login_scope_id": "fixture-login",
                   "expires_at_ms": int(time.time() * 1000) + 60000},
        "realm": {"realm_id": "fixture-realm", "state_root": "/inert/state",
                  "context_root": "/inert/context", "user_config_root": "/inert/config"},
    }
    if version == 4:
        value.update(adapter="forge_application", forge={
            "host": {"protocol": 1, "host_id": "forge:" + "a" * 64,
                     "application_id": "custom-app", "source_digest": "b" * 64},
            "private_fields": ["PRIVATE_TOKEN", "PRIVATE_KEY"],
        })
    elif version == 3:
        value.update(adapter="forge_services", services={
            "resource_manifest_sha256": "c" * 64, "preparation": "permitted"})
    return value


def state(version=4):
    value = dict(available=True, can_send=True, can_stop=False, outcome_uncertain=False,
                 active_send=None, last_completion=None)
    if version in {3, 4}:
        value["private_notice_pending"] = False
    return value


def capabilities(version=4):
    value = dict(prompt_kinds=SERVICE_PROMPTS.copy() if version == 3 else ORDINARY_PROMPTS.copy(),
                 forge_configuration=version != 3, service_preparation=version == 3, homecore=False)
    if version in {3, 4}:
        # The ordinary native response preserves declared tool order. V3 alone
        # sorts its names. Names are observations, never a permission inventory.
        value["tools"] = ["a", "z"] if version == 3 else ["z", "a"]
    return value


class Protocol4DecoderControls(unittest.IsolatedAsyncioTestCase):
    def child(self, version=4):
        return ConsoleConnect(Path("/inert/acquired-connect"), bootstrap(version), is_current=lambda: True)

    def initialize(self, child, *, result=None):
        # Reuse the actual retained request/response decoder. No subprocess,
        # native session, provider, presenter owner or network is created.
        child._process = object()
        identity, future = child._request("connect/initialize", child.bootstrap)
        wire = child._outgoing.get_nowait()
        if result is None:
            result = dict(protocol_version=child.protocol_version, scope="fixture-scope",
                          session_id="fixture-observation", capabilities=capabilities(child.protocol_version),
                          state=state(child.protocol_version))
        child._receive(dict(jsonrpc="2.0", id=identity, result=result))
        return wire, future.result()

    async def test_v4_retains_exact_trusted_metadata_without_starting_transport(self):
        supplied = bootstrap()
        supplied["forge"]["private_fields"] = ["PRIVATE_KEY", "", "PRIVATE_TOKEN", "PRIVATE_KEY"]
        supplied["model"] = "selected-model"
        supplied["auth_binding"] = {"realm": "fixture-realm", "binding": "selected", "profile": "exact"}
        child = ConsoleConnect(Path("/inert/acquired-connect"), supplied, is_current=lambda: True)
        retained = copy.deepcopy(supplied)
        supplied["forge"]["host"]["source_digest"] = "c" * 64
        supplied["forge"]["private_fields"].clear()
        self.assertEqual(child.bootstrap, retained)
        self.assertIsNone(child._process)
        self.assertIsNone(child.capabilities)
        self.assertEqual(child._state["private_notice_pending"], False)
        empty = bootstrap(); empty["forge"]["private_fields"] = []
        self.assertEqual(_bootstrap(empty), empty)

    async def test_v4_rejects_malformed_mismatched_and_mixed_mode_metadata(self):
        mutations = [
            (("adapter",), "forge_reading"), (("adapter",), "forge_services"),
            (("forge",), None), (("forge", "host"), None),
            (("forge", "host", "protocol"), True), (("forge", "host", "protocol"), 2),
            (("forge", "host", "host_id"), "forge:" + "A" * 64),
            (("forge", "host", "source_digest"), "b" * 63),
            (("forge", "host", "application_id"), "other-app"),
            (("forge", "private_fields"), None), (("forge", "private_fields"), [1]),
            (("application", "origin"), "http://localhost:4110"),
        ]
        for keys, replacement in mutations:
            value = bootstrap(); parent = value
            for key in keys[:-1]: parent = parent[key]
            parent[keys[-1]] = replacement
            with self.subTest(keys=keys, replacement=replacement), self.assertRaises(ValueError):
                _bootstrap(value)
        for target in ((), ("forge",), ("forge", "host")):
            value = bootstrap(); parent = value
            for key in target: parent = parent[key]
            parent["extra"] = True
            with self.subTest(target=target), self.assertRaises(ValueError): _bootstrap(value)
        for key in ("adapter", "forge"):
            value = bootstrap(); value.pop(key)
            with self.subTest(missing=key), self.assertRaises(ValueError): _bootstrap(value)
        value = bootstrap(); value["forge"].pop("private_fields")
        with self.assertRaises(ValueError): _bootstrap(value)
        for field in ("homecore", "services", "connector_auth", "credential"):
            with self.subTest(mixed=field), self.assertRaises(ValueError):
                _bootstrap({**bootstrap(), field: {}})
        for application_id in ("", "a" * 129, "unicode-\u00e9", "slash/app"):
            value = bootstrap()
            value["application"]["application_id"] = value["forge"]["host"]["application_id"] = application_id
            with self.subTest(application_id=application_id), self.assertRaises(ValueError): _bootstrap(value)

    async def test_v4_negotiates_actual_tools_and_ordinary_scope_prompt_custody(self):
        child = self.child()
        wire, result = self.initialize(child)
        self.assertEqual(_decode(wire)["params"], child.bootstrap)
        self.assertEqual(result["state"], state())
        self.assertEqual(child.capabilities, capabilities())
        self.assertIsNone(child.presentation_binding())
        observer = await child.attach(private=True)
        self.assertEqual((await anext(observer))["event"]["scope"], "fixture-scope")
        prompt = dict(kind="approval", approval_id="actual-owner-proof", summary="Complete proposal",
                      body={"configuration": {"extension": {"raw": [1, "two"]}}, "revision": {"opaque": True}},
                      allowed_decisions=["approve", "deny"], expires_at=None)
        child._receive(dict(jsonrpc="2.0", id="p:one", method="connect/prompt",
                            params=dict(scope="fixture-scope", prompt=prompt)))
        shown = await anext(observer)
        self.assertEqual(shown, dict(type="prompt", id="p:one", scope="fixture-scope", prompt=prompt))
        self.assertEqual(child._prompts["p:one"], prompt)
        self.assertEqual(_answer(dict(kind="approval", decision="deny"),
                                 prompt, protocol_version=4)["decision"], "deny")
        child._detach(observer)

    async def test_v4_refuses_capability_and_state_type_or_composition_mismatch(self):
        for key, value in (("tools", None), ("tools", [1]), ("tools", [""]),
                           ("tools", ["bad\ud800"]), ("forge_configuration", False),
                           ("forge_configuration", 1), ("service_preparation", True),
                           ("homecore", True), ("prompt_kinds", SERVICE_PROMPTS)):
            child = self.child()
            cap = capabilities(); cap[key] = value
            result = dict(protocol_version=4, scope="fixture-scope", session_id="fixture-observation",
                          capabilities=cap, state=state())
            with self.subTest(key=key, value=repr(value)), self.assertRaises((ValueError, UnicodeError)):
                self.initialize(child, result=result)
            self.assertIsNone(child.capabilities)
            self.assertFalse(child._initialized)
            self.assertIsNone(child._scope)
        for mutation in ("missing_tools", "extra_capability", "wrong_version", "missing_notice", "extra_state", "unowned_send"):
            child = self.child()
            result = dict(protocol_version=4, scope="fixture-scope", session_id="fixture-observation",
                          capabilities=capabilities(), state=state())
            if mutation == "missing_tools": result["capabilities"].pop("tools")
            elif mutation == "extra_capability": result["capabilities"]["permission"] = True
            elif mutation == "wrong_version": result["protocol_version"] = 1
            elif mutation == "missing_notice": result["state"].pop("private_notice_pending")
            elif mutation == "extra_state": result["state"]["permission"] = True
            else: result["state"]["active_send"] = dict(send_request_id="h:unknown", cancellation_requested=False)
            with self.subTest(mutation=mutation), self.assertRaises(ValueError): self.initialize(child, result=result)
            self.assertFalse(child._initialized)
            self.assertIsNone(child._scope)

    async def test_v4_native_notice_state_and_ordinary_prompt_types_remain_closed(self):
        for pending in (True, False):
            value = state(); value["private_notice_pending"] = pending
            self.assertEqual(_state(value, protocol_version=4), value)
        value["private_notice_pending"] = 1
        with self.assertRaises(ValueError): _state(value, protocol_version=4)
        for prompt in (
            dict(kind="forge_origin", permitted_origin=ORIGIN),
            dict(kind="forge_password", origin=ORIGIN, mode="initial", min_chars=12, max_chars=4096),
            dict(kind="forge_recovery", choices=[dict(choice_id="c:one", application_id="custom-app",
                                                      change_id="exact-change", origin=ORIGIN)], more=False, skipped_entries=0),
        ):
            self.assertEqual(_prompt(prompt, ORIGIN, protocol_version=4), prompt)
            with self.assertRaises(ValueError): _prompt({**prompt, "permission": True}, ORIGIN, protocol_version=4)
        with self.assertRaises(ValueError): _prompt(dict(kind="provider_target", choices=[]), ORIGIN, protocol_version=4)
        child = self.child(); self.initialize(child)
        for identity, params in (
            ("p:mixed", dict(target={"scope": "fixture-scope"}, prompt=dict(kind="forge_origin", permitted_origin=ORIGIN))),
            ("p:wrong", dict(scope="other-scope", prompt=dict(kind="forge_origin", permitted_origin=ORIGIN))),
            ("p:recovery", dict(scope="fixture-scope", prompt=dict(kind="forge_recovery", choices=[
                dict(choice_id="other", application_id="reading", change_id="change", origin=ORIGIN)], more=False, skipped_entries=0))),
        ):
            child._receive(dict(jsonrpc="2.0", id=identity, method="connect/prompt", params=params))
            refusal = _decode(child._outgoing.get_nowait())
            self.assertEqual(refusal["id"], identity)
            self.assertEqual(refusal["error"]["code"], -32602)
            self.assertNotIn(identity, child._prompts)

    async def test_old_v1_v3_wire_bytes_and_negotiation_keep_distinct_closed_contracts(self):
        for version in (1, 3):
            child = self.child(version)
            expected = json.dumps(dict(jsonrpc="2.0", id="h:fixed", method="connect/initialize", params=child.bootstrap),
                                  ensure_ascii=False, allow_nan=False, separators=(",", ":")).encode() + b"\n"
            self.assertEqual(_json(dict(jsonrpc="2.0", id="h:fixed", method="connect/initialize", params=child.bootstrap)), expected)
            wire, result = self.initialize(child)
            retained = _decode(wire)
            self.assertEqual(wire, expected.replace(b'"h:fixed"', json.dumps(retained["id"]).encode(), 1))
            self.assertEqual(result["capabilities"], capabilities(version))
            self.assertEqual(result["state"], state(version))
            self.assertEqual(child.capabilities, capabilities(version))
            with self.assertRaises(ValueError): _bootstrap({**bootstrap(version), "forge": bootstrap()["forge"]})
            invalid = state(version)
            if version == 1: invalid["private_notice_pending"] = False
            else: invalid.pop("private_notice_pending")
            with self.assertRaises(ValueError): _state(invalid, protocol_version=version)
        child = self.child(1)
        cap = capabilities(1); cap["tools"] = ["extra"]
        with self.assertRaises(ValueError): self.initialize(child, result=dict(
            protocol_version=1, scope="fixture-scope", session_id="fixture-observation", capabilities=cap, state=state(1)))
        for mutation in ("ordinary_composition", "unsorted_tools", "ordinary_prompt"):
            child = self.child(3); cap = capabilities(3)
            if mutation == "ordinary_composition": cap.update(forge_configuration=True, service_preparation=False)
            elif mutation == "unsorted_tools": cap["tools"] = ["z", "a"]
            else: cap["prompt_kinds"] = ORDINARY_PROMPTS
            with self.subTest(mutation=mutation), self.assertRaises(ValueError): self.initialize(child, result=dict(
                protocol_version=3, scope="fixture-scope", session_id="fixture-observation", capabilities=cap, state=state(3)))
