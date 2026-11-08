"""Offline harness controls. No native execution, real credentials or vendor calls."""
import asyncio
import copy
import hashlib
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest import mock
import verify_installed_github as g
import test_credential_console_context as prior


class Handoff:
    def __init__(self):
        self.take = mock.Mock(return_value="synthetic-private-test-value")


def review_case():
    old, review, results, accepted = prior.fixture()
    scenario = g.GitHubScenario(g.v, "http://127.0.0.1:18444", Handoff(), {"login": "example", "id": 42})
    scenario.task_scope, scenario.source_authority = old.task_scope, old.source_authority
    scenario._review_owner = old._review_owner
    body = review["prompt"]["body"]
    request, decision = body["access_request"], body["service_access"]
    launch = {"command": "/isolated/package/github-mcp-server", "cwd": "/isolated/package",
              "args": g.LAUNCH_ARGS, "env": {}}
    configuration = scenario.checked_native_configuration("server", launch)
    context = request["service_instruction"]
    context["proposed_fresh_instruction"] = g.REQUEST
    context["configuration_persistence"].update(configuration=configuration, credential_slots=[g.SLOT])
    request.update(installation=configuration, application_selection={"agent": "original", "operations": ["get_me"],
        "expires_in_ms": 3600000, "tool_policy": scenario.selected_tool_policy})
    request["software"].update(family="official-github", profile="selected-reads")
    decision["permission"]["operations"] = ["get_me"]
    decision["connection_revision"] = hashlib.sha256(g.v.canonical(configuration)).hexdigest()
    results["reconciled-install"]["observation"]["result"].update(family="official-github", profile="selected-reads",
        mcp_configuration={"mcpServers": {"server": launch}})
    review["credential_input"]["slots"] = [{"slot": g.SLOT, "label": "config.token_env"}]
    def artifact(revision, members, source):
        payload = {"schema_version": 1, "provider_id": "fixture", "policy_id": "tools", "revision": revision,
                   "source": source, "default_deny": True, "members": members}
        digest = g.v.installed_fixture_policy_digest(json.dumps(payload, separators=(",", ":")).encode())
        full = {key: payload[key] for key in ("schema_version", "provider_id", "policy_id", "revision")}
        full.update(policy_digest=digest, **{key: payload[key] for key in ("source", "default_deny", "members")})
        return json.dumps(full, separators=(",", ":")).encode() + b"\n"
    grants = lambda names: [{"tool_name": name, "action": "invoke", "consequence": "r2"} for name in sorted(names)]
    members = [{"member_identity": "helper", "grants": grants(["setup_report"])},
               {"member_identity": "original", "grants": grants(["find_and_install", "get_me"])}]
    scenario.application_tool_policy = artifact(1, members, {"source_id": "fixture", "source_digest": "sha256:" + "a" * 64})
    members[1]["grants"] = grants(["find_and_install", "get_me", "tool_catalog_search", "tool_catalog_load"])
    candidate = artifact(2, members, {"source_id": "toolkit-application-service-decision", "source_digest": "sha256:" + "b" * 64})
    request["member_tool_policy"] = {"proposal": {"provider_id": "fixture", "policy_id": "tools", "member": "original",
        "selection": scenario.selected_tool_policy, "catalog_session": accepted["session_id"], "lifetime": "until_superseded",
        "prior": {"revision": 1, "digest": g.v.installed_fixture_policy_digest(scenario.application_tool_policy)},
        "candidate": {"revision": 2, "digest": g.v.installed_fixture_policy_digest(candidate)},
        "candidate_canonical_json": candidate.decode()},
        "approval_scope": "the complete successor policy, including preserved permissions for all listed members",
        "lifetime": "until superseded; this member policy remains after the service task ends",
        "catalog_visibility": "when selected, eligible tool metadata in the entire original conversation, not only this integration",
        "native_service_scope": "the separate native service and catalog grants still expire with this task"}
    decision["catalog"] = "original_session"
    return scenario, review, results, accepted


