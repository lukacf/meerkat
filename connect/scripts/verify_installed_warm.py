"""One installed warm consumer, using the existing Console/SDK owners.

Imported explicitly by the gateway verifier. Import does no I/O. The controlled
model selects real tools and consumes their returned values; it supplies no
review, native grant, input, session, completion or installation receipt.
"""
from __future__ import annotations

import asyncio
import copy
import dataclasses
import hashlib
from http.client import HTTPConnection
import json
from pathlib import Path
import sqlite3
import threading
import time
import tomllib
import uuid
from urllib.parse import urlencode, urlsplit

Y_OPERATION = "search_wikipedia"
Y_SELECTION = {"family": "agno", "profile": "wikipedia",
               "operations": [Y_OPERATION], "values": {"language": "en", "timeout": 7}}
Y_QUERY = "Hacker News"
WARM_REQUEST = (
    "Keep the already installed Hacker News integration and add anonymous English Wikipedia reading. "
    "Reuse the exact checked Hacker News job without reinstalling it. Request fresh task-specific "
    "access to the complete set of both integrations. Then read one current Hacker News story and "
    "the English Wikipedia article for Hacker News, and report the returned story title and article URL."
)
B_FINISH = "The current task used Hacker News and requested a later complete-set acquisition. Its successor owns the remaining reads."
C_STORY = "warm-c-story"
C_WIKI = "warm-c-wikipedia"
B_FIND = "warm-b-request"
Y_ACCESS = "warm-complete-set-access"


def checked_controller_notice(messages, fresh, feedback, prior, require):
    """Display scheduling only; the stopped native audit must prove recovery."""
    found = [i for i, row in enumerate(messages) if i > fresh
             and row["role"] == "user" and row.get("content") == feedback]
    require(len(found) <= 1, "repeated controller recovery notice")
    if not found:
        require(prior is None, "retained controller recovery notice disappeared")
        return None
    index = found[0]
    require(messages[index] == {"role": "user", "content": feedback},
            "controller recovery notice has extra fields")
    seen, pending = set(), set()
    for row in messages[fresh+1:index]:
        if row["role"] == "assistant":
            for call in row.get("tool_calls", []):
                identifier = call["id"]
                require(isinstance(identifier, str) and identifier and identifier not in seen,
                        "controller recovery has a repeated or malformed tool call")
                seen.add(identifier)
                pending.add(identifier)
        elif row["role"] == "tool":
            identifier = row["tool_call_id"]
            require(identifier in pending, "controller recovery has an unmatched tool result")
            pending.remove(identifier)
    require(not pending, "controller recovery interrupted an unresolved tool call")
    prefix = json.dumps(messages[:index+1], sort_keys=True, separators=(",", ":"),
                        ensure_ascii=False).encode()
    observed = {"fresh_input_history_index": fresh, "notice_history_index": index,
                "history_prefix_sha256": hashlib.sha256(prefix).hexdigest()}
    require(prior is None or prior == observed, "retained controller recovery prefix changed")
    return observed


def configured_controller_target(config_source, realm):
    """Expected complete target from the pre-runtime, pinned fixture config."""
    config = tomllib.loads(config_source.decode())
    selected_realm = config["realm"][realm]
    binding_name = selected_realm["default_binding"]
    binding = selected_realm["binding"][binding_name]
    backend_name = binding["backend_profile"]
    backend = selected_realm["backend"][backend_name]
    model = config["self_hosted"]["models"]["fixture-original-model"]
    server = config["self_hosted"]["servers"][model["server"]]
    return {"kind": "model", "model": "fixture-original-model",
        "provider": backend["provider"], "self_hosted_server_id": model["server"],
        "auth_binding": {"realm": realm, "binding": binding_name},
        "credential": {"realm": realm, "binding": binding_name},
        "wire_model": model["remote_model"], "backend_profile_id": backend_name,
        "backend_kind": backend["backend_kind"],
        "endpoint": server["base_url"].rstrip("/") + "/chat/completions",
        "hosted_capabilities": [], "usage": "controller_inference"}


def checked_controller_audit(v, state, session_id, input_id, run_id, target):
    """Historical exact-input audit, never a live authorization or receipt API."""
    require = v.require
    require(state["input_id"] == input_id and state["last_run_id"] == run_id,
            "controller audit belongs to another input/run")
    require(state["current_state"] == "consumed"
            and state["terminal_outcome"] == {"outcome_type": "consumed"},
            "controller recovery input was not consumed")
    terminal = state["terminal_completion"]
    require(terminal["input_id"] == terminal["owner_input_id"] == input_id
            and terminal["completion_input_ids"] == [input_id]
            and terminal["batch_key"] == {"scope": "run", "run_id": run_id}
            and terminal["outcome"]["completion_type"] == "completed"
            and terminal["phase"]["phase"] == "finalized"
            and terminal["phase"]["finalization"] == "succeeded",
            "controller recovery lacks this completed run's finalized state")
    require(target["kind"] == "model" and target["provider"] == "self_hosted"
            and target["usage"] == "controller_inference" and target["hosted_capabilities"] == [],
            "selected recovery target is not the fixture controller")
    records = state["authorization_audit"]
    require(records and all(row["observation"]["run_id"] == run_id
            and [item["input_id"] for item in row["contributors"]] == [input_id]
            for row in records), "controller audit has foreign contributors or run")
    operations = {}
    for index, record in enumerate(records):
        row = record["observation"]
        identifier, observed = row["operation_id"], row["observation"]
        require(isinstance(identifier, str) and identifier, "native audit operation id missing")
        operation = operations.setdefault(identifier, {"id": identifier, "indices": [],
            "target": None, "prepared": [], "refused": [], "entry": None, "outcome": None,
            "ready": False, "last_kind": None})
        kind = observed["kind"]
        require(kind in ("prepared", "refused", "entry", "outcome"),
                "controller audit includes unavailable or unsupported observations")
        require(operation["outcome"] is None, "native operation continued after its outcome")
        if kind in ("prepared", "refused"):
            actual = observed["target"]
            require(operation["entry"] is None and (operation["target"] is None
                    or operation["target"] == actual), "native pre-entry target or attempt changed")
            operation["target"] = actual
            if actual["kind"] == "model":
                require(actual == target, "native model differs from the complete configured controller")
            if kind == "refused":
                require(actual["kind"] == "model" and observed["reason"] == "reprepare_required",
                        "native audit includes denied, malformed or refused non-model effects")
                operation["refused"].append(index)
                operation["ready"] = False
            else:
                operation["prepared"].append({"index": index, "policy": observed["policy"]})
                operation["ready"] = True
        elif kind == "entry":
            require(operation["ready"] and operation["entry"] is None,
                    "native entry lacks a fresh successful preparation")
            operation["entry"] = index
            operation["ready"] = False
        else:
            require(operation["entry"] is not None, "native outcome has no matching entry")
            outcome = observed["outcome"]
            if operation["target"]["kind"] == "model":
                require(outcome == {"kind": "http_response", "status": 200},
                        "native controller entry did not return HTTP200")
            else:
                require((outcome["kind"] == "tool_dispatch_returned"
                         and outcome["result_is_error"] is False and outcome["terminal_error"] is None)
                        or outcome == {"kind": "source_read_materialized"},
                        "native non-model entry did not return a successful observed result")
            operation["outcome"] = index
        operation["indices"].append(index)
        operation["last_kind"] = kind
    models, abandoned = [], []
    for operation in operations.values():
        require(operation["entry"] is None or operation["outcome"] is not None,
                "native audit ends with an unmatched entry")
        if operation["target"]["kind"] != "model":
            continue
        require(not models or models[-1]["indices"][-1] < operation["indices"][0],
                "native model operations overlap or resume an old operation")
        models.append(operation)
        if operation["entry"] is None:
            require(operation["refused"] and operation["last_kind"] == "refused",
                    "native model preparation has no terminal entry or refusal")
            abandoned.append(operation)
    require(len(abandoned) == 1, "recovery lacks one unique never-entered refused model operation")
    refused = abandoned[0]
    position = models.index(refused)
    require(position + 1 < len(models), "refused controller has no distinct replacement")
    replacement = models[position + 1]
    require(replacement["id"] != refused["id"] and replacement["target"] == refused["target"]
            and replacement["entry"] is not None and replacement["outcome"] is not None,
            "recovery did not enter a distinct exact-target controller successfully")
    # Notice-to-audit correspondence is at exact input/run and unique operation
    # level. It is not an invented link from a display row to every tool audit.
    policy = replacement["prepared"][-1]["policy"]
    return {"session_id": session_id, "input_id": input_id, "run_id": run_id,
        "refused_operation_id": refused["id"], "refusal_count": len(refused["refused"]),
        "refused_audit_indices": refused["refused"], "reason": "reprepare_required",
        "recovery_operation_id": replacement["id"], "recovered_http_status": 200,
        "replacement_audit_indices": replacement["indices"], "complete_target": target,
        "policy_publication_sequence": policy["publication_sequence"],
        "policy_observed_at_ms": policy["observed_at_ms"],
        "ordered_model_operations": [{"operation_id": item["id"], "audit_indices": item["indices"],
            "pre_entry_reprepare_indices": item["refused"], "entry_index": item["entry"],
            "http200_outcome_index": item["outcome"]} for item in models],
        "all_audit_rows_retained": len(records)}



class FirstPageUnavailable(Exception):
    """Exact authenticated first-page GET refusal, with no returned view."""

    def __init__(self, http_index):
        super().__init__("first-page status observation is unavailable")
        self.http_index = http_index


