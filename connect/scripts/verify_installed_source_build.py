"""One installed source-built MCP journey using existing Console owners.

Controlled local package, model schedule and fixture issuer. The helper performs
actual schema/package inspection, checked installation and readback. Native
Original events, not model prose, establish the compiled MCP call and completion.
No autonomous discovery, public registry, account, reset or D5 qualification.
"""
from __future__ import annotations

import asyncio
import copy
import dataclasses
import hashlib
from http.client import HTTPConnection
import importlib.util
import json
from pathlib import Path
import time
import uuid
from urllib.parse import urlencode

OPERATION = "source_add"
CALL = "original-source-add"
OPERANDS = {"left": 19, "right": 23}
VALUE = {"sum": 42}
ANSWER = "The source_add tool returned sum 42 for left 19 and right 23."
MCP_TOOLS = ("setup_state", "setup_inspect_schema", "setup_inspect_package",
             "setup_selected_check", "setup_selected_install", "setup_job", "setup_reconcile")


def create_scenario(v, console, warm, origin):
    require, nonempty = console.require, console.nonempty
    # Reuse only the reviewed observation and login/runtime setup methods.
    # The warm provider, second acquisition and cold recovery audit are not used.
    Observations = type(warm.create_scenario(v, console, origin))
    fixture_root = Path(__file__).parent / "fixtures/source-built-mcp"
    fixture_root = fixture_root.resolve(strict=True)
    fixture_path = fixture_root / "setup_fixture.py"
    spec = importlib.util.spec_from_file_location("source_build_supplied_fixture", fixture_path)
    fixture = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(fixture)
    fixture_files = {name: v.artifact_pin(fixture_root / name) for name in (
        "setup_fixture.py", "SCENARIO.json", "FIXTURE.json", *fixture.PACKAGE_FILES)}
    package_files = fixture.package_files(fixture_root)
    scenario = v.unique_json((fixture_root / "SCENARIO.json").read_bytes())
    request_text = scenario["request"]
    require(nonempty(request_text) and scenario["raw_tool"] == OPERATION and scenario["arguments"] == OPERANDS
            and scenario["expected_tool_value"] == VALUE and scenario["maximum_business_calls"] == 1,
            "selected scenario differs from the consumer contract")
    schema_raw = fixture.wire(v.unique_json(package_files["operation.schema.json"]))

    def policy_selection():
        return {"tools": [{"name": OPERATION, "consequence": "r2"}], "catalog": "r2"}

    def policy_successor(proposal, prior_raw, session):
        prior = v.unique_json(prior_raw)
        selected = policy_selection()
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
                and text == v.canonical(value).decode(), "context differs from pre-answer review")

    def schema_evidence(value):
        expected = {"schema_version": 1, "scope": "supplied_schema_evidence_only",
            "schema_digest": hashlib.sha256(schema_raw).hexdigest(), "bytes": len(schema_raw),
            "hash_algorithm": "sha256", "byte_convention": "exact_supplied_utf8",
            "runtime_observation": "not_established", "schema_validation": "not_performed"}
        require(isinstance(value, dict) and set(value) == set(expected) | {"notice"}
                and all(value[key] == item for key, item in expected.items()) and nonempty(value["notice"]),
                "helper schema evidence does not bind exact supplied canonical bytes")

    class SourceProvider(v.InstalledAcquisitionProvider):
        original_catalog = True
        operation = OPERATION

        def __init__(self, directory, events):
            self.request_text = request_text
            self.checked_request = None
            self.context_expectations, self.context_observations = [], []
            self.business = None
            v.save_json(directory / "supplied-source-fixture.json", {"scope": "separately supplied synthetic user package",
                "files": fixture_files, "request": request_text})
            super().__init__(directory, events)

        def public_reply(self):
            return {"state": "reviewed", "access_reviewed": True, "grant_issued": True,
                    "fresh_instruction_retained": True, "ready": False}

        def bind_context_expectation(self, expectation):
            require(not self.context_expectations and len(expectation["members"]) == 1,
                    "unexpected second source-build access expectation")
            self.context_expectations.append(copy.deepcopy(expectation))

        def tool(self, body, identifier, name, args):
            names = {row["function"]["name"] for row in body["tools"]}
            require(name in names and identifier not in self.issued,
                    "unavailable tool or repeated effect: " + identifier)
            self.issued.add(identifier)
            self.events.add("controlled_model_tool", model=body["model"], call_id=identifier, tool=name, args=args)
            return {"delta": {"tool_calls": [{"index": 0, "id": identifier, "type": "function",
                    "function": {"name": name, "arguments": json.dumps(args)}}]}, "finish_reason": "tool_calls"}

        @staticmethod
        def done(text):
            return {"delta": {"content": text}, "finish_reason": "stop"}

        def response(self, body):
            lane = body["model"]
            require(lane in ("fixture-original-model", "fixture-helper-model"), "unknown controlled model owner")
            results = {}
            for message in body["messages"]:
                if message["role"] != "tool":
                    continue
                identifier, content = message["tool_call_id"], message["content"]
                require(identifier in self.issued and identifier not in results and isinstance(content, str),
                        "unissued, duplicate or non-text model tool result")
                value = v.unique_json(content.encode())
                require(identifier not in self.results or self.results[identifier] == value,
                        "retained actual tool result changed")
                results[identifier] = self.results[identifier] = value
            if lane == "fixture-original-model":
                return self.original(body, results)
            require(request_text in v.canonical(body["messages"]).decode(), "helper lacks actual original request")
            return self.helper(body, results)

        def original(self, body, results):
            messages = body["messages"]
            inputs = [i for i, row in enumerate(messages) if row["role"] == "user" and row.get("content") == request_text]
            require(len(inputs) in (1, 2), "expected A and at most one fresh B instruction")
            if "original-request" not in results:
                require(len(inputs) == 1, "B cannot begin another acquisition")
                return self.tool(body, "original-request", "find_and_install", {
                    "request": request_text, "user_context": [], "caller_observations": []})
            found = results["original-request"]
            require(nonempty(found.get("operation")) and found.get("created") is True
                    and found.get("worker") == {"state": "input_admitted", "deduplicated": False,
                        "source_settlement": "fresh_acceptance_promoted"}, "actual helper admission missing")
            if len(inputs) == 1:
                require(CALL not in self.issued, "A cannot call the service")
                return self.done(v.ORIGINAL_A_FINISH)
            first, fresh = inputs
            require(any(row["role"] == "assistant" and row.get("content") == v.ORIGINAL_A_FINISH
                        for row in messages[first+1:fresh]) and len(self.context_expectations) == 1 and fresh > 0,
                    "fresh B lacks A completion or its checked context")
            expected = self.context_expectations[0]
            block = messages[fresh-1]
            require(set(block) == {"role", "content"} and block["role"] == "user", "native context message shape changed")
            checked_context_text(block["content"], expected, expected["members"][0])
            if not self.context_observations:
                self.context_observations.append([copy.deepcopy(block)])
            require(self.context_observations == [[block]], "approved context history changed")
            v.original_catalog_history(messages, request_text, OPERATION, fresh,
                service_context=v.unique_json(block["content"].encode()))
            if "original-catalog-search" not in results:
                return self.tool(body, "original-catalog-search", "tool_catalog_search", {"query": OPERATION, "limit": 50})
            found = results["original-catalog-search"]
            matches = [row for row in found["results"] if row["name"] == OPERATION]
            require(found["catalog_exact"] is True and not found.get("pending_sources", [])
                    and len(matches) == 1 and matches[0]["visibility_status"] in ("loaded", "deferred"),
                    "fresh Original catalog lacks the compiled operation")
            if "original-catalog-load" not in results:
                return self.tool(body, "original-catalog-load", "tool_catalog_load", {"names": [OPERATION]})
            v.exact_catalog_load(results["original-catalog-load"], [OPERATION])
            if CALL not in results:
                return self.tool(body, CALL, OPERATION, OPERANDS)
            require(results[CALL] == VALUE and self.business is None, "compiled result differs or answer repeated")
            actual = next(row["content"] for row in messages if row.get("tool_call_id") == CALL)
            self.business = {"answer": ANSWER, "value": results[CALL], "actual_tool_result": actual,
                "request_index": len(self.requests)-1, "request_sha256": hashlib.sha256(v.canonical(body)).hexdigest()}
            self.business_response_prepared.set()
            self.events.add("original_compiled_answer_prepared", **self.business)
            return self.done(ANSWER)

        def helper(self, body, results):
            names = {row["function"]["name"] for row in body["tools"]}
            # Search each exact desired name. Re-observation is bounded and only
            # used while native source readiness is explicitly pending.
            if any(name not in names for name in MCP_TOOLS):
                for name in MCP_TOOLS:
                    previous = [(key, value) for key, value in results.items() if key.startswith("helper-search-" + name + "-")]
                    found = previous[-1][1] if previous else None
                    if found is not None:
                        require(found["catalog_exact"] is True, "helper catalog inventory is not exact")
                        matches = [row for row in found["results"] if row["name"] == name]
                        if len(matches) == 1 and matches[0]["visibility_status"] in ("loaded", "deferred") and not found.get("pending_sources", []):
                            continue
                        require(found.get("pending_sources") and len(previous) < 3,
                                "helper catalog did not expose exact selected setup tool: " + name)
                    return self.tool(body, "helper-search-" + name + "-" + str(len(previous)),
                                     "tool_catalog_search", {"query": name, "limit": 10})
                require("helper-load" not in results, "loaded setup tools did not become visible")
                return self.tool(body, "helper-load", "tool_catalog_load", {"names": list(MCP_TOOLS)})
            require("helper-load" in results, "helper must observe its own exact Search/Load receipts")
            v.exact_catalog_load(results["helper-load"], list(MCP_TOOLS))
            if "host-state" not in results:
                return self.tool(body, "host-state", "setup_state", {})
            state = results["host-state"]
            require("node" in state["selected_installation"]["profiles"] and isinstance(state.get("node_build"), dict)
                    and state["node_build"].get("available") is True,
                    "matching product did not expose selected Node build support")
            if "schema-evidence" not in results:
                return self.tool(body, "schema-evidence", "setup_inspect_schema", {"schema_json": schema_raw.decode()})
            schema_evidence(results["schema-evidence"])
            if "package-inspection" not in results:
                return self.tool(body, "package-inspection", "setup_inspect_package", {
                    "package_json": fixture.wire(fixture.package_document()).decode()})
            if self.checked_request is None:
                self.checked_request = fixture.stage_and_check_request(fixture_root,
                    state["selected_installation"]["staging_root"], state["node_build"]["planning"], results["package-inspection"])
                v.save_json(self.directory / "supplied-selected-request.json", self.checked_request)
            if "checked-install" not in results:
                return self.tool(body, "checked-install", "setup_selected_check", self.checked_request)
            checked = results["checked-install"]
            plan = v.unique_json(self.checked_request["build_plan_json"].encode())
            require(checked["scope"] == "selected_software_inputs_only" and checked["profile"] == "node"
                    and checked["component"] == fixture.COMPONENT and checked["mcp_name"] == fixture.COMPONENT
                    and checked["build"]["plan_sha256"] == hashlib.sha256(fixture.wire(plan)).hexdigest()
                    and checked["build"]["profile_sha256"] == fixture.PROFILE_SHA256
                    and checked["build"]["target"] == plan["target"] and checked["build"]["outputs"] == plan["outputs"]
                    and checked["declared_service"] == {"authentication": {"kind": "none"}, "operation_tools": {"add": OPERATION}},
                    "actual checked build or service declaration differs")
            if "install-once" not in results:
                return self.tool(body, "install-once", "setup_selected_install", {
                    key: checked[key] for key in ("validation_id", "effect_sha256")})
            job = results["install-once"]["job"]["id"]
            current = [value["job"] for value in results.values() if isinstance(value, dict)
                       and isinstance(value.get("job"), dict) and value["job"].get("id") == job][-1]
            require(current["status"] in ("running", "succeeded"), "actual selected build failed: " + str(current))
            if current["status"] == "running":
                return self.tool(body, "install-poll-" + str(len(self.requests)), "setup_job", {"job_id": job, "wait_ms": 10000})
            if "installed-job" not in results:
                return self.tool(body, "installed-job", "setup_job", {"job_id": job})
            if "reconciled-install" not in results:
                return self.tool(body, "reconciled-install", "setup_reconcile", {"job_id": job})
            observed = results["reconciled-install"]["observation"]
            installed = observed["result"]
            require(observed["job_id"] == job and observed["status"] == "matched"
                    and installed["selection_kind"] == "node" and installed["component"] == fixture.COMPONENT
                    and installed["effect_sha256"] == checked["effect_sha256"]
                    and installed["declared_service"] == checked["declared_service"]
                    and installed["build_receipt_binding"] == "original_installer_reply"
                    and installed["installer_reply"]["receipt_sha256"] == installed["receipt_sha256"],
                    "actual build readback lost exact selection or live installer origin")
            if "public-task-access" not in results:
                return self.tool(body, "public-task-access", "toolkit_request_public_service", {
                    "job_id": job, "operations": [OPERATION], "agent": "original", "expires_in_ms": None,
                    "tool_policy": policy_selection()})
            self.check_access_result(results["public-task-access"])
            self.finished.set()
            return self.done(self.helper_finish())

    class SourceScenario(console.ConsoleScenario):
        name = "installed-console-source-build-success"
        scope = ("matching installed product with a separately supplied synthetic source-built stdio MCP package; "
                 "controlled issuer/model schedule; real helper Check/Install/Reconcile and Original Search/Load/compiled call; "
                 "no autonomous discovery, registry acquisition, account, reset, D5 or general compiler qualification")
        access_snapshot = Observations.access_snapshot
        access_presentation = Observations.access_presentation
        observe_completed_setup = Observations.observe_completed_setup
        completion_frame = Observations.completion_frame

        def original_request(self):
            return request_text

        def configure_owner(self, owner, source):
            super().configure_owner(owner, source)
            helper = next(row for row in owner["native_host"]["members"] if row["member"]["member"] == "helper")
            helper["audience"] = {"audience": "principal", "principal": self.human()}
            self.source_owner = copy.deepcopy(owner)
            source["purpose"] = self.scope
            source["source_build_fixture"] = {"inputs": fixture_files, "operation": OPERATION,
                "arguments": OPERANDS, "maximum_business_calls": 1, "controlled_package_and_model": True}

        def configure_plan(self, plan):
            super().configure_plan(plan)
            plan["acquisition"]["console_setup"]["access_management"] = "requester_issued_leaves"
            members = plan["application"]["tool_policy"]["members"]
            helper = next(row for row in members if row["member_identity"] == "helper")
            helper["grants"] = [{"tool_name": name, "action": "invoke", "consequence": "r2"}
                for name in sorted(set(MCP_TOOLS) | {"setup_report", "toolkit_request_public_service",
                                                     "tool_catalog_search", "tool_catalog_load"})]
            original = next(row for row in members if row["member_identity"] == "original")
            original["grants"] = [{"tool_name": name, "action": "invoke", "consequence": "r2"}
                                  for name in ("find_and_install", OPERATION)]
            self.realm_state_root = plan["application"]["realm"]["state_root"]
            self.source_application = copy.deepcopy(plan["application"])

        def configure_runtime(self, config, directory):
            super().configure_runtime(config, directory)
            with open(config.access_config_path, "a") as output:
                output.write('\n[[rules]]\nid = "controlled-console-helper-observation"\nsubjects = ["'
                    + console.EMAIL + '"]\nactions = ["agent.view"]\nagents = ["helper"]\n')

        def provider(self, directory, events):
            self.directory = directory
            return SourceProvider(directory, events)

        def software_answer(self, review, checked, selected_request):
            answer = console.approval_answer(review)
            body = review["prompt"]["body"]
            require(review["can_start"] is False and body["kind"] == "service_software_install"
                    and body["check_tool"] == "setup_selected_check" and body["scope"] == "software_installation_only"
                    and body["owner_proposal"] == checked and body["selection"] == selected_request,
                    "software approval differs from actual helper CHECK and visible build proposal")
            return answer

        def access_review(self, review, accepted, results, contexts, policies):
            require(not contexts and not policies, "only one fresh service access review is allowed")
            body = review["prompt"]["body"]
            require(body["kind"] == "public_services_with_setup_v2" and len(body["members"]) == 1,
                    "expected one setup-enabled source-built service")
            request, setup = body["access_request"], body["setup"]
            context, helper = request["service_instruction"], setup["helper"]
            fresh = context["fresh_original"]
            native = self.source_owner["native_host"]
            require(nonempty(fresh) and context["proposed_fresh_instruction"] == request_text
                    and context["business_task"] == {**self.task_scope, "id": results["original-request"]["operation"]}
                    and console.original_work_reference(context["prior_original"], self.source_authority)
                    and context["operator_and_requester"] == self.human() and context["represented_subject"] == self.human()
                    and context["ingress_actor"] == native["routes"][0]["source_authority"]
                    and context["audience"] == {"audience": "principal", "principal": self.human()}
                    and context["configuration_persistence"] == "not authorized by this review",
                    "access review changed instruction, task or requester")
            configured = request["configured_setup"]
            require(configured["selection"] == setup and nonempty(helper["session_id"])
                    and helper["member"] == self.expected_helper_member
                    and helper["logical_executor"] == v.installed_fixture_principal("helper")
                    and helper["audience"] == {"audience": "principal", "principal": self.human()}
                    and setup["configured"]["session_id"] == accepted["session_id"]
                    and setup["configured"]["capability"]["route"] == v.ACQUISITION_ROUTE
                    and configured["processors"] == [v.installed_fixture_principal("original-agent"), helper["logical_executor"]]
                    and configured["sessions"] == [accepted["session_id"], helper["session_id"]]
                    and configured["audience"] == helper["audience"] and type(configured["expires_at_ms"]) is int,
                    "access review omitted exact configured helper/session scope")
            self.reviewed_helper = copy.deepcopy(helper)
            owner = {"operator": native["remote_operator"], "application": native["grant_administrator"],
                "realm": native["realm"], "route": v.ACQUISITION_ROUTE, "purpose": "operations", "represented_subject": self.human()}
            member = body["members"][0]
            parent = member["parent"]
            require(all(parent.get(key) == value for key, value in owner.items()), "service parent owner differs")
            document, access = parent["access_request"], parent["service_access"]
            checked, observation = results["checked-install"], results["reconciled-install"]["observation"]
            installed, job = observation["result"], results["installed-job"]["job"]["id"]
            require(observation["status"] == "matched" and observation["job_id"] == job
                    and installed["selection_kind"] == "node", "exact installed Node readback missing")
            configuration = installed["mcp_configuration"]
            require(set(configuration) == {"mcpServers"} and set(configuration["mcpServers"]) == {fixture.COMPONENT},
                    "installed source service identity differs")
            server = fixture.COMPONENT
            launch = configuration["mcpServers"][server]
            expected_script = str(Path(checked["destination"]) / "node_modules" / fixture.PACKAGE / "server.cjs")
            require(set(launch) == {"command", "args"} and Path(launch["command"]).is_absolute()
                    and launch["args"] == [expected_script], "actual selected Node launch differs")
            expected_configuration = {"name": server, "command": launch["command"], "args": launch["args"],
                "env": {}, "access": {"allowed_operations": [OPERATION]}}
            software, basis = document["software"], document["operation_basis"]
            expected_basis = {"kind": "selected_software_declarations", "authentication": {"kind": "none"},
                "package_sha256": checked["files"]["package.json"], "effect_sha256": checked["effect_sha256"],
                "source": {"plugin_source": None, "acquired_source": None},
                "selection_sha256": checked["selection_sha256"], "document_kind": "local_software_declaration",
                "declaration_digest": checked["declaration_digest"]}
            require(set(basis) == set(expected_basis) | {"notice"}
                    and all(basis[key] == value for key, value in expected_basis.items()) and nonempty(basis["notice"]),
                    "native operation basis differs from exact checked declaration")
            permission, destination = access["permission"], access["live_attachment"]["destination"]
            require(member["server"] == server and software["job_id"] == job
                    and software["family"] == "selected-node" and software["profile"] == "node"
                    and software["server_name"] == server and software["validation_id"] == checked["validation_id"]
                    and software["receipt_sha256"] == installed["receipt_sha256"]
                    and software["configuration_sha256"] is None
                    and software["document_sha256"] == hashlib.sha256(v.canonical(configuration)).hexdigest()
                    and document["phase"] == "installed_public_service_access"
                    and document["realm"] == native["realm"] and document["realm_state_root"] == self.realm_state_root
                    and document["session"] == helper["session_id"] and document["service_instruction"] == context
                    and document["authentication"] == {"kind": "none"}
                    and document["application_selection"] == {"operations": [OPERATION], "agent": "original",
                        "expires_in_ms": None, "tool_policy": policy_selection()}
                    and document["installation"] == expected_configuration
                    and access["route"] == v.ACQUISITION_ROUTE and access["choice"] == "approve"
                    and permission["target"] == "original" and permission["connection"] == server
                    and permission["resource"] == server and permission["operations"] == [OPERATION]
                    and permission["lifetime"]["scope"] == {"kind": "task", "task": fresh}
                    and type(permission["lifetime"]["expires_at_ms"]) is int
                    and permission["lifetime"]["expires_at_ms"] > int(time.time()*1000)
                    and destination["context"] == accepted["session_id"]
                    and destination["logical_owner"] == v.installed_fixture_principal("original-agent")
                    and destination["audience"] == helper["audience"], "public review crossed actual service or native scope")
            require(all(body["catalog"].get(key) == value for key, value in owner.items()), "catalog parent differs")
            catalog = body["catalog"]["catalog_access"]
            require(catalog["original"] == fresh and catalog["session"] == accepted["session_id"]
                    and catalog["kind"] == {"public_service_catalog_with_setup_v2": {"helper": helper}}
                    and catalog["lifetime"]["scope"] == {"kind": "task", "task": fresh}
                    and nonempty(access["decision"]) and nonempty(catalog["decision"])
                    and access["decision"] != catalog["decision"], "catalog decision does not match reviewed task")
            wrapper = request["member_tool_policy"]
            require(set(wrapper) == {"proposal", "approval_scope", "lifetime", "catalog_visibility"}
                    and wrapper["approval_scope"] == "the complete successor policy including preserved member permissions"
                    and wrapper["lifetime"] == "until superseded; task closure does not undo this separately reviewed policy"
                    and wrapper["catalog_visibility"] == "eligible tool metadata in the original session; invocation still needs its exact native target grant",
                    "whole-policy approval scope changed")
            policies.append(policy_successor(wrapper["proposal"], self.application_tool_policy, accepted["session_id"]))
            self.reviewed_set = {"fresh_original": fresh, "service_lifetime": permission["lifetime"],
                "catalog_lifetime": catalog["lifetime"], "server": server,
                "decisions": [access["decision"], catalog["decision"]]}
            fields = {"schema": "toolkit.approved-service-context.v1", "operation": context["business_task"]["id"],
                "original": fresh, "connection": server, "resource": server, "allowed_operations": [OPERATION],
                "configuration_sha256": access["connection_revision"], "attachment": "observed_usable_before_input_admission",
                "meaning": "The host completed the approved integration attachment checks for this continuation. This context accompanies the unchanged reviewed user instruction. Native authorization is still checked on each use. This observation does not guarantee future readiness, extend permission, establish filesystem isolation or complete the user's task."}
            expectation = {"original": fresh, "instruction": request_text,
                "members": [{"fields": fields, "reviewed_expiry_max_ms": permission["lifetime"]["expires_at_ms"]}],
                "observed_before_answer_ms": int(time.time()*1000), "scope": "display consistency against pre-answer review; native currentness remains authoritative"}
            return console.approval_answer(review), context, expectation

        async def run(self, private, provider, directory, events, process, base_url):
            require(base_url.rstrip("/") == self.origin, "listener differs from selected Console origin")
            self.reviewed_set, self.reviewed_helper = None, None
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
            scope = "installed-console-source-build-" + uuid.uuid4().hex

            def request(method, path, body=None, *, bearer=True, expected=200, text=False, first_page=False):
                require(not first_page or (method == "GET" and path == console.PANEL + "/data"
                    and body is None and bearer is True and expected == 200 and text is False),
                    "First-page observation cannot select another request")
                require(time.monotonic() < deadline and process.poll() is None, "source-build Console owner/deadline ended")
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
                    require(len(raw) <= 4 * 1024 * 1024, "source-build HTTP response exceeds bound")
                    value = raw.decode() if text else v.unique_json(raw)
                    report["http"].append({"method": method, "path": path, "request": body,
                                           "status": response.status, "response": value})
                    if first_page and response.status == 503 and value == {"error": "Protected setup is unavailable"}:
                        raise warm.FirstPageUnavailable(len(report["http"]) - 1)
                    require(response.status == expected, "source-build HTTP failed: " + path)
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

                value = await warm.first_page_observation(read, validate, report["first_page_actions"], deadline)
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
                raise AssertionError("source-build access observation deadline ended")

            async def timeline(identity):
                page = await http("GET", "/console/timeline?" + urlencode({"identity": identity, "mode": "since",
                    "limit": 400, **({"after": cursors[identity]} if cursors[identity] else {})}))
                require(page.get("available", True) is True and isinstance(page.get("frames"), list),
                        "actual source-build timeline unavailable")
                report["timeline"].append({"identity": identity, "page": page})
                for raw in page["frames"]:
                    frame = console.frame_record(raw)
                    previous = indices[identity].get(frame["id"])
                    if previous is None:
                        require(len(frames[identity]) < 4000, "source-build native timeline exceeds bound")
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
                require(fresh_data["review"] is None, "fresh source-build fixture has review")
                fresh_access = self.access_snapshot(fresh_data)
                require(fresh_access["rows"] == [], "fresh source-build fixture has retained access")
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
                    # Observe native timelines without holding the provider mutex.
                    await timeline("original")
                    await timeline("helper")
                    original_starts = [f for f in frames["original"] if f.get("kind") == "run_started"
                        and f.get("session_id") == accepted["session_id"]]
                    helper_starts = [f for f in frames["helper"] if f.get("kind") == "run_started"]
                    require(len(original_starts) <= 2 and len(helper_starts) <= 1, "unexpected extra source-build run")
                    # Missing snapshot is not failure or progress. No lock is
                    # held by this observer across HTTP, native reads or sleep.
                    if not provider.lock.acquire(blocking=False):
                        await asyncio.sleep(.05)
                        continue
                    try:
                        require(not provider.errors, "controlled source-build provider refused actual input")
                        results = copy.deepcopy(provider.results)
                        business = copy.deepcopy(provider.business)
                        selected_request = copy.deepcopy(provider.checked_request)
                        report["service_context_display"] = {
                            "expectations": copy.deepcopy(provider.context_expectations),
                            "validated_observed_blocks": copy.deepcopy(provider.context_observations)}
                    finally:
                        provider.lock.release()
                    data, review = None, None
                    data = await first_page()
                    review = data["review"]
                    if review is not None:
                        if review["can_start"] and review["review_id"] not in report["starts"]:
                            require(review["prompt"] is None and len(report["starts"]) < 1,
                                    "unexpected additional setup start")
                            await http("POST", console.PANEL + "/start", {"review_id": review["review_id"]}, expected=202)
                            report["starts"].append(review["review_id"])
                        elif review["prompt"] is not None:
                            key = (review["review_id"], review["prompt"].get("approval_id"))
                            if key not in answered:
                                body = review["prompt"]["body"]
                                if body.get("kind") == "service_software_install":
                                    require(len(report["decisions"]) == 0, "unexpected software approval")
                                    answer = self.software_answer(review, results["checked-install"], selected_request)
                                    kind = "software"
                                else:
                                    require(not contexts and len(report["decisions"]) == 1,
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
                                    kind = "service_b_with_setup"
                                await http("POST", console.PANEL + "/answer", answer, expected=202)
                                report["decisions"].append({"kind": kind, "prompt": review, "answer": answer,
                                                           "observation": "forwarded; native Console continuation and completion checked separately"})
                                answered.add(key)
                    if len(contexts) == 1 and business is not None:
                        if access_for(data, "completion_readback") is None:
                            await asyncio.sleep(.25)
                            continue
                        completion = self.complete(frames, accepted, contexts[0], business, results, data, report)
                        if completion is not None:
                            report.update(status="passed", completion=completion,
                                reviewed_set=self.reviewed_set, helper=self.reviewed_helper)
                            break
                    await asyncio.sleep(.1)
                require(report["status"] == "passed", "source-build installed consumer did not complete")
                # Normal one-use panel offers revoke only these two displayed
                # service/catalog leaves. Setup children and the separately
                # reviewed whole policy are not swept by this action.
                retained_rows = {row["id"]: copy.deepcopy(row) for row in data["application_access"]["rows"]
                    if row["id"] in report["completion"]["issued_access_records"]}
                require(len(retained_rows) == 2, "exact B closeout rows missing")
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
                report["grant_closeout_scope"] = ("exact two displayed B service and catalog leaves revoked; "
                    "no claim that source-owned setup children, shared parents or whole policy were revoked")
            except BaseException as error:
                original_error = error
                report.update(status="failed", error=repr(error))
                raise
            finally:
                if original_error is not None:
                    provider.deadline = 0
                if enrollment is not None:
                    try:
                        retired = await asyncio.to_thread(enrollment.retire, timeout=10.0)
                        report["login_retirement"] = dataclasses.asdict(retired)
                        require(retired.fenced and retired.reservations_drained and not retired.uncertain,
                                "source-build Console login retirement is unresolved")
                    except BaseException as error:
                        report.update(status="failed", retirement_error=repr(error))
                        if original_error is None:
                            v.save_json(directory / "console-journey.json", report)
                            raise
                self.final_report = report
                v.save_json(directory / "console-journey.json", report)
            events.add("installed_source_build_consumer_completed", **report["completion"])
            return report

        def tool_pair(self, frames, run, identifier, name, args, expected, *, structured=False):
            selected = [f for f in frames if f.get("run_id") == run and f["payload"].get("id") == identifier]
            calls = [f for f in selected if f.get("kind") == "tool_call_requested"]
            results = [f for f in selected if f.get("kind") == "tool_result_received"]
            require(len(calls) <= 1 and len(results) <= 1, "native tool was replayed: " + identifier)
            if not calls or not results:
                return None
            require(all(console.run_id(f) == run and f["payload"]["name"] == name for f in calls + results)
                    and calls[0]["payload"]["args"] == args and results[0]["payload"]["is_error"] is False,
                    "actual native tool call/result differs: " + identifier)
            blocks = results[0]["payload"]["content"]
            require(isinstance(blocks, list) and len(blocks) == 1, "native result block count changed")
            if structured:
                require(blocks[0].get("type") == "structured" and blocks[0].get("data") == expected,
                        "native setup evidence is not its actual structured result")
            else:
                require(blocks[0].get("type") == "text" and isinstance(blocks[0].get("text"), str)
                        and v.unique_json(blocks[0]["text"].encode()) == expected,
                        "native result differs from model-visible JSON")
            positions = {f["id"]: i for i, f in enumerate(frames)}
            require(positions[calls[0]["id"]] < positions[results[0]["id"]], "native result preceded call")
            return {"call": calls[0]["id"], "result": results[0]["id"]}

        def complete(self, frames, accepted, context, business, results, data, report):
            originals = [f for f in frames["original"] if f.get("source", {}).get("kind") == "console_event"]
            helpers = [f for f in frames["helper"] if f.get("source", {}).get("kind") == "console_event"]
            starts = [f for f in originals if f.get("kind") == "run_started"]
            helper_starts = [f for f in helpers if f.get("kind") == "run_started"]
            require(len(starts) <= 2 and len(helper_starts) <= 1, "unexpected additional native run")
            if len(starts) != 2 or len(helper_starts) != 1 or "public-task-access" not in results:
                return None
            require(all(f["session_id"] == accepted["session_id"] for f in starts)
                    and helper_starts[0]["session_id"] == self.reviewed_helper["session_id"], "native session changed")
            a_run, b_run = [console.run_id(f) for f in starts]
            h_run = console.run_id(helper_starts[0])
            require(len({a_run, b_run, h_run}) == 3, "native run identity reused")
            for frame in starts + helper_starts:
                require(frame["payload"].get("source_event_type") == "run_started"
                        and frame["payload"].get("session_id") == frame["session_id"], "native start session contradicts payload")
            anchor = [f for f in frames["original"] if f["id"] == accepted["input_frame_id"]]
            require(len(anchor) == 1 and anchor[0]["payload"]["content"] == request_text
                    and anchor[0]["source"]["kind"] == "send" and anchor[0]["interaction_id"] == accepted["interaction_id"]
                    and anchor[0]["identity"] == "original" and anchor[0]["session_id"] == accepted["session_id"]
                    and anchor[0]["status"] in ("accepted", "delivered")
                    and sum(f.get("kind") == "user_input" and f.get("source", {}).get("kind") == "send"
                            for f in frames["original"]) == 1, "fresh continuation lacks exact single Send anchor")
            require(console.input_text(starts[0]) == request_text, "native A instruction differs")
            display = report["service_context_display"]
            expectations, observed = display["expectations"], display["validated_observed_blocks"]
            require(len(expectations) == len(observed) == 1 and len(observed[0]) == 1
                    and expectations[0]["original"] == context["fresh_original"]
                    and expectations[0]["instruction"] == request_text, "native B context expectation missing")
            value = starts[1]["payload"].get("input")
            require(isinstance(value, dict) and set(value) == {"kind", "content"} and value["kind"] == "content"
                    and isinstance(value["content"], list) and len(value["content"]) == 2
                    and all(isinstance(block, dict) and set(block) == {"type", "text"} and block["type"] == "text"
                            and isinstance(block["text"], str) for block in value["content"])
                    and value["content"][1]["text"] == request_text, "native B context/instruction blocks differ")
            context_text = value["content"][0]["text"]
            checked_context_text(context_text, expectations[0], expectations[0]["members"][0])
            require(observed == [[{"role": "user", "content": context_text}]], "native/model context bytes differ")
            for frame in originals + helpers:
                if frame.get("run_id") in (a_run, b_run, h_run):
                    console.run_id(frame)
                    require(frame.get("kind") not in ("interaction_failed", "run_failed"), "actual native interaction failed")
            done = [self.completion_frame(originals, "original", run, accepted["session_id"], text)
                    for run, text in ((a_run, v.ORIGINAL_A_FINISH), (b_run, ANSWER))]
            helper_done = self.completion_frame(helpers, "helper", h_run, self.reviewed_helper["session_id"])
            if not all(done) or helper_done is None:
                return None
            require(done[0]["payload"]["assistant_message_id"] != done[1]["payload"]["assistant_message_id"],
                    "native completion identity reused")
            original_pairs = {}
            for identifier, run, name, args in (
                ("original-request", a_run, "find_and_install", {"request": request_text, "user_context": [], "caller_observations": []}),
                ("original-catalog-search", b_run, "tool_catalog_search", {"query": OPERATION, "limit": 50}),
                ("original-catalog-load", b_run, "tool_catalog_load", {"names": [OPERATION]}),
                (CALL, b_run, OPERATION, OPERANDS)):
                pair = self.tool_pair(originals, run, identifier, name, args, results[identifier])
                if pair is None:
                    return None
                original_pairs[identifier] = pair
            require(results[CALL] == business["value"] == VALUE and business["answer"] == ANSWER,
                    "model answer is not bound to actual native compiled result")
            require(sum(f.get("kind") == "tool_call_requested" and f["payload"].get("name") == OPERATION
                        for f in originals) == 1
                    and not any(f.get("kind") == "tool_call_requested" and f["payload"].get("name") == OPERATION
                                for f in helpers), "compiled operation was repeated or delegated to helper")
            provider_request = v.unique_json((self.directory / "supplied-selected-request.json").read_bytes())
            checked = results["checked-install"]
            helper_pairs = {}
            for identifier, name, args in (
                ("schema-evidence", "setup_inspect_schema", {"schema_json": schema_raw.decode()}),
                ("package-inspection", "setup_inspect_package", {"package_json": fixture.wire(fixture.package_document()).decode()}),
                ("checked-install", "setup_selected_check", provider_request),
                ("install-once", "setup_selected_install", {key: checked[key] for key in ("validation_id", "effect_sha256")}),
                ("reconciled-install", "setup_reconcile", {"job_id": results["installed-job"]["job"]["id"]}),
                ("public-task-access", "toolkit_request_public_service", {"job_id": results["installed-job"]["job"]["id"],
                    "operations": [OPERATION], "agent": "original", "expires_in_ms": None, "tool_policy": policy_selection()})):
                pair = self.tool_pair(helpers, h_run, identifier, name, args, results[identifier],
                    structured=name in MCP_TOOLS)
                if pair is None:
                    return None
                helper_pairs[identifier] = pair
            require(sum(f.get("kind") == "tool_call_requested" and f["payload"].get("name") == "setup_selected_install"
                        for f in helpers) == 1 and len(report["decisions"]) == 2 and len(report["starts"]) == 1,
                    "actual install/review counts differ")
            pos = {f["id"]: i for i, f in enumerate(frames["original"])}
            chain = [anchor[0]["id"], starts[0]["id"], original_pairs["original-request"]["result"], done[0]["id"],
                starts[1]["id"], original_pairs["original-catalog-search"]["result"], original_pairs["original-catalog-load"]["result"],
                original_pairs[CALL]["result"], done[1]["id"]]
            require(all(pos[a] < pos[b] for a, b in zip(chain, chain[1:])), "native acquisition/use/completion order differs")
            hpos = {f["id"]: i for i, f in enumerate(helpers)}
            chain = [helper_pairs[key]["result"] for key in ("schema-evidence", "package-inspection", "checked-install",
                "install-once", "reconciled-install", "public-task-access")] + [helper_done["id"]]
            require(all(hpos[a] < hpos[b] for a, b in zip(chain, chain[1:])), "native helper evidence/install/access order differs")
            rows = self.access_snapshot(data)["rows"]
            require(len(rows) == 2 and all(row["state"] == "grant_issued" and row["can_revoke"] is True
                    and row["lifetime"]["scope"] == {"kind": "task", "task": context["fresh_original"]} for row in rows),
                    "exact fresh service/catalog access rows missing")
            service = [row for row in rows if row["resource"] == self.reviewed_set["server"]]
            require(len(service) == 1 and service[0]["lifetime"] == self.reviewed_set["service_lifetime"]
                    and next(row for row in rows if row["id"] != service[0]["id"])["lifetime"] == self.reviewed_set["catalog_lifetime"],
                    "displayed native leaf lifetimes differ from review")
            self.results = copy.deepcopy(results)
            return {"session_id": accepted["session_id"], "helper_session_id": self.reviewed_helper["session_id"],
                "a_run": a_run, "b_run": b_run, "helper_run": h_run,
                "native_start_frames": [f["id"] for f in starts], "native_completion_frames": [f["id"] for f in done],
                "helper_completion_frame": helper_done["id"], "tool_frames": original_pairs, "helper_tool_frames": helper_pairs,
                "issued_access_records": [row["id"] for row in rows], "job_id": results["installed-job"]["job"]["id"],
                "business_call_count": 1, "arguments": OPERANDS, "result": VALUE, "answer": ANSWER,
                "input_receipt_scope": "not observed; actual Console native event lineage establishes completion"}

        def check_final_artifacts(self, directory, report):
            try:
                self.verify_final_artifacts(directory, report)
            except BaseException as error:
                report["source_build_artifact_error"] = repr(error)
                raise

        def verify_final_artifacts(self, directory, report):
            if report.get("status") != "passed":
                return
            require(report.get("shutdown_response", {}).get("result", {}).get("host_cleanup_completed") is True,
                    "compiled artifact observation requires existing joined host shutdown")
            require(self.final_report["status"] == "passed" and len(self.final_report["grant_closeout"]) == 2
                    and all(row["state"] == "revoked" for row in self.final_report["grant_closeout"]), "exact leaf closeout incomplete")
            checked = self.results["checked-install"]
            installed = self.results["reconciled-install"]["observation"]["result"]
            destination = Path(checked["destination"])
            require(destination.is_absolute() and destination.resolve(strict=True) == destination, "installed destination moved")
            receipt_path = destination / "ready/installed.json"
            require(str(receipt_path) == installed["receipt_path"] and receipt_path.is_file() and not receipt_path.is_symlink(),
                    "exact installed receipt unavailable")
            raw = receipt_path.read_bytes()
            require(len(raw) <= 16*1024*1024 and hashlib.sha256(raw).hexdigest() == installed["receipt_sha256"],
                    "installed receipt changed after actual readback")
            receipt = v.unique_json(raw)
            build = receipt["node_build"]
            output_name = scenario["generated_output"]
            output = destination / output_name
            require(output.resolve(strict=True).is_relative_to(destination) and output.is_file() and not output.is_symlink()
                    and 0 < output.stat().st_size <= 16*1024*1024, "exact generated output unavailable")
            addon = output.read_bytes()
            require(addon[:4] == b"\x7fELF" and build["review"] == checked["build"]
                    and build["steps"] == [{"index": 0, "exit_code": 0}]
                    and build["outputs"] == {output_name: {"sha256": hashlib.sha256(addon).hexdigest(), "bytes": len(addon)}}
                    and not (destination / scenario["lifecycle_marker"]).exists(), "compiled output, step or disabled-script invariant differs")
            expected_command = [installed["mcp_configuration"]["mcpServers"][fixture.COMPONENT]["command"],
                str(destination / "node_modules" / fixture.PACKAGE / "server.cjs")]
            require(receipt["command"] == expected_command and receipt["mcp_configuration"] == installed["mcp_configuration"],
                    "installed command differs from actual native attachment")
            for name, payload in package_files.items():
                path = destination / "node_modules" / fixture.PACKAGE / name
                require(path.is_file() and not path.is_symlink() and path.read_bytes() == payload,
                        "retained package source changed: " + name)
            for name, pin in fixture_files.items():
                require(v.artifact_pin(fixture_root / name) == pin, "supplied fixture changed during case")
            report["source_build_observation"] = v.new_artifact(directory / "source-build-observation.json", v.canonical({
                "scope": "post-shutdown readback of exact compiled artifact; native call/result proof is in console-journey",
                "receipt": v.artifact_pin(receipt_path), "output": v.artifact_pin(output),
                "build": build, "command": expected_command, "live_origin": installed["installer_reply"],
                "fixture": fixture_files, "lifecycle_marker_absent": True, "native_completion": self.final_report["completion"]}))

    return SourceScenario(v, origin)