class GitHubHarnessTests(unittest.TestCase):
    def test_credential_request_proposes_exact_get_me_policy_before_private_entry(self):
        scenario, review, results, accepted = review_case()
        provider = object.__new__(scenario.github_provider_type())
        policy = {"tools": [{"name": "get_me", "consequence": "r2"}], "catalog": "r2"}
        self.assertEqual(provider.access_arguments("checked-job", ["get_me"]), {
            "job_id": "checked-job", "operations": ["get_me"], "agent": "original",
            "expires_in_ms": 3600000, "authentication": {"kind": "declared_secret_slots"},
            "tool_policy": policy})
        request = review["prompt"]["body"]["access_request"]
        self.assertEqual(request["application_selection"]["tool_policy"], policy)
        self.assertEqual(request["member_tool_policy"]["proposal"]["selection"], policy)
        for tools in ([], [{"name": "list_commits", "consequence": "r2"}]):
            wrong = copy.deepcopy(review)
            wrong["prompt"]["body"]["access_request"]["application_selection"]["tool_policy"]["tools"] = tools
            with self.subTest(tools=tools), self.assertRaises(AssertionError):
                asyncio.run(scenario.access_answer(None, wrong, accepted, results, {}))
        scenario.credential.take.assert_not_called()
        self.assertIsNone(scenario._persistence_proposal)

    def test_current_help_requires_exact_ordered_read_operations_without_authentication(self):
        scenario = g.GitHubScenario(g.v, "http://127.0.0.1:18444", Handoff(), {"login": "example", "id": 42})
        provider = object.__new__(scenario.github_provider_type())
        # setup_help normalizes publisher operations with their declared effect.
        metadata = {"operations": [{"id": name, "tool": name, "effect": "read"}
            for name in ("get_me", "list_commits", "get_file_contents")]}
        self.assertIsNone(provider.check_authentication_metadata(metadata))
        for change in ("missing_effect", "write_effect", "missing_operation", "extra_operation",
                       "reordered", "changed_tool", "extra_field", "unexpected_authentication"):
            wrong = copy.deepcopy(metadata)
            rows = wrong["operations"]
            if change == "missing_effect": rows[0].pop("effect")
            elif change == "write_effect": rows[0]["effect"] = "write"
            elif change == "missing_operation": rows.pop()
            elif change == "extra_operation": rows.append({"id": "list_issues", "tool": "list_issues", "effect": "read"})
            elif change == "reordered": rows.reverse()
            elif change == "changed_tool": rows[0]["tool"] = "other_get_me"
            elif change == "extra_field": rows[0]["extra"] = True
            else: wrong["authentication"] = {"kind": "none"}
            with self.subTest(change=change), self.assertRaisesRegex(AssertionError,
                    "publisher changed fixed operations or credential authentication contract"):
                provider.check_authentication_metadata(wrong)
        scenario.credential.take.assert_not_called()

    def test_review_is_bound_before_private_handoff(self):
        scenario, review, results, accepted = review_case()
        reply = asyncio.run(scenario.access_answer(None, review, accepted, results, {}))
        scenario.credential.take.assert_called_once_with()
        self.assertEqual(reply[2:], (g.c.PANEL + "/credentials", "credential_access"))
        self.assertEqual(reply[0]["credentials"], [{"slot": g.SLOT, "value": "synthetic-private-test-value"}])
        self.assertNotIn("synthetic-private-test-value", str(scenario.report_value(reply[0])))

    def test_foreign_review_never_reads_private_pipe(self):
        for change in ("account_operation", "slot", "cwd", "arguments", "family", "request", "duration", "catalog_session", "catalog_policy"):
            scenario, review, results, accepted = review_case()
            body = review["prompt"]["body"]
            launch = results["reconciled-install"]["observation"]["result"]["mcp_configuration"]["mcpServers"]["server"]
            if change == "account_operation": body["service_access"]["permission"]["operations"] = ["list_commits"]
            elif change == "slot": review["credential_input"]["slots"][0]["slot"] = "OTHER"
            elif change == "cwd": launch["cwd"] = "/different"
            elif change == "arguments": launch["args"] = ["stdio"]
            elif change == "family": body["access_request"]["software"]["family"] = "agno"
            elif change == "request": body["access_request"]["service_instruction"]["proposed_fresh_instruction"] = "other"
            elif change == "duration": body["access_request"]["application_selection"]["expires_in_ms"] = None
            elif change == "catalog_session": body["access_request"]["member_tool_policy"]["proposal"]["catalog_session"] = "foreign"
            else: body["access_request"]["member_tool_policy"]["proposal"]["candidate_canonical_json"] = "{}"
            with self.subTest(change=change), self.assertRaises((AssertionError, KeyError)):
                asyncio.run(scenario.access_answer(None, review, accepted, results, {}))
            scenario.credential.take.assert_not_called()

    def test_pipe_is_private_bounded_one_use_and_not_read_by_construction(self):
        read, write = os.pipe()
        try:
            private = g.PrivateCredential(read)
            self.assertFalse(os.get_inheritable(read))
            self.assertFalse(private.entered)
            os.write(write, b"synthetic-only\n")
            os.close(write); write = None
            self.assertEqual(private.take(), "synthetic-only")
            self.assertIsNone(private.fd)
            with self.assertRaises(AssertionError): private.take()
        finally:
            if write is not None: os.close(write)
            private.close()

    def test_non_pipe_and_multiline_private_inputs_refuse(self):
        with tempfile.TemporaryFile() as stream:
            with self.assertRaises(AssertionError): g.PrivateCredential(stream.fileno())
        for payload in (b"", b"a\nb", b"x" * 16385):
            read, write = os.pipe()
            # Oversize write is not needed to test the parser's bounded read.
            private = g.PrivateCredential(read)
            os.close(write)
            with mock.patch.object(g.os, "read", side_effect=[payload, b""]), self.subTest(size=len(payload)):
                with self.assertRaises(AssertionError): private.take()

    def test_exact_live_result_no_expected_value_substitution(self):
        expected = {"login": "example", "id": 42}
        actual = {**expected, "profile_url": "https://github.com/example", "details": {"public_repos": 1}}
        self.assertEqual(g.account_result(g.v.canonical(actual).decode(), expected),
                         {key: actual[key] for key in ("login", "id", "profile_url")})
        for change in ({"login": "different"}, {"id": True}, {"id": 43}, {"profile_url": "https://other/example"}):
            with self.subTest(change=change), self.assertRaises(AssertionError):
                g.account_result(g.v.canonical({**actual, **change}).decode(), expected)
        with self.assertRaises(AssertionError): g.account_result('{"error":"authorization failed"}', expected)

    def test_cleanup_requires_the_single_exact_leaf_and_actual_native_reply(self):
        scenario, _, _, _ = review_case()
        scenario._persistence_proposal = {"server_name": "server"}
        scenario._cleanup_session, scenario._cleanup_task = "session", "fresh"
        row = {"id": "a" * 64, "service": "server", "resource": "server", "target": "original",
               "operations": ["get_me"], "state": "grant_issued", "can_revoke": True,
               "selected_account": None, "lifetime": {"scope": {"kind": "task", "task": "fresh"}}}
        def snapshot(rows, offer=2):
            return {"application_access": {"scope": "application_issued_access", "offer_id": offer, "rows": rows},
                    "page": {"first": True, "has_next": False, "page_id": offer}}
        after = {**row, "state": "grant_unavailable", "can_revoke": False}
        reply = {"scope": "application_issued_access", "state": "revoked"}
        http = mock.AsyncMock(side_effect=[snapshot([row]), reply, snapshot([after], 3)])
        report = {}
        asyncio.run(scenario.cleanup_access(http, report, None))
        self.assertEqual([entry["state"] for entry in report["grant_closeout"]], ["revoked"])
        self.assertEqual(http.call_args_list[1].args, ("POST", g.c.PANEL + "/access/revoke", {"offer_id": 2, "row_id": row["id"]}))
        self.assertEqual(http.call_count, 3)
        for wrong in ({**row, "service": "foreign"}, {**row, "operations": g.TOOLS},
                      {**row, "resource": "foreign"}, {**row, "target": "helper"}, {**row, "selected_account": "foreign"},
                      {**row, "lifetime": {"scope": {"kind": "task", "task": "foreign"}}}):
            http = mock.AsyncMock(return_value=snapshot([wrong]))
            with self.assertRaises(AssertionError): asyncio.run(scenario.cleanup_access(http, {}, None))
            self.assertEqual(http.call_count, 1)
        catalog = {**row, "id": "b" * 64, "service": "public_service_set_catalog", "resource": "session", "operations": ["catalog"]}
        for rows in ([], [row, row], [row, catalog]):
            http = mock.AsyncMock(return_value=snapshot(rows))
            with self.assertRaises(AssertionError): asyncio.run(scenario.cleanup_access(http, {}, None))
            self.assertEqual(http.call_count, 1)
        report = {}
        http = mock.AsyncMock(side_effect=[snapshot([row]), OSError("uncertain")])
        with self.assertRaises(OSError): asyncio.run(scenario.cleanup_access(http, report, None))
        self.assertEqual([entry["state"] for entry in report["grant_closeout"]], ["entered_outcome_unobserved"])
        self.assertEqual(http.call_count, 2)
        for changed in (row, {**after, "id": "b" * 64}, {**after, "selected_account": "foreign"},
                        {**after, "lifetime": {**row["lifetime"], "expires_at_ms": 1}}):
            report = {}
            http = mock.AsyncMock(side_effect=[snapshot([row]), reply, snapshot([changed], 3)])
            with self.assertRaises(AssertionError): asyncio.run(scenario.cleanup_access(http, report, None))
            self.assertEqual([entry["state"] for entry in report["grant_closeout"]], ["entered_outcome_unobserved"])
            self.assertEqual(http.call_count, 3)

    def test_success_requires_revocation_and_secret_removal_but_keeps_read(self):
        read = {"status": "native_committed", "completion": {"account": {"login": "example", "id": 42}}}
        grant = {"state": "revoked", "readback": {"state": "grant_unavailable", "can_revoke": False}}
        report = {"native_case": {"status": "passed", "console_journey": {"grant_closeout": [grant]}},
                  "read_result": read, "credential_cleanup": {"status": "removed"}}
        g.require_success(report)
        for change in ("missing_credential", "unknown_credential", "missing_grant", "extra_grant", "unknown_grant", "native_failed"):
            wrong = copy.deepcopy(report)
            if change == "missing_credential": wrong.pop("credential_cleanup")
            elif change == "unknown_credential": wrong["credential_cleanup"]["status"] = "uncertain"
            elif change == "missing_grant": wrong["native_case"]["console_journey"]["grant_closeout"].pop()
            elif change == "extra_grant": wrong["native_case"]["console_journey"]["grant_closeout"].append(grant)
            elif change == "unknown_grant": wrong["native_case"]["console_journey"]["grant_closeout"][0]["state"] = "entered_outcome_unobserved"
            else: wrong["native_case"]["status"] = "failed"
            with self.subTest(change=change), self.assertRaises(AssertionError): g.require_success(wrong)
            self.assertEqual(wrong["read_result"], read)

    def test_protocol_secret_is_redacted_before_retention_and_fails_acceptance(self):
        scenario, _, _, _ = review_case()
        scenario._credential_values[g.SLOT] = "synthetic-private-test-value"
        value = {"result": "error synthetic-private-test-value"}
        safe = scenario.wire_value(value)
        self.assertEqual(safe, {"result": "error [REDACTED]"})
        self.assertTrue(scenario._secret_exposure)
        with tempfile.TemporaryDirectory() as directory, self.assertRaises(AssertionError):
            scenario.check_final_artifacts(Path(directory), {})
        self.assertTrue(scenario.discard_native_stderr)

    def test_cli_cleanup_uses_only_isolated_realm_after_exact_review(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            scenario, _, _, _ = review_case()
            scenario.realm = {"realm_id": "isolated", **{key: str(directory / key)
                for key in ("state_root", "context_root", "user_config_root")}}
            scenario._persistence_proposal = {"defining_realm": "installation", "server_name": "server"}
            presence = {"owner": "installation", "slots": {g.SLOT: "Stored"},
                        "scope": "presence_only_not_update_receipt_or_vendor_authentication"}
            body = {"phase": "service_credential_management", "realm": "installation", "service": "server",
                    "action": "remove", "slots": [g.SLOT], "current_presence": presence}
            outcome = {"phase": "service_credential_management", "service": "server", "action": "remove", "state": "updated",
                       "updates": [{"slot": g.SLOT, "state": "removed"}], "configuration_changed": False, "grant_changed": False}
            chunks = [(json.dumps(body) + "\nType 'approve exact'\n").encode(), json.dumps(outcome).encode()]
            process = mock.Mock(returncode=0)
            process.poll.side_effect = [None, 0, 0, 0]
            with mock.patch.object(g.subprocess, "Popen", return_value=process) as spawn, \
                 mock.patch.object(g.select, "select", return_value=([3], [], [])), \
                 mock.patch.object(g.os, "read", side_effect=chunks), mock.patch.object(g.os, "write") as write:
                result = g.credential_cleanup(scenario, Path("/selected/cli"), directory)
            self.assertEqual(result["status"], "removed")
            self.assertEqual(spawn.call_args.args[0][1:9], ["--realm", "installation",
                "--state-root", scenario.realm["state_root"], "--context-root", scenario.realm["context_root"],
                "--user-config-root", scenario.realm["user_config_root"]])
            self.assertEqual(spawn.call_args.args[0][-5:], ["credentials", "remove", "server", "--slot", g.SLOT])
            self.assertEqual(spawn.call_args.kwargs["env"], {"PATH": "/usr/bin:/bin", "HOME": str(directory / "isolated-home"),
                "XDG_CONFIG_HOME": str(directory / "isolated-home" / "config"), "LANG": "C.UTF-8"})
            self.assertEqual(write.call_args.args[1], b"approve exact\n")
            for key in ("state_root", "context_root", "user_config_root"):
                wrong = copy.deepcopy(scenario.realm)
                wrong[key] = str(directory.parent)
                with mock.patch.object(scenario, "realm", wrong), mock.patch.object(g.subprocess, "Popen") as spawn:
                    with self.subTest(outside_root=key), self.assertRaises(AssertionError):
                        g.credential_cleanup(scenario, Path("/selected/cli"), directory)
                    spawn.assert_not_called()
            for defining_realm in (None, "", " "):
                wrong = {**scenario._persistence_proposal, "defining_realm": defining_realm}
                with mock.patch.object(scenario, "_persistence_proposal", wrong), mock.patch.object(g.subprocess, "Popen") as spawn:
                    with self.subTest(defining_realm=defining_realm), self.assertRaises(AssertionError):
                        g.credential_cleanup(scenario, Path("/selected/cli"), directory)
                    spawn.assert_not_called()
            wrong_body = {**body, "realm": scenario.realm["realm_id"]}
            process = mock.Mock(returncode=0)
            process.poll.return_value = 0
            with mock.patch.object(g.subprocess, "Popen", return_value=process), \
                 mock.patch.object(g.select, "select", return_value=([3], [], [])), \
                 mock.patch.object(g.os, "read", return_value=(json.dumps(wrong_body) + "\nType 'approve exact'\n").encode()), \
                 mock.patch.object(g.os, "write") as write:
                with self.assertRaisesRegex(AssertionError, "cleanup review changed exact owner/service/slot"):
                    g.credential_cleanup(scenario, Path("/selected/cli"), directory)
                write.assert_not_called()
            missing_presence = {key: value for key, value in body.items() if key != "current_presence"}
            wrong_presence = [None, {"state": "unavailable"}, {**presence, "owner": "foreign"},
                {**presence, "slots": {g.SLOT: "NotStored"}}, {**presence, "slots": {g.SLOT: "NotObserved"}},
                {**presence, "slots": {g.SLOT: "Stored", "extra": "Stored"}},
                {**presence, "slots": {"wrong": "Stored"}}, {**presence, "scope": "unverified"}]
            for wrong_body in [missing_presence, *({**body, "current_presence": value} for value in wrong_presence)]:
                process = mock.Mock(returncode=0)
                process.poll.return_value = 0
                with self.subTest(presence=wrong_body.get("current_presence", "missing")), \
                     mock.patch.object(g.subprocess, "Popen", return_value=process), \
                     mock.patch.object(g.select, "select", return_value=([3], [], [])), \
                     mock.patch.object(g.os, "read", return_value=(json.dumps(wrong_body) + "\nType 'approve exact'\n").encode()), \
                     mock.patch.object(g.os, "write") as write:
                    with self.assertRaisesRegex(AssertionError, "cleanup review lacks the exact stored credential presence"):
                        g.credential_cleanup(scenario, Path("/selected/cli"), directory)
                    write.assert_not_called()


if __name__ == '__main__':
    unittest.main()
