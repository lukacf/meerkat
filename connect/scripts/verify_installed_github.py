#!/usr/bin/env python3
"""Opt-in installed-product GitHub get_me case. Import performs no credential or native I/O.

Uses the existing installed Console verifier, protected credential review,
native continuation, task-access revocation and shipping credential command.
No gh subprocess, token lookup, account login or alternative launch exists.
Native execution requires the supported Core cwd contract and rebuilt product.
"""
from __future__ import annotations
import argparse
import asyncio
import copy
import importlib
import importlib.util
import json
import os
from pathlib import Path
import re
import select
import signal
import stat
import subprocess
import sys
import time


def sibling(name):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).with_name(name + ".py"))
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


v = sibling("verify_native_gateway_subprocess")
c = sibling("verify_installed_console")
require = v.require
SLOT = "GITHUB_PERSONAL_ACCESS_TOKEN"
TOOLS = ["get_me", "list_commits", "get_file_contents"]
SELECTION = {"family": "official-github", "profile": "selected-reads", "operations": TOOLS,
             "values": {"token_env": SLOT}}
LAUNCH_ARGS = ["stdio", "--read-only", "--gh-host", "https://github.com", "--tools", ",".join(TOOLS)]
REQUEST = ("Install the checked official-github selected-reads package with its fixed GitHub.com read-only configuration. "
    "Use GITHUB_PERSONAL_ACCESS_TOKEN only as the declared credential reference. Request original-task access "
    "to get_me only through toolkit_request_service_account with declared_secret_slots. "
    "The operator supplies the credential only through the protected setup panel. "
    "After native attachment and ordinary continuation, call get_me once with no arguments and report the returned login and ID. "
    "Do not call other GitHub operations, request login, refresh credentials or retry a failed read.")
CALL = "github-get-me-once"


class PrivateCredential:
    """One caller-supplied pipe, read only after the exact protected review.

    No secret path, command, environment lookup or value is accepted in argv.
    The supplying owner closes the pipe after one UTF-8 credential. EOF is not
    a retry signal. This object is not a credential store or an auth authority.
    """
    def __init__(self, fd):
        require(type(fd) is int and fd >= 3 and stat.S_ISFIFO(os.fstat(fd).st_mode),
                "private credential requires an already-open pipe descriptor")
        os.set_inheritable(fd, False)
        self.fd, self.value, self.entered = fd, None, False

    def take(self):
        require(not self.entered, "private credential handoff already entered")
        self.entered = True
        raw = bytearray()
        deadline = time.monotonic() + 10
        try:
            while True:
                ready, _, _ = select.select([self.fd], [], [], max(0, deadline - time.monotonic()))
                require(bool(ready), "private credential handoff did not settle")
                part = os.read(self.fd, 16385 - len(raw))
                if not part:
                    break
                raw.extend(part)
                require(len(raw) <= 16384, "private credential exceeds bound")
            value = raw.decode("utf-8").removesuffix("\n")
            require(0 < len(value) <= 16384 and not any(ord(ch) < 33 or ord(ch) == 127 for ch in value),
                    "private credential must be one nonempty line")
            self.value = value
            return value
        except UnicodeError:
            raise AssertionError("private credential is not valid text") from None
        finally:
            raw[:] = b"\0" * len(raw)
            os.close(self.fd)
            self.fd = None

    def close(self):
        if self.fd is not None:
            os.close(self.fd)
            self.fd = None


def account_result(text, expected):
    require(isinstance(text, str) and len(text.encode()) <= 256 * 1024, "get_me result exceeds bound")
    value = v.unique_json(text.encode())
    require(isinstance(value, dict) and value.get("login") == expected["login"]
            and type(value.get("id")) is int and value["id"] == expected["id"]
            and value.get("profile_url") == "https://github.com/" + expected["login"],
            "get_me did not return the explicitly selected account")
    return {key: value[key] for key in ("login", "id", "profile_url")}