async def first_page_observation(read, validate, actions, deadline, *,
                                 now=time.monotonic, pause=asyncio.sleep):
    """Record explicit First-page actions; never retain or retry an offer."""
    action = None
    while True:
        if now() >= deadline:
            raise AssertionError("warm First-page observation deadline ended")
        try:
            data, http_index = await read()
        except FirstPageUnavailable as error:
            if action is not None:
                action.update(state="refused", fresh_http_index=error.http_index)
            if len(actions) >= 3:
                raise AssertionError("warm First-page action bound exhausted") from error
            action = {"action": "first_page", "failed_http_index": error.http_index,
                      "reason": "status_observation_unavailable_not_classified_transient",
                      "discarded_presentation": True, "selected_page_or_offer": None,
                      "delay_seconds": .25, "state": "requested"}
            actions.append(action)
            await pause(.25)
            continue
        except BaseException:
            if action is not None:
                action["state"] = "fresh_observation_failed"
            raise
        if action is not None:
            try:
                access = validate(data)
            except BaseException:
                action.update(state="invalid_fresh_snapshot", fresh_http_index=http_index)
                raise
            action.update(fresh_http_index=http_index, page_id=data["page"]["page_id"])
            if access is None:
                action.update(state="fresh_snapshot_unavailable", access_state="unavailable")
            else:
                action.update(state="fresh_snapshot_validated", offer_id=access["offer_id"])
        # A fresh unavailable access projection is a valid presentation, not an
        # access offer. Each protected action keeps its own current authority.
        return data