def call(provider, body, identifier, name, arguments):
    names = [row["function"]["name"] for row in body["tools"]]
    require(names.count(name) == 1 and identifier not in provider.issued,
            "selected tool absent, ambiguous or already issued")
    provider.issued.add(identifier)
    provider.events.add("controlled_model_tool", model=body["model"], call_id=identifier, tool=name, args=arguments)
    return {"delta": {"tool_calls": [{"index": 0, "id": identifier, "type": "function",
        "function": {"name": name, "arguments": json.dumps(arguments)}}]}, "finish_reason": "tool_calls"}


class GitHubScenario(c.TrelloConsoleScenario):
    name = "installed-console-github-get-me"
    scope = ("actual packaged official GitHub get_me through native installed Console, controlled model/issuer; "
             "one real authorized account read, same-original continuation and exact cleanup; no browser or provider writes")
    selection, operation, request_text = SELECTION, "get_me", REQUEST
    slots, slot_labels = (SLOT,), ("config.token_env",)
    expires_in_ms = 3600000
    selected_tool_policy = {"tools": [{"name": "get_me", "consequence": "r2"}], "catalog": "r2"}
    discard_native_stderr = True

    def __init__(self, verifier, origin, credential, expected):
        c.ConsoleScenario.__init__(self, verifier, origin)
        self.credential, self.expected = credential, expected
        self._credential_values = {SLOT: "<private-entry-not-read>"}
        self._persistence_proposal = self._credential_service_context = None
        self.account = None
        self.native_read = None
        self._secret_exposure = False
        self._cleanup_session = self._cleanup_task = None

    def wire_value(self, value):
        safe = self.report_value(value)
        if safe != value:
            self._secret_exposure = True
        return safe

    def check_final_artifacts(self, directory, report):
        super().check_final_artifacts(directory, report)
        require(not self._secret_exposure, "credential escaped into native protocol; retained evidence was redacted")

    def configure_owner(self, owner, source):
        super().configure_owner(owner, source)
        source["purpose"] = self.scope
        source["console_fixture"]["credential_setup"]["vendor_operation"] = "get_me_once"

    def configure_plan(self, plan):
        super().configure_plan(plan)
        plan["acquisition"]["console_setup"]["access_management"] = "requester_issued_leaves"
        self.realm = copy.deepcopy(plan["application"]["realm"])

    def checked_native_configuration(self, server, launch):
        # This assertion cannot pass on the old native contract. It is not a
        # serde shim: the real reviewed native configuration must retain cwd.
        require(set(launch) == {"command", "args", "cwd", "env"}
                and Path(launch["command"]).is_absolute()
                and launch["command"] == str(Path(launch["cwd"]) / "github-mcp-server")
                and Path(launch["cwd"]).is_absolute() and launch["args"] == LAUNCH_ARGS and launch["env"] == {},
                "official package launch changed its exact executable, cwd, flags or empty public environment")
        return {"name": server, "command": launch["command"], "args": LAUNCH_ARGS, "cwd": launch["cwd"],
                "env": {}, "secret_env": [SLOT], "access": {"allowed_operations": ["get_me"]}}

    async def access_answer(self, http, review, accepted, results, report):
        # The inherited checks bind the complete real package/slot/native
        # proposal, original, authenticated human, task and audience FIRST.
        answer, context = self.credential_answer(review, accepted, results)
        self._cleanup_session, self._cleanup_task = accepted["session_id"], context["fresh_original"]
        self._credential_values[SLOT] = self.credential.take()
        answer["credentials"] = [{"slot": SLOT, "value": self._credential_values[SLOT]}]
        return answer, context, c.PANEL + "/credentials", "credential_access"

    def github_provider_type(self):
        scenario = self
        base = self.credential_provider_type()

        class GitHubProvider(base):
            def check_model_request(self, body):
                scenario.check_response(body)
                require(len(v.canonical(body)) <= 8 * 1024 * 1024, "model request exceeds bound")

            def check_authentication_metadata(self, metadata):
                require("authentication" not in metadata
                        and metadata["operations"] == [{"id": name, "tool": name, "effect": "read"} for name in TOOLS],
                        "publisher changed fixed operations or credential authentication contract")

            def helper_finish(self):
                return "The native owner retained reviewed credential persistence and task continuation. The original owns the GitHub read."

            def response(self, body):
                if body["model"] != "fixture-original-model":
                    return super().response(body)
                messages = body["messages"]
                originals = [i for i, row in enumerate(messages) if row["role"] == "user" and row.get("content") == REQUEST]
                require(len(originals) in (1, 2), "unexpected original input count")
                # Before fresh continuation, reuse the ordinary Find worker.
                if len(originals) == 1:
                    return super().response(body)
                first, fresh = originals
                require(any(row["role"] == "assistant" and row.get("content") == v.ORIGINAL_A_FINISH
                            for row in messages[first + 1:fresh]), "fresh original lost its first native history")
                v.original_catalog_history(messages, REQUEST, "get_me", fresh,
                    service_context=scenario._credential_service_context)
                results = {}
                for row in messages:
                    if row["role"] == "tool":
                        key = row["tool_call_id"]
                        require(key in self.issued and key not in results, "unissued or duplicate tool result")
                        results[key] = row["content"] if key == CALL else v.unique_json(row["content"].encode())
                self.results.update(results)
                require(results["original-request"].get("worker") == {"state": "input_admitted", "deduplicated": False,
                    "source_settlement": "fresh_acceptance_promoted"}, "original worker was not actually admitted")
                search, loaded = "original-catalog-search", "original-catalog-load"
                if search not in results:
                    return call(self, body, search, "tool_catalog_search", {"query": "get_me", "limit": 50})
                found = results[search]
                matched = [row for row in found["results"] if row["name"] == "get_me"]
                require(found["catalog_exact"] is True and not found.get("pending_sources") and len(matched) == 1
                        and matched[0]["visibility_status"] in ("loaded", "deferred"), "get_me catalog entry is not usable")
                if loaded not in results:
                    return call(self, body, loaded, "tool_catalog_load", {"names": ["get_me"]})
                v.exact_catalog_load(results[loaded], ["get_me"])
                if CALL not in results:
                    return call(self, body, CALL, "get_me", {})
                account = account_result(results[CALL], scenario.expected)
                require(self.business_observation is None, "GitHub answer was replayed")
                answer = "GitHub account " + account["login"] + " (ID " + str(account["id"]) + ")."
                self.business_observation = {"answer": answer, "account": account, "actual_tool_result": results[CALL],
                    "a_input_history_index": first, "b_input_history_index": fresh,
                    "catalog_results": {search: results[search], loaded: results[loaded]}}
                self.business_response_prepared.set()
                return {"delta": {"content": answer}, "finish_reason": "stop"}
        return GitHubProvider

    def provider(self, directory, events):
        return self.github_provider_type()(directory, events)

    def completed(self, frames, accepted, context, business):
        live = [f for f in frames if f.get("source", {}).get("kind") == "console_event"
                and f.get("identity") == accepted["identity"]]
        require(not any(f.get("kind") in ("interaction_failed", "run_failed") for f in live), "native original failed")
        if context is None or business is None:
            return None
        pair = c.native_original_pair(frames, live, accepted, REQUEST, context["proposed_fresh_instruction"],
                                     fresh_context=self.v.canonical(self._credential_service_context).decode())
        if pair is None:
            return None
        first, fresh, a_run, b_run, positions = pair
        completed = []
        for run, answer in ((a_run, v.ORIGINAL_A_FINISH), (b_run, business["answer"])):
            found = [f for f in live if f.get("kind") == "interaction_complete" and f.get("run_id") == run
                     and f["payload"].get("source_event_type") == "run_completed"]
            require(len(found) <= 1, "duplicate native completion")
            if not found:
                return None
            f = found[0]
            require(c.run_id(f) == run and f.get("session_id") == accepted["session_id"]
                    and f["payload"].get("session_id") == accepted["session_id"]
                    and f.get("status") == "completed" and f["payload"]["result"] == answer
                    and f["payload"].get("extraction_required", False) is False
                    and c.nonempty(f["payload"].get("assistant_message_id")), "uncommitted or foreign native answer")
            completed.append(f)
        selected = [f for f in live if f.get("run_id") == b_run and f["payload"].get("id") == CALL]
        calls = [f for f in selected if f["kind"] == "tool_call_requested"]
        replies = [f for f in selected if f["kind"] == "tool_result_received"]
        require(len(calls) <= 1 and len(replies) <= 1, "get_me repeated")
        if not calls or not replies:
            return None
        require(all(f["payload"]["name"] == "get_me" for f in calls + replies)
                and calls[0]["payload"]["args"] == {} and replies[0]["payload"]["is_error"] is False,
                "native get_me did not succeed exactly once")
        blocks = replies[0]["payload"]["content"]
        require(len(blocks) == 1 and set(blocks[0]) == {"type", "text"} and blocks[0]["type"] == "text"
                and blocks[0]["text"] == business["actual_tool_result"]
                and account_result(blocks[0]["text"], self.expected) == business["account"], "native result/model result mismatch")
        # Native calls, not a model-proposed answer, establish discovery/use.
        observed = [f for f in live if f.get("run_id") == b_run and f.get("kind") == "tool_call_requested"]
        require(len(observed) == 3 and {f["payload"]["id"] for f in observed}
                == {"original-catalog-search", "original-catalog-load", CALL}, "unexpected fresh-original tool effect")
        previous = positions[fresh[0]["id"]]
        for identifier, name, arguments in (("original-catalog-search", "tool_catalog_search", {"query": "get_me", "limit": 50}),
                ("original-catalog-load", "tool_catalog_load", {"names": ["get_me"]})):
            invoked = [f for f in observed if f["payload"]["id"] == identifier]
            returned = [f for f in live if f.get("run_id") == b_run and f.get("kind") == "tool_result_received"
                        and f["payload"].get("id") == identifier]
            require(len(invoked) == 1 and len(returned) == 1 and invoked[0]["payload"]["name"] == name
                    and invoked[0]["payload"]["args"] == arguments and returned[0]["payload"]["name"] == name
                    and returned[0]["payload"]["is_error"] is False, "native catalog effect missing or changed")
            content = returned[0]["payload"]["content"]
            require(content and all(set(block) == {"type", "text"} and block["type"] == "text" for block in content)
                    and v.unique_json("\n".join(block["text"] for block in content).encode())
                        == business["catalog_results"][identifier], "native/model catalog result mismatch")
            require(previous < positions[invoked[0]["id"]] < positions[returned[0]["id"]]
                    < positions[calls[0]["id"]], "native discovery did not precede get_me")
            previous = positions[returned[0]["id"]]
        ordered = [first[0], completed[0], fresh[0], calls[0], replies[0], completed[1]]
        require([positions[f["id"]] for f in ordered] == sorted({positions[f["id"]] for f in ordered}),
                "native input/call/result/completion order changed")
        return {"session_id": accepted["session_id"], "a_run_id": a_run, "b_run_id": b_run,
                "frames": [f["id"] for f in ordered], "account": business["account"], "answer": business["answer"]}

    def finish_report(self, report, results, provider, completion):
        self.check_persistence_result(results["credential-task-access"])
        require([row["kind"] for row in report["decisions"]] == ["software", "credential_access"]
                and len(report["starts"]) == 1 and CALL in provider.issued, "missing actual reviews/start/read")
        self.native_read = {"status": "native_committed", "completion": completion,
                            "actual_tool_result": provider.business_observation["actual_tool_result"]}
        self.check_response(self.native_read)
        v.save_json(provider.directory / "original-business-observation.json", self.native_read)
        report.update(status="passed", completion=completion, credential_reply=results["credential-task-access"])

    async def cleanup_access(self, http, report, provider):
        if self._persistence_proposal is None:
            return
        report["grant_closeout"] = []
        server = self._persistence_proposal["server_name"]
        # Credential approval retains a Single service link. Catalog policy
        # visibility does not add the separate leaf used by batch approval.
        expected = {(server, server, ("get_me",))}

        async def current():
            data = await http("GET", c.PANEL + "/data")
            access = data.get("application_access")
            require(isinstance(access, dict) and set(access) == {"scope", "rows", "offer_id"}
                    and access["scope"] == "application_issued_access" and data["page"]["first"] is True
                    and data["page"]["has_next"] is False and data["page"]["page_id"] == access["offer_id"],
                    "current complete access cleanup view unavailable")
            rows = access["rows"]
            require(len(rows) == 1
                    and {(row["service"], row["resource"], tuple(row["operations"])) for row in rows} == expected
                    and all(row["target"] == "original" and row["selected_account"] is None
                            and row["lifetime"]["scope"] == {"kind": "task", "task": self._cleanup_task}
                            for row in rows), "exact issued GitHub service leaf was not observed")
            return access

        selected = await current()
        retained = {row["id"]: copy.deepcopy(row) for row in selected["rows"]}
        revoked = set()
        for row_id in sorted(retained):
            require(all(row == ({**retained[row["id"]], "state": "grant_unavailable", "can_revoke": False}
                                if row["id"] in revoked else retained[row["id"]]) for row in selected["rows"]),
                    "another access leaf changed during exact cleanup")
            row = next(row for row in selected["rows"] if row["id"] == row_id)
            require(row["state"] == "grant_issued" and row["can_revoke"] is True,
                    "grant cleanup is not currently offered")
            intent = {"offer_id": selected["offer_id"], "row_id": row_id}
            closeout = {"intent": intent, "reviewed": row, "state": "entered_outcome_unobserved"}
            report["grant_closeout"].append(closeout)
            result = await http("POST", c.PANEL + "/access/revoke", intent)
            require(result == {"scope": "application_issued_access", "state": "revoked"},
                    "native grant revoke did not settle")
            selected = await current()
            after = selected["rows"][0]
            require(after == {**retained[row_id], "state": "grant_unavailable", "can_revoke": False},
                    "native revoke readback absent or changed")
            revoked.add(row_id)
            closeout.update(state="revoked", result=result, readback=after)
        report["grant_closeout_scope"] = "exact service leaf; no blanket setup-parent or whole-policy revocation"


def json_objects(text):
    decoder, offset, result = json.JSONDecoder(), 0, []
    while True:
        start = text.find("{", offset)
        if start < 0:
            return result
        try:
            value, end = decoder.raw_decode(text[start:])
        except ValueError:
            offset = start + 1
            continue
        result.append(value)
        offset = start + end


def credential_cleanup(scenario, cli, directory):
    """Run the existing explicit CLI remove command only after native join.

    The native command owns current definition checks, protected review and
    joined mutation. This observer never opens a vault or changes store files.
    """
    import pty
    realm = scenario.realm
    require(all(Path(realm[key]).is_absolute() and Path(realm[key]).resolve().is_relative_to(directory.resolve())
                for key in ("state_root", "context_root", "user_config_root")),
            "credential cleanup must stay inside this isolated product case")
    proposal = scenario._persistence_proposal
    defining_realm = proposal["defining_realm"]
    require(isinstance(defining_realm, str) and bool(defining_realm.strip()),
            "cleanup requires the reviewed defining realm")
    # Native persistence can belong to a different realm than the application
    # task. Keep its reviewed owner and the same isolated storage roots.
    argv = [str(cli), "--realm", defining_realm, "--state-root", realm["state_root"],
            "--context-root", realm["context_root"], "--user-config-root", realm["user_config_root"],
            "credentials", "remove", proposal["server_name"], "--slot", SLOT]
    # Same explicit realm/user root as the product; no ambient account/env.
    environment = {"PATH": "/usr/bin:/bin", "HOME": str(directory / "isolated-home"),
                   "XDG_CONFIG_HOME": str(directory / "isolated-home" / "config"), "LANG": "C.UTF-8"}
    master, slave = pty.openpty()
    process = None
    text, approved, outcome = "", False, None
    deadline = time.monotonic() + 30
    try:
        process = subprocess.Popen(argv, stdin=slave, stdout=slave, stderr=slave, env=environment,
                                   cwd=directory, close_fds=True, start_new_session=True)
        os.close(slave)
        slave = None
        while time.monotonic() < deadline:
            ready, _, _ = select.select([master], [], [], .1)
            if ready:
                try:
                    raw = os.read(master, 65536)
                except OSError as error:
                    if error.errno != 5:
                        raise
                    raw = b""
                text += raw.decode("utf-8", errors="strict")
                require(len(text.encode()) <= 256 * 1024, "cleanup output exceeds bound")
                scenario.check_response(text)
            objects = json_objects(text)
            if not approved:
                prompts = re.findall(r"Type 'approve ([^'\r\n]+)'", text)
                if prompts:
                    require(len(prompts) == 1, "repeated cleanup review")
                    bodies = [row for row in objects if row.get("phase") == "service_credential_management"]
                    require(len(bodies) == 1 and bodies[0]["realm"] == defining_realm
                            and bodies[0]["service"] == proposal["server_name"] and bodies[0]["action"] == "remove"
                            and bodies[0]["slots"] == [SLOT], "cleanup review changed exact owner/service/slot")
                    require(bodies[0].get("current_presence") == {"owner": defining_realm,
                            "slots": {SLOT: "Stored"},
                            "scope": "presence_only_not_update_receipt_or_vendor_authentication"},
                            "cleanup review lacks the exact stored credential presence")
                    os.write(master, ("approve " + prompts[0] + "\n").encode())
                    approved = True
            settled = [row for row in objects if row.get("phase") == "service_credential_management" and "updates" in row]
            require(len(settled) <= 1, "multiple cleanup outcomes")
            if settled:
                outcome = settled[0]
            if process.poll() is not None:
                break
        require(process.poll() is not None, "credential cleanup did not join; no automatic retry")
        require(process.returncode == 0 and approved and outcome is not None
                and outcome["state"] == "updated" and outcome["action"] == "remove"
                and outcome["service"] == proposal["server_name"]
                and outcome["updates"] == [{"slot": SLOT, "state": "removed"}]
                and outcome["configuration_changed"] is False and outcome["grant_changed"] is False,
                "native credential removal was not confirmed")
        return {"status": "removed", "native_outcome": outcome, "process_exit": process.returncode,
                "scope": "this isolated realm slot only; no provider-token revocation or original account-store mutation"}
    finally:
        # Request the shipping CLI's normal interruption and join its retained
        # native effect. Never turn an interrupted/unknown removal into success.
        if process is not None and process.poll() is None:
            process.send_signal(signal.SIGINT)
            try:
                process.wait(timeout=30)
            except subprocess.TimeoutExpired:
                v.save_json(directory / "credential-cleanup-unsettled.json", {"pid": process.pid, "state": "unsettled"})
        if slave is not None:
            os.close(slave)
        os.close(master)