def create_scenario(v, console, origin):
    require, nonempty = console.require, console.nonempty

    def policy_selection(second):
        return {"tools": [{"name": v.ACQUISITION_OPERATION, "consequence": "r2"}]
                + ([{"name": Y_OPERATION, "consequence": "r2"}] if second else []),
                "catalog": "r2"}

    def service_value(content):
        decoder, values, offset = json.JSONDecoder(), [], 0
        while offset < len(content):
            while offset < len(content) and content[offset].isspace():
                offset += 1
            if offset == len(content):
                break
            _, end = decoder.raw_decode(content, offset)
            values.append(v.unique_json(content[offset:end].encode()))
            require(len(values) <= 2, "unexpected service result block")
            offset = end
        require(len(values) == 2 and set(values[1]) == {"value"}
                and isinstance(values[1]["value"], str), "text/structured service pair missing")
        require(v.canonical(values[0]) == v.canonical(v.unique_json(values[1]["value"].encode())),
                "service text and structured value disagree")
        return values[0]

    def policy_successor(proposal, prior_raw, session, second):
        # Same native candidate contract as reviewed_original_catalog_policy,
        # with an already present catalog permitted on the second exact review.
        prior = v.unique_json(prior_raw)
        selected = policy_selection(second)
        raw = proposal["candidate_canonical_json"].encode()
        candidate = v.unique_json(raw)
        require(set(proposal) == {"provider_id", "policy_id", "member", "selection", "catalog_session",
                    "prior", "candidate", "candidate_canonical_json", "lifetime"}
                and proposal["provider_id"] == prior["provider_id"]
                and proposal["policy_id"] == prior["policy_id"] and proposal["member"] == "original"
                and proposal["selection"] == selected and proposal["catalog_session"] == session
                and proposal["lifetime"] == "until_superseded"
                and proposal["prior"] == {"revision": prior["revision"], "digest": v.installed_fixture_policy_digest(prior_raw)}
                and proposal["candidate"] == {"revision": prior["revision"] + 1, "digest": v.installed_fixture_policy_digest(raw)},
                "review is not the exact current complete policy successor")
        expected = copy.deepcopy(prior)
        expected["revision"] += 1
        members = [row for row in expected["members"] if row["member_identity"] == "original"]
        require(len(members) == 1, "exact Original policy member missing")
        grants = members[0]["grants"]
        additions = selected["tools"] + [{"name": name, "consequence": "r2"}
            for name in ("tool_catalog_search", "tool_catalog_load")]
        for addition in additions:
            grant = {"tool_name": addition["name"], "action": "invoke", "consequence": addition["consequence"]}
            current = [row for row in grants if row["tool_name"] == grant["tool_name"]]
            require(not current or current == [grant], "review changed an existing policy grant")
            if not current:
                grants.append(grant)
        grants.sort(key=lambda row: row["tool_name"])
        source = candidate["source"]
        require(set(source) == {"source_id", "source_digest"}
                and source["source_id"] == "toolkit-application-service-decision"
                and source["source_digest"].startswith("sha256:") and len(source["source_digest"]) == 71
                and all(c in "0123456789abcdef" for c in source["source_digest"][7:]),
                "native candidate source provenance missing")
        expected["source"] = source
        payload = {key: expected[key] for key in ("schema_version", "provider_id", "policy_id", "revision",
                   "source", "default_deny", "members")}
        expected["policy_digest"] = v.installed_fixture_policy_digest(json.dumps(
            payload, separators=(",", ":"), ensure_ascii=False).encode())
        require(candidate == expected and raw == json.dumps(expected, separators=(",", ":"),
                ensure_ascii=False).encode() + b"\n", "review widened unrelated members or policy bytes")
        return raw

    def checked_context_text(text, expected, member):
        require(isinstance(text, str), "native context text missing")
        value = v.unique_json(text.encode())
        require(isinstance(value, dict) and set(value) == set(member["fields"]) | {"expires_at_ms"},
                "native context field set changed")
        expiry = value["expires_at_ms"]
        require(type(expiry) is int and expected["observed_before_answer_ms"] < expiry
                <= member["reviewed_expiry_max_ms"]
                and {key: item for key, item in value.items() if key != "expires_at_ms"} == member["fields"]
                and text == v.canonical(value).decode(),
                "native context differs from checked review or bounded expiry")

    class WarmProvider(v.InstalledAcquisitionProvider):
        original_catalog = True
        controller_reprepare = True

        def __init__(self, directory, events):
            self.first_pair_settled = threading.Event()
            self.c_answer = None
            self.first_business = None
            self.context_expectations = []
            self.context_observations = []
            self.b_final_history = None
            self.controller_recovery = {"b": None, "c": None}
            self.controller_history = {"b": None, "c": None}
            super().__init__(directory, events)

        def record_controller_notice(self, phase, messages, fresh):
            previous = self.controller_history[phase]
            require(previous is None or messages[:len(previous)] == previous,
                    "retained current-run controller history changed")
            prior = self.controller_recovery[phase]
            current = checked_controller_notice(messages, fresh, v.ORIGINAL_CONTROLLER_FEEDBACK, prior, require)
            self.controller_recovery[phase] = current
            self.controller_history[phase] = copy.deepcopy(messages)
            return None if current is None else current["notice_history_index"]

        def checked_original_catalog_history(self, messages, fresh):
            recovery = self.record_controller_notice("b", messages, fresh)
            # Only the exact separately checked display notice is projected
            # away for the unchanged catalog validator. The actual request and
            # its retained full prefix are never edited or replayed.
            catalog = messages if recovery is None else messages[:recovery] + messages[recovery+1:]
            v.original_catalog_history(catalog, self.request_text, self.operation, fresh,
                service_context=self.expected_service_context)
            return recovery

        def bind_context_expectation(self, expectation):
            # Called under the observation mutex before the HTTP answer. All
            # identities and display fields come from the checked review.
            require(len(self.context_expectations) < 2
                    and len(expectation["members"]) == len(self.context_expectations) + 1,
                    "unexpected service context expectation stage")
            require(not any(row["original"] == expectation["original"]
                            for row in self.context_expectations), "context expectation reused an Original")
            self.context_expectations.append(copy.deepcopy(expectation))

        def checked_context_block(self, messages, fresh, index):
            require(index < len(self.context_expectations), "native context preceded its checked review")
            expected = self.context_expectations[index]
            members = expected["members"]
            require(fresh >= len(members) and messages[fresh] == {
                "role": "user", "content": expected["instruction"]}, "context instruction changed")
            observed = []
            for message, member in zip(messages[fresh-len(members):fresh], members):
                require(set(message) == {"role", "content"} and message["role"] == "user"
                        and isinstance(message["content"], str), "native context block shape changed")
                checked_context_text(message["content"], expected, member)
                observed.append(copy.deepcopy(message))
            # Native parent attenuation is not recomputed from panel data.
            # Retaining validated bytes below checks history stability only.
            if index == len(self.context_observations):
                self.context_observations.append(observed)
            else:
                require(index < len(self.context_observations)
                        and self.context_observations[index] == observed,
                        "historical native context bytes changed")
            return observed

        def checked_b_history(self, body):
            messages = body["messages"]
            originals = [i for i, row in enumerate(messages)
                         if row["role"] == "user" and row.get("content") == self.request_text]
            require(len(originals) == 2, "B does not retain the exact A and fresh B inputs")
            observed = self.checked_context_block(messages, originals[1], 0)
            # The inherited exact B history check consumes only the context
            # already checked against the independent pre-answer expectation.
            self.expected_service_context = v.unique_json(observed[0]["content"].encode())
            self.checked_original_catalog_history(messages, originals[1])

        def checked_c_history(self, body):
            messages = body["messages"]
            fresh = [i for i, row in enumerate(messages)
                     if row["role"] == "user" and row.get("content") == WARM_REQUEST]
            originals = [i for i, row in enumerate(messages)
                         if row["role"] == "user" and row.get("content") == self.request_text]
            require(len(fresh) == 1 and len(originals) == 2 and originals[1] < fresh[0]
                    and self.b_final_history is not None, "C lacks the exact settled B history")
            fresh = fresh[0]
            prefix = self.b_final_history
            require(messages[:len(prefix)] == prefix
                    and messages[len(prefix):fresh-2] == [{"role": "assistant", "content": B_FINISH, "tool_calls": []}],
                    "C changed B's history or added input before the context block")
            self.checked_context_block(messages, originals[1], 0)
            self.checked_context_block(messages, fresh, 1)
            recovery = self.record_controller_notice("c", messages, fresh)
            operations = [self.operation, Y_OPERATION]
            loads = {}
            for prefix_name, operation in zip(("x", "y"), operations):
                found = [i for i, row in enumerate(messages) if row["role"] == "tool"
                         and row.get("tool_call_id") == "warm-c-load-" + prefix_name]
                require(len(found) <= 1 and (not found or found[0] > fresh), "C catalog load crossed input")
                loads[operation] = found[0] if found else len(messages)
            boundaries, added, removed = set(), set(), set()
            subsets = [[], [operations[0]], [operations[1]], sorted(operations)]
            for i, message in enumerate(messages[fresh+1:], fresh+1):
                if message["role"] != "user":
                    continue
                if i == recovery:
                    continue
                content = message.get("content")
                if v.original_tool_notice(content, self.operation) == "boundary":
                    stage = sum(position < i for position in loads.values())
                    require(stage not in boundaries, "duplicate C tool boundary notice")
                    boundaries.add(stage)
                    continue
                matches = []
                # Exact selected Core display forms, in its sorted-name and
                # added-then-loaded order. No pending or removed-source notice.
                for new in subsets:
                    for loaded in subsets:
                        if not (new or loaded) or set(new) & set(loaded):
                            continue
                        parts = (["new deferred tools available: " + ", ".join(new)] if new else [])
                        parts += (["deferred tools now available to call: " + ", ".join(loaded)] if loaded else [])
                        expected = ("Deferred catalog changed at turn boundary: " + "; ".join(parts)
                                    + "\nTool configuration changed: deferred catalog updated")
                        if content == expected:
                            matches.append((new, loaded))
                require(len(matches) == 1, "unexpected user input or C catalog notice")
                new, loaded = matches[0]
                require(all(name not in added and i < loads[name] for name in new)
                        and all(name in added and name not in removed and i > loads[name] for name in loaded),
                        "C deferred notice crossed its exact load or repeated")
                added.update(new)
                removed.update(loaded)

        def access_arguments(self, job_id, raw):
            return {**super().access_arguments(job_id, raw), "tool_policy": policy_selection(False)}

        def public_reply(self):
            return {"state": "reviewed", "access_reviewed": True, "grant_issued": True,
                    "fresh_instruction_retained": True, "ready": False}

        def warm_results(self, body):
            result = {}
            for message in body["messages"]:
                if message["role"] != "tool":
                    continue
                identifier, content = message["tool_call_id"], message["content"]
                if identifier in (v.ORIGINAL_STORY_CALL, C_STORY):
                    value = {"content": content, "story": v.installed_public_story(content)}
                elif identifier == C_WIKI:
                    value = {"content": content, "article": service_value(content)}
                else:
                    value = v.unique_json(content.encode())
                require(identifier not in result, "duplicate actual warm tool result")
                require(identifier not in self.results or self.results[identifier] == value,
                        "retained warm tool result changed")
                result[identifier] = value
                self.results[identifier] = value
            return result

        def tool(self, body, identifier, name, args):
            names = {row["function"]["name"] for row in body["tools"]}
            require(name in names and identifier not in self.issued,
                    "warm call is unavailable or would replay an effect: " + identifier)
            self.issued.add(identifier)
            self.events.add("controlled_model_tool", model=body["model"], call_id=identifier, tool=name, args=args)
            return {"delta": {"tool_calls": [{"index": 0, "id": identifier, "type": "function",
                    "function": {"name": name, "arguments": json.dumps(args)}}]}, "finish_reason": "tool_calls"}

        def done(self, text):
            return {"delta": {"content": text}, "finish_reason": "stop"}

        def response(self, body):
            lane = body["model"]
            require(lane in ("fixture-original-model", "fixture-helper-model"), "unknown warm model owner")
            warm_input = any(row["role"] == "user" and WARM_REQUEST in str(row.get("content", ""))
                             for row in body["messages"])
            warm_find_returned = any(row["role"] == "tool" and row.get("tool_call_id") == B_FIND
                                     for row in body["messages"])
            if lane == "fixture-original-model":
                if warm_input:
                    self.checked_c_history(body)
                elif sum(row["role"] == "user" and row.get("content") == self.request_text
                         for row in body["messages"]) == 2:
                    self.checked_b_history(body)
            if lane == "fixture-original-model" and warm_find_returned and not warm_input:
                results = self.warm_results(body)
                self.check_find(results[B_FIND])
                require(self.b_final_history is None, "B final response was replayed")
                self.b_final_history = copy.deepcopy(body["messages"])
                return self.done(B_FINISH)
            if warm_input:
                results = self.warm_results(body)
                return self.original_c(body, results) if lane == "fixture-original-model" else self.helper_y(body, results)
            reply = super().response(body)
            if lane == "fixture-original-model" and self.business_observation is not None:
                require(self.first_business is None and v.ORIGINAL_STORY_CALL in self.results,
                        "B public observation was replayed")
                self.first_business = copy.deepcopy(self.business_observation)
                # This waits only for the observer's actual native A/helper
                # completion frames. The observer never blocks on this lock.
                # Handler owns this mutex on entry. The helper's final model
                # response needs the same mutex, so release only this observation
                # custody while waiting for its actual native terminal frame.
                self.lock.release()
                try:
                    settled = self.first_pair_settled.wait(
                        min(30.0, max(0.0, self.deadline - time.monotonic())))
                finally:
                    self.lock.acquire()
                require(settled, "A and first helper did not settle before B's later acquisition")
                require(time.monotonic() < self.deadline, "warm observer retired before effect selection")
                return self.tool(body, B_FIND, "find_and_install", {
                    "request": WARM_REQUEST, "user_context": [], "caller_observations": []})
            return reply

        def check_find(self, result):
            require(nonempty(result.get("operation")) and result.get("created") is True
                    and result.get("worker") == {"state": "input_admitted", "deduplicated": False,
                        "source_settlement": "fresh_acceptance_promoted"}, "warm helper input was not freshly admitted")
            require(result["operation"] != self.results["original-request"]["operation"],
                    "B acquisition reused A's canonical acquisition")

        def helper_y(self, body, results):
            # The same retained helper may include both acquisition histories.
            # Only IDs issued in this second input drive its new effects.
            require("public-task-access" in results and results["public-task-access"] == self.public_reply(),
                    "warm helper lacks the first real access result")
            phase = len(self.requests)
            if "warm-x-job" not in results:
                return self.tool(body, "warm-x-job", "setup_job", {
                    "job_id": self.results["installed-job"]["job"]["id"]})
            require(results["warm-x-job"]["job"]["id"] == self.results["installed-job"]["job"]["id"]
                    and results["warm-x-job"]["job"]["status"] == "succeeded", "retained X job is not current/successful")
            if "warm-y-help" not in results:
                return self.tool(body, "warm-y-help", "setup_help", {"family": "agno", "profile": "wikipedia"})
            if "warm-y-check" not in results:
                return self.tool(body, "warm-y-check", "setup_check", Y_SELECTION)
            checked = results["warm-y-check"]
            self.checked_configuration(checked)
            if "warm-y-install" not in results:
                return self.tool(body, "warm-y-install", "setup_install", {**Y_SELECTION,
                    "validation_id": checked["validation_id"], "configuration_sha256": checked["configuration_sha256"]})
            job = results["warm-y-install"]["job"]["id"]
            current = [value for value in results.values() if isinstance(value, dict)
                       and isinstance(value.get("job"), dict) and value["job"].get("id") == job][-1]
            require(current["job"]["status"] in ("running", "succeeded"), "Y installation failed")
            if current["job"]["status"] == "running":
                return self.tool(body, "warm-y-poll-" + str(phase), "setup_job", {"job_id": job, "wait_ms": 10000})
            if "warm-y-reconciled" not in results:
                return self.tool(body, "warm-y-reconciled", "setup_reconcile", {"job_id": job})
            observation = results["warm-y-reconciled"]["observation"]
            require(observation["job_id"] == job and observation["status"] == "matched", "Y readback did not reconcile")
            if "warm-y-current-help" not in results:
                return self.tool(body, "warm-y-current-help", "setup_help", {"family": "agno", "profile": "wikipedia"})
            metadata = results["warm-y-current-help"]
            self.check_authentication_metadata(metadata)
            require([row["tool"] for row in metadata["operations"] if row["id"] == Y_OPERATION] == [Y_OPERATION],
                    "actual Wikipedia operation metadata differs")
            if Y_ACCESS not in results:
                return self.tool(body, Y_ACCESS, self.access_tool, {
                    "services": [{"job_id": self.results["installed-job"]["job"]["id"], "operations": [self.operation]},
                                 {"job_id": job, "operations": [Y_OPERATION]}],
                    "agent": "original", "expires_in_ms": None, "tool_policy": policy_selection(True)})
            self.check_access_result(results[Y_ACCESS])
            self.finished.set()
            return self.done(self.helper_finish())

        def original_c(self, body, results):
            require(B_FIND in results and self.first_business is not None, "C lacks actual B history")
            self.check_find(results[B_FIND])
            for prefix, operation in (("x", self.operation), ("y", Y_OPERATION)):
                search, loaded = "warm-c-search-" + prefix, "warm-c-load-" + prefix
                if search not in results:
                    return self.tool(body, search, "tool_catalog_search", {"query": operation, "limit": 50})
                found = results[search]
                matches = [row for row in found["results"] if row["name"] == operation]
                require(found["catalog_exact"] is True and not found.get("pending_sources", [])
                        and len(matches) == 1 and matches[0]["visibility_status"] in ("loaded", "deferred"),
                        "C catalog lacks an exact current operation")
                if loaded not in results:
                    return self.tool(body, loaded, "tool_catalog_load", {"names": [operation]})
                v.exact_catalog_load(results[loaded], [operation])
            if C_STORY not in results:
                return self.tool(body, C_STORY, self.operation, {"num_stories": 1})
            if C_WIKI not in results:
                return self.tool(body, C_WIKI, Y_OPERATION, {"query": Y_QUERY})
            story, article = results[C_STORY]["story"], results[C_WIKI]["article"]
            meta = article["meta_data"]
            require(isinstance(article, dict) and nonempty(article["content"])
                    and meta["language"] == "en" and type(meta["page_id"]) is int and meta["page_id"] > 0
                    and nonempty(meta["canonical_title"]) and meta["source_url"].startswith("https://en.wikipedia.org/wiki/")
                    and nonempty(meta["retrieved_at"]), "Wikipedia did not return actual citation metadata")
            answer = "Hacker News story " + str(story["id"]) + ": " + story["title"] + ". Wikipedia: " + meta["source_url"]
            require(self.c_answer is None, "C answer was replayed")
            self.c_answer = {"answer": answer, "story": story, "article": article,
                "x_result": results[C_STORY]["content"], "y_result": results[C_WIKI]["content"],
                "request_index": len(self.requests) - 1,
                "request_sha256": hashlib.sha256(v.canonical(body)).hexdigest()}
            return self.done(answer)

    class WarmScenario(console.ConsoleScenario):
        name = "installed-console-warm-success"
        scope = ("controlled installed SDK/Console A/helper/B/helper/C consumer; actual retained X job reuse, "
                 "fresh complete-set C review/grants and native Console run completion; private input receipts "
                 "are not observed; no Browser, live account, autonomous model strategy, D5 sharing or process-restart qualification")

        def configure_owner(self, owner, source):
            super().configure_owner(owner, source)
            helper = next(row for row in owner["native_host"]["members"] if row["member"]["member"] == "helper")
            helper["audience"] = {"audience": "principal", "principal": self.human()}
            self.source_owner = copy.deepcopy(owner)
            source["purpose"] = self.scope
            source["warm_fixture"] = {"first_request": self.original_request(), "later_request": WARM_REQUEST,
                "selection": "same-audience configured setup before immutable binding",
                "services": ["hackernews", "wikipedia"], "software_install_count": 2}

        def configure_plan(self, plan):
            super().configure_plan(plan)
            plan["acquisition"]["console_setup"]["access_management"] = "requester_issued_leaves"
            self.realm_state_root = plan["application"]["realm"]["state_root"]
            self.source_application = copy.deepcopy(plan["application"])

        def access_snapshot(self, data):
            # The pinned application selects this existing requester policy.
            # The live panel must independently expose its exact typed offer.
            access, page = data.get("application_access"), data.get("page")
            require(isinstance(access, dict) and set(access) == {"scope", "rows", "offer_id"}
                    and access["scope"] == "application_issued_access"
                    and isinstance(access["rows"], list) and len(access["rows"]) <= 5
                    and all(isinstance(row, dict) and nonempty(row.get("id")) for row in access["rows"])
                    and len({row["id"] for row in access["rows"]}) == len(access["rows"])
                    and type(access["offer_id"]) is int and 0 < access["offer_id"] <= 2**53-1
                    and isinstance(page, dict) and type(page.get("page_id")) is int
                    and page["page_id"] == access["offer_id"]
                    and page.get("first") is True and page.get("has_next") is False,
                    "exact requester-issued access capability is unavailable or incomplete")
            return access

        def access_presentation(self, data):
            access, page = data.get("application_access"), data.get("page")
            if access == {"scope": "application_issued_access", "state": "unavailable"}:
                require(isinstance(page, dict) and set(page) == {"page_id", "first", "has_next"}
                        and type(page["page_id"]) is int and 0 < page["page_id"] <= 2**53-1
                        and page["first"] is True and page["has_next"] is False,
                        "unavailable access presentation lacks its exact fresh page")
                return None
            return self.access_snapshot(data)

        def observe_completed_setup(self, setup, artifact):
            # The finalizer already checked this native output against its
            # completed manifest and published selection before native Ready.
            # Its composed role is observed metadata, never an admission key.
            source = self.source_application
            source_member = source["helper"]["member"]
            helper = setup["helper"]
            require(source_member == {"mob_id": "fixture", "role": "research", "member": "helper"}
                    and setup["source_helper_member"] == source_member
                    and setup["helper_profile"] == source["helper_profile"] == "dedicated"
                    and setup["definition_source"] == source["definition"]
                    and helper["member"]["mob_id"] == source_member["mob_id"]
                    and helper["member"]["member"] == source_member["member"]
                    and nonempty(helper["member"]["role"])
                    and helper["member"]["role"] in setup["definition"]["profiles"],
                    "completed helper differs from the selected source application")
            members = [row for row in setup["native_members"]
                       if row["member"]["member"] == source_member["member"]]
            require(len(members) == 1 and members[0]["member"] == helper["member"]
                    and members[0]["logical_owner"] == helper["logical_executor"]
                        == v.installed_fixture_principal("helper")
                    and members[0]["audience"] == {"audience": "principal", "principal": self.human()},
                    "completed native member differs from the selected helper")
            self.expected_helper_member = copy.deepcopy(helper["member"])
            self.completed_helper_observation = {"completed_setup": copy.deepcopy(artifact),
                "source_member": copy.deepcopy(source_member),
                "native_member": copy.deepcopy(members[0]),
                "scope": "completed native selection metadata; live admission remains native-owned"}

        def configure_runtime(self, config, directory):
            super().configure_runtime(config, directory)
            # Observe helper settlement; this adds no send authority for it.
            with open(config.access_config_path, "a") as output:
                output.write('\n[[rules]]\nid = "controlled-console-helper-observation"\nsubjects = ["'
                             + console.EMAIL + '"]\nactions = ["agent.view"]\nagents = ["helper"]\n')

        def provider(self, directory, events):
            provider = WarmProvider(directory, events)
            self.controller_configuration = v.config_text(provider.port, acquisition=True).encode()
            return provider

        def access_review(self, review, accepted, results, contexts, policies):
            second = bool(contexts)
            body = review["prompt"]["body"]
            require(body["kind"] == "public_services_with_setup_v2"
                    and len(body["members"]) == (2 if second else 1), "expected exact setup-enabled reviewed set")
            request, setup = body["access_request"], body["setup"]
            context = request["service_instruction"]
            fresh = context["fresh_original"]
            expected_request = WARM_REQUEST if second else self.original_request()
            operation = results[B_FIND if second else "original-request"]["operation"]
            native = self.source_owner["native_host"]
            source_route = native["routes"][0]
            parent_owner = {"operator": native["remote_operator"], "application": native["grant_administrator"],
                "realm": native["realm"], "route": v.ACQUISITION_ROUTE, "purpose": "operations",
                "represented_subject": self.human()}
            require(context["proposed_fresh_instruction"] == expected_request and nonempty(fresh)
                    and context["business_task"] == {**self.task_scope, "id": operation}
                    and console.original_work_reference(context["prior_original"], self.source_authority)
                    and context["operator_and_requester"] == self.human()
                    and context["represented_subject"] == self.human()
                    and context["ingress_actor"] == source_route["source_authority"]
                    and context["audience"] == {"audience": "principal", "principal": self.human()}
                    and context["configuration_persistence"] == "not authorized by this review",
                    "fresh review crossed acquisition, requester or persistence scope")
            if second:
                require(fresh != contexts[0]["fresh_original"]
                        and context["prior_original"] != contexts[0]["prior_original"], "C reused B source identity")
            helper = setup["helper"]
            configured = request["configured_setup"]
            require(configured["selection"] == setup and nonempty(helper["session_id"])
                    and helper["member"] == self.expected_helper_member
                    and setup["configured"]["session_id"] == accepted["session_id"]
                    and setup["configured"]["capability"]["route"] == v.ACQUISITION_ROUTE
                    and helper["logical_executor"] == v.installed_fixture_principal("helper")
                    and helper["audience"] == {"audience": "principal", "principal": self.human()}
                    and configured["processors"] == [v.installed_fixture_principal("original-agent"), helper["logical_executor"]]
                    and configured["sessions"] == [accepted["session_id"], helper["session_id"]]
                    and configured["audience"] == helper["audience"]
                    and type(configured["expires_at_ms"]) is int, "review omitted actual 2x2 metadata scope")
            if second:
                require(helper == self.reviewed_helper, "later acquisition changed the selected live helper")
            else:
                self.reviewed_helper = copy.deepcopy(helper)
            expected = {"hackernews": (v.ACQUISITION_OPERATION, results["reconciled-install"],
                        results["installed-job"]["job"]["id"])}
            if second:
                expected["wikipedia"] = (Y_OPERATION, results["warm-y-reconciled"], results["warm-y-install"]["job"]["id"])
            decisions, installations = [], {}
            for member in body["members"]:
                parent = member["parent"]
                require(all(parent.get(key) == value for key, value in parent_owner.items()),
                        "service parent differs from actual configured native issuer and represented subject")
                document, access = parent["access_request"], parent["service_access"]
                software = document["software"]
                profile = software["profile"]
                require(profile in expected and profile not in installations, "unexpected or duplicate reviewed service")
                op, reconciled, job = expected[profile]
                observation = reconciled["observation"]
                installed = observation["result"]
                checked = results["warm-y-check" if profile == "wikipedia" else "checked-install"]
                require(observation["job_id"] == job and observation["status"] == "matched"
                        and installed["family"] == "agno" and installed["profile"] == profile,
                        "review lacks the exact successful reconciled job")
                configuration = installed["mcp_configuration"]
                require(set(configuration) == {"mcpServers"}, "unexpected public configuration fields")
                servers = configuration["mcpServers"]
                require(len(servers) == 1, "installed job has more than one service")
                server = next(iter(servers))
                launch = servers[server]
                require(isinstance(launch, dict) and set(launch) <= {"command", "args", "env", "secret_env"}
                        and nonempty(launch.get("command")) and Path(launch["command"]).is_absolute()
                        and "\0" not in launch["command"] and isinstance(launch.get("args", []), list)
                        and all(isinstance(arg, str) and "\0" not in arg for arg in launch.get("args", []))
                        and launch.get("env", {}) == {} and launch.get("secret_env", []) == [],
                        "reconciled public launch has unexpected transport or credential fields")
                # Match selected public_launch_configuration and Core's stdio
                # serialization: empty secret_env and unset optional fields omit.
                expected_configuration = {"name": server, "command": launch["command"],
                    "args": launch.get("args", []), "env": {}, "access": {"allowed_operations": [op]}}
                expected_selection = {"operations": [op], "agent": "original", "expires_in_ms": None}
                if not second:
                    expected_selection["tool_policy"] = policy_selection(False)
                permission, destination = access["permission"], access["live_attachment"]["destination"]
                require(member["server"] == server and software["job_id"] == job
                        and software["family"] == "agno" and software["server_name"] == server
                        and software["validation_id"] == checked["validation_id"]
                        and software["receipt_sha256"] == installed["receipt_sha256"]
                        and software["configuration_sha256"] == installed["configuration_sha256"] == checked["configuration_sha256"]
                        and software["document_sha256"] == hashlib.sha256(v.canonical(configuration)).hexdigest()
                        and document["phase"] == "installed_public_service_access"
                        and document["realm"] == native["realm"]
                        and document["realm_state_root"] == self.realm_state_root
                        and document["authentication"] == {"kind": "none"}
                        and document["operation_basis"] == {"kind": "package_published_metadata"}
                        and document["session"] == helper["session_id"]
                        and document["service_instruction"] == context
                        and document["application_selection"] == expected_selection
                        and document["installation"] == expected_configuration
                        and access["route"] == v.ACQUISITION_ROUTE and access["choice"] == "approve"
                        and permission["target"] == "original" and permission["connection"] == server
                        and permission["resource"] == server and permission["operations"] == [op]
                        and permission["lifetime"]["scope"] == {"kind": "task", "task": fresh}
                        and type(permission["lifetime"]["expires_at_ms"]) is int
                        and int(time.time() * 1000) < permission["lifetime"]["expires_at_ms"]
                        and destination["context"] == accepted["session_id"]
                        and destination["logical_owner"] == v.installed_fixture_principal("original-agent")
                        and destination["audience"] == helper["audience"], "review differs from checked job or native task scope")
                require(nonempty(access["decision"]), "native decision identity missing")
                decisions.append(access["decision"])
                installations[profile] = {"software": software, "configuration": document["installation"],
                    "decision": access["decision"], "lifetime": permission["lifetime"], "server": server}
            require(all(body["catalog"].get(key) == value for key, value in parent_owner.items()),
                    "catalog parent differs from actual configured native issuer and represented subject")
            catalog = body["catalog"]["catalog_access"]
            require(catalog["original"] == fresh and catalog["session"] == accepted["session_id"]
                    and catalog["kind"] == {"public_service_catalog_with_setup_v2": {"helper": helper}} and catalog["lifetime"]["scope"] == {"kind": "task", "task": fresh},
                    "catalog grant does not cover the exact reviewed setup task")
            decisions.append(catalog["decision"])
            require(len(set(decisions)) == len(decisions), "reviewed decision identities collide")
            if second:
                prior_x = self.reviewed_sets[0]["installations"]["hackernews"]
                x = installations["hackernews"]
                require(x["software"] == prior_x["software"] and x["configuration"] == prior_x["configuration"]
                        and set(decisions).isdisjoint(self.reviewed_sets[0]["decisions"]),
                        "X was replaced/reinstalled or C reused B decision authority")
            wrapper = request["member_tool_policy"]
            require(set(wrapper) == {"proposal", "approval_scope", "lifetime", "catalog_visibility"}
                    and wrapper["approval_scope"] == "the complete successor policy including preserved member permissions"
                    and wrapper["lifetime"] == "until superseded; task closure does not undo this separately reviewed policy"
                    and wrapper["catalog_visibility"] == "eligible tool metadata in the original session; invocation still needs its exact native target grant",
                    "complete successor policy review scope missing")
            policies.append(policy_successor(wrapper["proposal"], policies[-1] if policies else self.application_tool_policy,
                                            accepted["session_id"], second))
            self.reviewed_sets.append({"fresh_original": fresh, "decisions": decisions, "installations": installations,
                                       "catalog_decision": catalog["decision"], "catalog_lifetime": catalog["lifetime"]})
            names = [member["server"] for member in body["members"]]
            require(names == sorted(names) and len(names) == len(set(names)), "reviewed context order changed")
            observed_at = int(time.time() * 1000)
            expected_contexts = []
            for member in body["members"]:
                access = member["parent"]["service_access"]
                permission = access["permission"]
                maximum = permission["lifetime"]["expires_at_ms"]
                require(type(maximum) is int and observed_at < maximum, "review expired before context binding")
                fields = {"schema": "toolkit.approved-service-context.v1",
                    "operation": context["business_task"]["id"], "original": fresh,
                    "connection": permission["connection"], "resource": permission["resource"],
                    "allowed_operations": list(permission["operations"]),
                    "configuration_sha256": access["connection_revision"],
                    "attachment": "observed_usable_before_input_admission",
                    "meaning": "The host completed the approved integration attachment checks for this continuation. This context accompanies the unchanged reviewed user instruction. Native authorization is still checked on each use. This observation does not guarantee future readiness, extend permission, establish filesystem isolation or complete the user's task."}
                expected_contexts.append({"fields": fields, "reviewed_expiry_max_ms": maximum})
            expectation = {"original": fresh, "instruction": expected_request, "members": expected_contexts,
                "observed_before_answer_ms": observed_at,
                "scope": "display consistency only; native parent attenuation is not independently recomputed from the panel; admission and each call retain native authorization/currentness checks"}
            return console.approval_answer(review), context, expectation

        def completion_frame(self, frames, identity, run, session, text=None):
            rows = [f for f in frames if f.get("identity") == identity and f.get("run_id") == run
                    and f.get("source", {}).get("kind") == "console_event"
                    and f.get("kind") == "interaction_complete" and f["payload"].get("source_event_type") == "run_completed"]
            require(len(rows) <= 1, "duplicate native run completion")
            if not rows:
                return None
            row = rows[0]
            require(console.run_id(row) == run and row["session_id"] == session
                    and row["payload"]["session_id"] == session and row["status"] == "completed"
                    and nonempty(row["payload"].get("assistant_message_id"))
                    and row["payload"].get("extraction_required", False) is False
                    and (text is None or row["payload"]["result"] == text), "native terminal identity/result mismatch")
            return row

        def tool_pair(self, frames, run, identifier, name, args, content=None):
            selected = [f for f in frames if f.get("run_id") == run and f["payload"].get("id") == identifier]
            calls = [f for f in selected if f.get("kind") == "tool_call_requested"]
            results = [f for f in selected if f.get("kind") == "tool_result_received"]
            require(len(calls) <= 1 and len(results) <= 1, "native tool was replayed")
            if not calls or not results:
                return None
            require(all(console.run_id(f) == run and f["payload"]["name"] == name for f in calls + results)
                    and calls[0]["payload"]["args"] == args and results[0]["payload"]["is_error"] is False,
                    "native tool result differs from selected call")
            if content is not None:
                blocks = results[0]["payload"]["content"]
                require(len(blocks) == 2 and blocks[0]["type"] == "text" and blocks[1]["type"] == "structured",
                        "native service result lacks both blocks")
                require(blocks[0]["text"] + "\n" + v.canonical(blocks[1]["data"]).decode() == content,
                        "native service result differs from model-visible value")
            positions = {f["id"]: i for i, f in enumerate(frames)}
            require(positions[calls[0]["id"]] < positions[results[0]["id"]], "native result preceded call")
            return {"call": calls[0]["id"], "result": results[0]["id"]}

        async def run(self, private, provider, directory, events, process, base_url):
            require(base_url.rstrip("/") == self.origin, "listener differs from selected Console origin")
            self.reviewed_sets, self.reviewed_helper = [], None
            report = {"scope": self.scope, "status": "running", "http": [], "timeline": [],
                      "decisions": [], "starts": [], "grant_closeout": [], "first_page_actions": [],
                      "access_unavailable_observations": [],
                      "input_receipt_scope": "not requested or observed; native Console events establish run completion",
                      "completed_helper_observation": self.completed_helper_observation,
                      "login_retirement": "not_observed"}
            frames = {name: [] for name in ("original", "helper")}
            indices = {name: {} for name in frames}
            cursors = {name: None for name in frames}
            contexts, policies, answered = [], [], set()
            enrollment, token, original_error = None, None, None
            deadline = time.monotonic() + v.ACQUISITION_LIMIT
            expires = int(time.time() + v.ACQUISITION_LIMIT + 60)
            scope = "installed-console-warm-" + uuid.uuid4().hex

            def request(method, path, body=None, *, bearer=True, expected=200, text=False, first_page=False):
                require(not first_page or (method == "GET" and path == console.PANEL + "/data"
                    and body is None and bearer is True and expected == 200 and text is False),
                    "First-page observation cannot select another request")
                require(time.monotonic() < deadline and process.poll() is None, "warm Console owner/deadline ended")
                connection = HTTPConnection("127.0.0.1", self.port, timeout=min(10.0, max(.1, deadline-time.monotonic())))
                headers = {"Accept": "text/html" if text else "application/json"}
                if bearer:
                    headers["Authorization"] = "Bearer " + token
                if body is not None:
                    headers.update({"Origin": self.origin, "Content-Type": "application/json"})
                try:
                    connection.request(method, path, body=None if body is None else v.canonical(body), headers=headers)
                    response = connection.getresponse()
                    raw = response.read(4 * 1024 * 1024 + 1)
                    require(len(raw) <= 4 * 1024 * 1024, "warm HTTP response exceeds bound")
                    value = raw.decode() if text else v.unique_json(raw)
                    report["http"].append({"method": method, "path": path, "request": body,
                                           "status": response.status, "response": value})
                    if first_page and response.status == 503 and value == {"error": "Protected setup is unavailable"}:
                        raise FirstPageUnavailable(len(report["http"]) - 1)
                    require(response.status == expected, "warm HTTP failed: " + path)
                    return value
                finally:
                    connection.close()

            async def http(*args, **kwargs):
                return await asyncio.to_thread(request, *args, **kwargs)

            async def first_page():
                async def read():
                    value = await http("GET", console.PANEL + "/data", first_page=True)
                    return value, len(report["http"]) - 1

                def validate(value):
                    require(isinstance(value, dict) and set(value) == {"projection", "review", "page", "application_access"}
                            and isinstance(value["projection"], dict)
                            and (value["review"] is None or isinstance(value["review"], dict)),
                            "fresh First-page snapshot is incomplete")
                    return self.access_presentation(value)

                value = await first_page_observation(read, validate, report["first_page_actions"], deadline)
                validate(value)
                return value

            def access_for(data, stage):
                access = self.access_presentation(data)
                if access is None:
                    report["access_unavailable_observations"].append({"stage": stage,
                        "http_index": len(report["http"]) - 1, "page_id": data["page"]["page_id"],
                        "state": "unavailable", "action": "observe_again_without_access_effect"})
                return access

            async def current_access(stage):
                while time.monotonic() < deadline:
                    access = access_for(await first_page(), stage)
                    if access is not None:
                        return access
                    await asyncio.sleep(.25)
                raise AssertionError("warm access observation deadline ended")

            async def timeline(identity):
                page = await http("GET", "/console/timeline?" + urlencode({"identity": identity, "mode": "since",
                    "limit": 400, **({"after": cursors[identity]} if cursors[identity] else {})}))
                require(page.get("available", True) is True and isinstance(page.get("frames"), list),
                        "actual warm timeline unavailable")
                report["timeline"].append({"identity": identity, "page": page})
                for raw in page["frames"]:
                    frame = console.frame_record(raw)
                    previous = indices[identity].get(frame["id"])
                    if previous is None:
                        require(len(frames[identity]) < 4000, "warm native timeline exceeds bound")
                        indices[identity][frame["id"]] = len(frames[identity])
                        frames[identity].append(frame)
                    else:
                        old = frames[identity][previous]
                        require(all(old.get(key) == frame.get(key) for key in
                            ("identity", "session_id", "run_id", "kind", "source", "source_event_id")),
                            "native updated frame changed owner")
                        frames[identity][previous] = frame
                if page["frames"]:
                    cursors[identity] = page.get("next_cursor") or page.get("latest_cursor")
                    require(nonempty(cursors[identity]), "native timeline continuation missing")

            try:
                enrollment = private.begin_console_login(v.SDK.ConsoleLoginSelection(
                    issuer=console.ISSUER, subject=console.SUBJECT, login_scope_id=scope, expires_at_ms=expires*1000), timeout=10.0)
                await asyncio.to_thread(enrollment.result)
                token = console.signed_login(scope, expires)
                report["enrollment"] = {"id": enrollment.enrollment_id, "acknowledged": True,
                    "issuer": console.ISSUER, "subject": console.SUBJECT, "login_scope_id": scope}
                await http("GET", console.PANEL + "/data", bearer=False, expected=401)
                experience = await http("GET", "/console/experience")
                require(sum(row.get("panel") == console.PANEL for row in experience["console_config"]["sidebar"]["buttons"]) == 1,
                        "actual mounted setup panel missing")
                fresh_data = await first_page()
                require(fresh_data["review"] is None, "fresh warm fixture has review")
                fresh_access = self.access_snapshot(fresh_data)
                require(fresh_access["rows"] == [], "fresh warm fixture has retained access")
                report["access_capability_checks"] = [{"stage": "before_console_send", "access": copy.deepcopy(fresh_access)}]
                with provider.lock:
                    require(not provider.requests and not provider.errors, "model work preceded initial Console Send")
                    provider.enabled = True
                accepted = await http("POST", "/console/send", {"identity": "original", "content": self.original_request(),
                    "origin": "console", "origin_kind": "operator", "idempotency_key": scope})
                require(accepted["status"] == "accepted" and accepted["identity"] == "original"
                        and all(nonempty(accepted.get(k)) for k in ("session_id", "interaction_id", "input_frame_id")),
                        "initial Console admission missing")
                report["accepted"] = accepted
                while time.monotonic() < deadline:
                    # Always advance native observations before attempting the
                    # provider lock. B may hold that lock waiting for this gate.
                    await timeline("original")
                    await timeline("helper")
                    original_starts = [f for f in frames["original"] if f.get("kind") == "run_started"
                        and f.get("session_id") == accepted["session_id"]]
                    helper_starts = [f for f in frames["helper"] if f.get("kind") == "run_started"]
                    require(len(original_starts) <= 3 and len(helper_starts) <= 2, "unexpected extra warm run")
                    if original_starts and helper_starts:
                        a = self.completion_frame(frames["original"], "original", console.run_id(original_starts[0]),
                                                  accepted["session_id"], v.ORIGINAL_A_FINISH)
                        h = self.completion_frame(frames["helper"], "helper", console.run_id(helper_starts[0]),
                                                  helper_starts[0]["session_id"])
                        if a and h:
                            provider.first_pair_settled.set()
                            report.setdefault("first_pair_settled", {"a": a["id"], "helper": h["id"]})
                    # Missing snapshot is not failure or progress. No lock is
                    # held by this observer across HTTP, native reads or sleep.
                    if not provider.lock.acquire(blocking=False):
                        await asyncio.sleep(.05)
                        continue
                    try:
                        require(not provider.errors, "controlled warm provider refused actual input")
                        results = copy.deepcopy(provider.results)
                        first_business, c_answer = copy.deepcopy(provider.first_business), copy.deepcopy(provider.c_answer)
                        report["service_context_display"] = {
                            "expectations": copy.deepcopy(provider.context_expectations),
                            "validated_observed_blocks": copy.deepcopy(provider.context_observations)}
                        report["controller_recovery_display"] = copy.deepcopy(provider.controller_recovery)
                    finally:
                        provider.lock.release()
                    data, review = None, None
                    data = await first_page()
                    review = data["review"]
                    if review is not None:
                        if review["can_start"] and review["review_id"] not in report["starts"]:
                            require(review["prompt"] is None and len(report["starts"]) < 2,
                                    "unexpected additional setup start")
                            await http("POST", console.PANEL + "/start", {"review_id": review["review_id"]}, expected=202)
                            report["starts"].append(review["review_id"])
                        elif review["prompt"] is not None:
                            key = (review["review_id"], review["prompt"].get("approval_id"))
                            if key not in answered:
                                body = review["prompt"]["body"]
                                if body.get("kind") == "service_software_install":
                                    second = bool(contexts)
                                    require(len(report["decisions"]) == (2 if second else 0), "unexpected software approval")
                                    answer = console.software_review(review, results["warm-y-check" if second else "checked-install"],
                                        Y_OPERATION if second else v.ACQUISITION_OPERATION,
                                        selection=Y_SELECTION if second else None)
                                    kind = "software_y" if second else "software_x"
                                else:
                                    require(len(contexts) < 2 and len(report["decisions"]) == (3 if contexts else 1),
                                            "unexpected extra access review")
                                    access = access_for(data, "before_access_answer")
                                    if access is None:
                                        await asyncio.sleep(.25)
                                        continue
                                    if not contexts:
                                        require(access["rows"] == [], "first access review already has retained grants")
                                    report["access_capability_checks"].append({"stage": "before_access_answer",
                                        "review_id": review["review_id"], "access": copy.deepcopy(access)})
                                    answer, context, expectation = self.access_review(review, accepted, results, contexts, policies)
                                    with provider.lock:
                                        provider.bind_context_expectation(expectation)
                                    contexts.append(context)
                                    kind = "complete_set_c" if len(contexts) == 2 else "service_b_with_setup"
                                await http("POST", console.PANEL + "/answer", answer, expected=202)
                                report["decisions"].append({"kind": kind, "prompt": review, "answer": answer,
                                                           "observation": "forwarded; native Console continuation and completion checked separately"})
                                answered.add(key)
                    if len(contexts) == 2 and first_business is not None and c_answer is not None:
                        if access_for(data, "completion_readback") is None:
                            await asyncio.sleep(.25)
                            continue
                        completion = self.complete(frames, accepted, contexts,
                                                   first_business, c_answer, results, data, report)
                        if completion is not None:
                            report.update(status="passed", completion=completion,
                                reviewed_sets=self.reviewed_sets, helper=self.reviewed_helper)
                            break
                    await asyncio.sleep(.1)
                require(report["status"] == "passed", "warm installed consumer did not complete")
                # Normal one-use panel offers revoke only these five displayed
                # service/catalog leaves. Setup children and the separately
                # reviewed whole policy are not swept by this action.
                retained_rows = {row["id"]: copy.deepcopy(row) for row in data["application_access"]["rows"]
                    if row["id"] in report["completion"]["issued_access_records"]}
                require(len(retained_rows) == 5, "exact B/C closeout rows missing")
                for row_id in report["completion"]["issued_access_records"]:
                    current = None
                    current = await current_access("before_revoke")
                    rows = [row for row in current["rows"] if row["id"] == row_id]
                    require(rows == [retained_rows[row_id]] and rows[0]["state"] == "grant_issued"
                            and rows[0]["can_revoke"] is True,
                            "closeout grant changed or current revoke offer is unavailable")
                    intent = {"offer_id": current["offer_id"], "row_id": row_id}
                    closeout = {"row_id": row_id, "reviewed": rows[0], "intent": intent,
                                "state": "entered_outcome_unobserved"}
                    report["grant_closeout"].append(closeout)
                    value = await http("POST", console.PANEL + "/access/revoke", intent)
                    require(value.get("state") == "revoked", "exact panel revocation did not settle")
                    after = await current_access("after_revoke")
                    readback = [row for row in after["rows"] if row["id"] == row_id]
                    require(len(readback) == 1 and readback[0]["state"] == "grant_unavailable"
                            and readback[0]["can_revoke"] is False, "exact grant revocation readback missing")
                    closeout.update(state="revoked", result=value, readback=readback[0])
                report["grant_closeout_scope"] = ("exact five displayed B/C service and catalog leaves revoked; "
                    "no claim that source-owned setup children, shared parents or whole policy were revoked")
            except BaseException as error:
                original_error = error
                report.update(status="failed", error=repr(error))
                raise
            finally:
                # Release the observation-only gate on failure so the retained
                # provider handler can unwind. No effect is permitted by this:
                # the provider also requires the recorded positive gate facts.
                if original_error is not None:
                    provider.deadline = 0
                    provider.first_pair_settled.set()
                if enrollment is not None:
                    try:
                        retired = await asyncio.to_thread(enrollment.retire, timeout=10.0)
                        report["login_retirement"] = dataclasses.asdict(retired)
                        require(retired.fenced and retired.reservations_drained and not retired.uncertain,
                                "warm Console login retirement is unresolved")
                    except BaseException as error:
                        report.update(status="failed", retirement_error=repr(error))
                        if original_error is None:
                            v.save_json(directory / "console-journey.json", report)
                            raise
                self.final_report = report
                v.save_json(directory / "console-journey.json", report)
            events.add("installed_warm_consumer_completed", **report["completion"])
            return report

        def check_native_instructions(self, starts, contexts, report):
            # Service continuation inputs carry the reviewed context blocks in
            # front of the unchanged instruction. Check the entire native input
            # against expectations captured before each access answer, then join
            # its bytes to the separately validated provider-visible blocks.
            display = report["service_context_display"]
            expectations, observed = display["expectations"], display["validated_observed_blocks"]
            require(len(starts) == 3 and len(contexts) == len(expectations) == len(observed) == 2,
                    "native warm input expectation count differs")
            require(console.input_text(starts[0]) == self.original_request(),
                    "native A instruction lineage differs")
            for index, (start, context, expected, prior_blocks, instruction) in enumerate(zip(
                    starts[1:], contexts, expectations, observed, (self.original_request(), WARM_REQUEST))):
                members = expected["members"]
                require(expected["original"] == context["fresh_original"]
                        and expected["instruction"] == instruction
                        and len(members) == len(prior_blocks) == index + 1,
                        "native warm input differs from its reviewed expectation")
                value = start["payload"].get("input")
                require(isinstance(value, dict) and set(value) == {"kind", "content"}
                        and value["kind"] == "content" and isinstance(value["content"], list),
                        "native service input projection changed")
                blocks = value["content"]
                require(len(blocks) == len(members) + 1
                        and all(isinstance(block, dict) and set(block) == {"type", "text"}
                                and block["type"] == "text" and isinstance(block["text"], str) for block in blocks)
                        and blocks[-1]["text"] == instruction,
                        "native service context/instruction block sequence differs")
                for block, member, prior in zip(blocks[:-1], members, prior_blocks):
                    checked_context_text(block["text"], expected, member)
                    require(prior == {"role": "user", "content": block["text"]},
                            "native input and validated provider context bytes differ")

        def complete(self, frames, accepted, contexts, first_business, c_answer, results, data, report):
            originals = [f for f in frames["original"] if f.get("source", {}).get("kind") == "console_event"]
            helpers = [f for f in frames["helper"] if f.get("source", {}).get("kind") == "console_event"]
            starts = [f for f in originals if f.get("kind") == "run_started" and f.get("session_id") == accepted["session_id"]]
            helper_starts = [f for f in helpers if f.get("kind") == "run_started"]
            if len(starts) != 3 or len(helper_starts) != 2:
                return None
            require(len({console.run_id(f) for f in starts}) == 3
                    and len({console.run_id(f) for f in helper_starts}) == 2, "warm run identity was reused")
            anchor = [f for f in frames["original"] if f["id"] == accepted["input_frame_id"]]
            require(len(anchor) == 1 and anchor[0]["payload"]["content"] == self.original_request()
                    and anchor[0]["source"]["kind"] == "send" and anchor[0]["interaction_id"] == accepted["interaction_id"]
                    and anchor[0]["identity"] == accepted["identity"]
                    and anchor[0]["session_id"] == accepted["session_id"]
                    and anchor[0]["status"] in ("accepted", "delivered"),
                    "initial Send does not bind the actual warm timeline")
            require(sum(f.get("kind") == "user_input" and f.get("source", {}).get("kind") == "send"
                        for f in frames["original"]) == 1,
                    "warm continuation was manually sent")
            self.check_native_instructions(starts, contexts, report)
            all_positions = {frame["id"]: i for i, frame in enumerate(frames["original"])}
            require(len(all_positions) == len(frames["original"])
                    and all(all_positions[anchor[0]["id"]] < all_positions[frame["id"]] for frame in starts),
                    "native warm run preceded the accepted Send anchor")
            for frame in starts + helper_starts:
                require(frame["payload"].get("source_event_type") == "run_started"
                        and frame["payload"].get("session_id") == frame["session_id"],
                        "native run start contradicts its session or event type")
            a_run, b_run, c_run = [console.run_id(f) for f in starts]
            for frame in originals:
                if frame.get("run_id") in (a_run, b_run, c_run):
                    console.run_id(frame)
                    require(frame.get("interaction_id") == frame["payload"]["identity"].get("interaction_id")
                            and frame.get("interaction_id") in (None, accepted["interaction_id"]),
                            "native run contradicts the accepted Console interaction")
                    require(frame.get("kind") not in ("interaction_failed", "run_failed"),
                            "the actual warm Original interaction failed")
            done = [self.completion_frame(originals, "original", console.run_id(f), accepted["session_id"], text)
                    for f, text in zip(starts, [v.ORIGINAL_A_FINISH, B_FINISH, c_answer["answer"]])]
            helper_done = [self.completion_frame(helpers, "helper", console.run_id(f), self.reviewed_helper["session_id"])
                           for f in helper_starts]
            if not all(done + helper_done):
                return None
            require(len({frame["payload"]["assistant_message_id"] for frame in done}) == 3,
                    "native A/B/C completion reused an assistant message identity")
            require(all(f["session_id"] == self.reviewed_helper["session_id"] for f in helper_starts),
                    "warm helper did not reuse its actual session")
            pairs = {}
            for key, run, name, args, content in (
                ("original-request", a_run, "find_and_install", {"request": self.original_request(), "user_context": [], "caller_observations": []}, None),
                (v.ORIGINAL_STORY_CALL, b_run, v.ACQUISITION_OPERATION, {"num_stories": 1}, first_business["actual_tool_result"]),
                (B_FIND, b_run, "find_and_install", {"request": WARM_REQUEST, "user_context": [], "caller_observations": []}, None),
                (C_STORY, c_run, v.ACQUISITION_OPERATION, {"num_stories": 1}, c_answer["x_result"]),
                (C_WIKI, c_run, Y_OPERATION, {"query": Y_QUERY}, c_answer["y_result"])):
                pair = self.tool_pair(originals, run, key, name, args, content)
                if pair is None:
                    return None
                pairs[key] = pair
            for key, context in zip(("original-request", B_FIND), contexts):
                frame = next(f for f in originals if f["id"] == pairs[key]["result"])
                blocks = frame["payload"]["content"]
                require(isinstance(blocks, list) and blocks
                        and all(block.get("type") == "text" and isinstance(block.get("text"), str) for block in blocks),
                        "native acquisition result lacks its exact text projection")
                observed = v.unique_json("\n".join(block["text"] for block in blocks).encode())
                require(observed == results[key] and observed["operation"] == context["business_task"]["id"],
                        "reviewed acquisition is not the actual native operation result")
            position = {f["id"]: i for i, f in enumerate(originals)}
            require(position[done[0]["id"]] < position[starts[1]["id"]]
                    < position[pairs[v.ORIGINAL_STORY_CALL]["result"]] < position[pairs[B_FIND]["call"]]
                    < position[done[1]["id"]] < position[starts[2]["id"]]
                    < position[pairs[C_STORY]["result"]] < position[pairs[C_WIKI]["result"]] < position[done[2]["id"]],
                    "native warm acquisition/use/continuation order differs")
            installs = [f for f in helpers if f.get("kind") == "tool_call_requested" and f["payload"].get("name") == "setup_install"]
            require(len(installs) == 2 and [f["payload"]["id"] for f in installs] == ["install-once", "warm-y-install"]
                    and len(report["decisions"]) == 4 and len(report["starts"]) == 2
                    and results[Y_ACCESS] == results["public-task-access"] == provider_reply(), "actual install/review counts differ")
            rows = self.access_snapshot(data)["rows"]
            actual_grants = []
            for reviewed in self.reviewed_sets:
                selected = [row for row in rows if row["lifetime"]["scope"] == {"kind": "task", "task": reviewed["fresh_original"]}]
                require(len(selected) == len(reviewed["installations"]) + 1
                        and all(row["state"] == "grant_issued" for row in selected), "fresh issued access records missing")
                for item in reviewed["installations"].values():
                    require(sum(row["resource"] == item["server"] and row["lifetime"] == item["lifetime"] for row in selected) == 1,
                            "issued access record differs from reviewed native scope")
                actual_grants.extend(row["id"] for row in selected)
            require(len(set(actual_grants)) == 5, "B/C access-record identities were reused")
            return {"session_id": accepted["session_id"], "helper_session_id": self.reviewed_helper["session_id"],
                "a_run": a_run, "b_run": b_run, "c_run": c_run, "tool_frames": pairs,
                "native_start_frames": [frame["id"] for frame in starts],
                "native_completion_frames": [frame["id"] for frame in done],
                "assistant_message_ids": [frame["payload"]["assistant_message_id"] for frame in done],
                "private_input_receipts": "not requested or observed; native Console run events are the completion witness",
                "helper_runs": [console.run_id(f) for f in helper_starts], "issued_access_records": actual_grants,
                "x_job": results["installed-job"]["job"]["id"], "y_job": results["warm-y-install"]["job"]["id"],
                "x_install_calls": 1, "y_install_calls": 1, "answer": c_answer["answer"],
                "native_source_lineages": "historical source/leaf rows checked after joined shutdown; no native input-receipt correlation"}

        def check_final_artifacts(self, directory, report):
            if report.get("status") != "passed":
                return
            require(report.get("shutdown_response", {}).get("result", {}).get("host_cleanup_completed") is True,
                    "source evidence requires the existing joined host shutdown")
            journey = self.final_report
            require(journey["status"] == "passed", "warm completion failed before source readback")
            native = self.source_owner["native_host"]
            realm, owner = native["realm"], self.source_owner["deployment"]
            root = directory / "deployment/shared/data/toolkit-native" / hashlib.sha256(realm.encode()).hexdigest()
            database = (root / "originals.sqlite3").resolve(strict=True)
            route = native["routes"][0]
            contexts = [decision["prompt"]["prompt"]["body"]["access_request"]["service_instruction"]
                        for decision in journey["decisions"] if decision["kind"] in ("service_b_with_setup", "complete_set_c")]
            originals = [contexts[0]["prior_original"]["work"]] + [context["fresh_original"] for context in contexts]
            observations, leaf_sets = [], []
            # Exactly three source-owned keys, after cleanup, without creating
            # a native owner, refreshing authority or scanning unrelated rows.
            with sqlite3.connect(database.as_uri() + "?mode=ro", uri=True) as connection:
                for index, original in enumerate(originals):
                    key = hashlib.sha256(v.canonical(["toolkit.authenticated-source-event", realm,
                        native["ingress_domain"], {"authority": route["source_authority"], "work": original}])).hexdigest()
                    rows = connection.execute("SELECT revision,digest,data FROM toolkit_records WHERE realm=? AND owner=? AND id=?",
                                              (realm, owner, key)).fetchall()
                    require(len(rows) == 1, "exact canonical warm source missing")
                    revision, digest, raw = rows[0]
                    record = v.unique_json(raw.encode())
                    require(record["schema"] == "toolkit.authenticated-source-event" and record["version"] == 3,
                            "canonical source record version changed")
                    body = record["body"]
                    association = body["final_association"]
                    require(association["original_work"] == {"authority": route["source_authority"], "work": original}
                            and association["target"]["context"] == journey["accepted"]["session_id"],
                            "canonical source association differs from actual input owner")
                    basis = association["authority_basis"]
                    require(basis["kind"] == "targeted_grant_lineages" and basis["targets"], "source lacks exact target lineages")
                    leaves = [row["lineage"][-1] for row in basis["targets"]]
                    if basis["base"]:
                        leaves.append(basis["base"][-1])
                    identities = {v.canonical(ref) for ref in leaves}
                    require(len(identities) == len(leaves) and all(nonempty(ref["grant_id"]) for ref in leaves),
                            "source target leaf identities collide")
                    if index:
                        admission = body.get("service_admission")
                        require(isinstance(admission, dict) and set(admission) == {"session_id", "input_id"}
                                and admission["session_id"] == journey["accepted"]["session_id"]
                                and nonempty(admission["input_id"]),
                                "historical source admission does not name the observed Original session")
                        require(all(identities.isdisjoint(prior) for prior in leaf_sets),
                                "later task copied a prior task's native leaf authority")
                        expected_servers = {item["server"] for item in self.reviewed_sets[index-1]["installations"].values()}
                        actual_servers = {row["target"]["server"] for row in basis["targets"] if row["target"]["kind"] == "mcp_server"}
                        require(expected_servers.issubset(actual_servers), "fresh source omits a reviewed business target")
                    leaf_sets.append(identities)
                    observations.append({"original": original, "key": {"realm": realm, "owner": owner, "id": key},
                        "revision": revision, "stored_digest": digest, "row_sha256": hashlib.sha256(raw.encode()).hexdigest(),
                        "association": association, "service_admission": body.get("service_admission"),
                        "leaf_references": leaves})
            require(contexts[1]["prior_original"] == observations[1]["association"]["original_work"],
                    "C acquisition did not originate in the admitted B source")
            pin = v.new_artifact(directory / "warm-source-lineages.json", v.canonical({
                "scope": "historical exact source/leaf readback and saved source admission labels; no independently observed native input receipt or input-to-run correlation; no new admission authority",
                "database": str(database), "rows": observations}))
            report["warm_source_lineages"] = pin
            self.check_controller_audits(directory, report, observations)

        def check_controller_audits(self, directory, report, sources):
            journey = self.final_report
            display = journey["controller_recovery_display"]
            require(set(display) == {"b", "c"}, "controller recovery run selection changed")
            expected_config = self.controller_configuration
            require(v.exact_bytes(directory / "meerkat.toml") == expected_config
                    and report["model_configuration"] == {"path": str(directory / "meerkat.toml"),
                        "sha256": hashlib.sha256(expected_config).hexdigest()},
                    "actual controller configuration differs from its pre-runtime selection")
            target = configured_controller_target(expected_config, self.source_owner["native_host"]["realm"])
            session = journey["accepted"]["session_id"]
            selected = [(phase, sources[index]["service_admission"]["input_id"],
                         journey["completion"][phase + "_run"])
                        for index, phase in ((1, "b"), (2, "c")) if display[phase] is not None]
            states = []
            database_observation = None
            # This supplements the existing cold source read only for runs
            # which displayed recovery. No runtime is recreated or queried.
            if selected:
                database = (directory / "state/runtime.sqlite").resolve(strict=True)

                def identities():
                    result = []
                    for path in (database, Path(str(database) + "-wal"), Path(str(database) + "-shm")):
                        require(not path.is_symlink(), "controller audit database sidecar is a symlink")
                        result.append({"present": path.exists(), **(v.artifact_pin(path) if path.exists()
                                      else {"path": str(path)})})
                    return result

                before = identities()
                connection = sqlite3.connect(database.as_uri() + "?mode=ro", uri=True, timeout=3)
                try:
                    connection.execute("PRAGMA query_only=ON")
                    deadline = time.monotonic() + 30
                    connection.set_progress_handler(lambda: int(time.monotonic() >= deadline), 10000)
                    for phase, input_id, run_id in selected:
                        rows = connection.execute("SELECT state_json FROM runtime_input_states "
                            "WHERE runtime_id=? AND input_id=? LIMIT 2", ("rt:session:" + session, input_id)).fetchall()
                        require(len(rows) == 1, "exact recovered input audit is unavailable")
                        raw = rows[0][0].encode() if isinstance(rows[0][0], str) else rows[0][0]
                        require(len(raw) <= 8 * 1024 * 1024, "controller audit row exceeds bound")
                        states.append({"phase": phase, "input_id": input_id, "run_id": run_id,
                            "row_sha256": hashlib.sha256(raw).hexdigest(), "state": v.unique_json(raw)})
                finally:
                    connection.close()
                after = identities()
                database_observation = {"before": before, "after": after,
                    "database_file_unchanged": before[0] == after[0],
                    "sidecar_identity_changed": before[1:] != after[1:]}
            raw_pin = v.new_artifact(directory / "warm-controller-audit-rows.json", v.canonical({
                "database_observation": database_observation, "states": states}))
            report["warm_controller_audit_rows"] = raw_pin
            require(database_observation is None or database_observation["database_file_unchanged"],
                    "controller audit database changed during stopped read")
            recovered = []
            for row in states:
                checked = checked_controller_audit(v, row["state"], session, row["input_id"], row["run_id"], target)
                recovered.append({"phase": row["phase"], "display": display[row["phase"]],
                                  "row_sha256": row["row_sha256"], "audit": checked})
            report["warm_controller_recovery"] = v.new_artifact(directory / "warm-controller-recovery.json", v.canonical({
                "scope": "historical native audit after joined shutdown; no live receipt API, new authority or restart claim",
                "model_configuration": report["model_configuration"], "recoveries": recovered,
                "no_recovery_runs": [phase for phase in ("b", "c") if display[phase] is None]}))

    def provider_reply():
        return {"state": "reviewed", "access_reviewed": True, "grant_issued": True,
                "fresh_instruction_retained": True, "ready": False}

    return WarmScenario(v, origin)