def require_success(report):
    result = report["native_case"]
    require(result["status"] == "passed" and report.get("read_result", {}).get("status") == "native_committed",
            "installed GitHub native read did not complete")
    grants = result.get("console_journey", {}).get("grant_closeout", [])
    require(len(grants) == 1 and all(row["state"] == "revoked" and row["readback"]["state"] == "grant_unavailable"
                                   and row["readback"]["can_revoke"] is False for row in grants),
            "the exact native service access revocation is required for success")
    require(report.get("credential_cleanup", {}).get("status") == "removed",
            "exact isolated native credential removal is required for success")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("toolkit-gateway", "credential-cli", "toolkit-python", "meta-python", "installed-staging", "output"):
        parser.add_argument("--" + name, required=True, type=Path)
    for name in ("installed-staging-sha256", "credential-cli-sha256", "expected-login"):
        parser.add_argument("--" + name, required=True)
    parser.add_argument("--expected-id", type=int, required=True)
    parser.add_argument("--credential-fd", type=int, required=True)
    parser.add_argument("--allow-authenticated-github-read", action="store_true", required=True)
    args = parser.parse_args()
    require(re.fullmatch(r"[A-Za-z0-9](?:[A-Za-z0-9-]{0,38})", args.expected_login)
            and args.expected_id > 0, "explicit known GitHub account identity required")
    for name in ("toolkit_gateway", "credential_cli", "toolkit_python", "meta_python", "installed_staging", "output"):
        path = getattr(args, name)
        require(path.is_absolute(), "all paths must be absolute")
        setattr(args, name, path.resolve())
    args.mobkit_python = args.meta_python
    candidate, package, manifest, inputs = v.installed_candidate(args)
    require(v.digest(args.credential_cli) == args.credential_cli_sha256 and os.access(args.credential_cli, os.X_OK),
            "exact credential command binary required")
    require(not args.output.exists() and not args.output.is_relative_to(package), "fresh separate evidence output required")
    args.output.mkdir(mode=0o700, parents=True)
    sys.path[:0] = [str(args.toolkit_python), str(args.meta_python)]
    sources = []
    for name, global_name, root in (("meerkat_toolkit.host_control", "SDK", args.toolkit_python),
        ("meerkat_toolkit.application_runtime", "APP_RUNTIME", args.toolkit_python),
        ("meerkat_meta.builder", "META_BUILDER", args.meta_python),
        ("meerkat_meta.identity_first_models", "META_IDENTITY", args.meta_python)):
        module = importlib.import_module(name)
        require(Path(module.__file__).resolve().is_relative_to(root), "SDK import escaped explicit source")
        setattr(v, global_name, module)
        sources.append(v.artifact_pin(Path(module.__file__)))
    credential = PrivateCredential(args.credential_fd)
    scenario = GitHubScenario(v, c.loopback_origin(), credential, {"login": args.expected_login, "id": args.expected_id})
    report = {"status": "running", "scope": scenario.scope, "inputs": inputs, "sdk_sources": sources,
              "credential_cli": v.artifact_pin(args.credential_cli), "verifier": v.artifact_pin(Path(__file__))}
    try:
        report["native_case"] = asyncio.run(v._run_installed_product(args, candidate, package, manifest,
            corrupt=False, console=scenario))
        result = report["native_case"]
        report["read_result"] = scenario.native_read
        cleanup = result.get("cleanup_observation", {})
        require(all(cleanup.get(key) is True for key in ("process_reaped", "transport_process_released",
                    "public_reader_joined", "private_reader_joined", "pending_responses_empty", "observed_descendants_gone"))
                and not result["forced_cleanup"],
                "real native process/reader/descendant cleanup is incomplete")
        if scenario._persistence_proposal is not None:
            try:
                report["credential_cleanup"] = credential_cleanup(scenario, args.credential_cli, args.output / scenario.name)
            except BaseException as error:
                report["credential_cleanup_error"] = scenario.report_value(repr(error))
                raise
        require_success(report)
        require(v.installed_candidate(args)[0] == candidate, "installed candidate changed")
        require(all(v.digest(Path(row["path"])) == row["sha256"] for row in sources), "selected SDK source changed")
        report["status"] = "passed"
    except BaseException as error:
        report.update(status="failed", error=report.get("native_case", {}).get("error")
                      or scenario.report_value(repr(error)))
    finally:
        credential.close()
        report["read_result"] = scenario.native_read
        scenario.check_response(report)
        v.save_json(args.output / "RESULT.json", report)
    return 0 if report["status"] == "passed" else 1


if __name__ == "__main__":
    raise SystemExit(main())
