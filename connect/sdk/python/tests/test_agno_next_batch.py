"""Real OpenWeather/Bitbucket/GitLab donors over offline HTTP boundaries."""

from __future__ import annotations

import asyncio
import base64
from contextlib import ExitStack
import inspect
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
import traceback
import unittest
from unittest.mock import patch
from urllib.parse import parse_qs, urlsplit

from agno.tools.bitbucket import BitbucketTools
from agno.tools.gitlab import GitlabTools
from agno.tools.openweather import OpenWeatherTools
from mcp import Client, StdioServerParameters
import requests

from meerkat_toolkit.adapters.agno_mcp import _invoke, _joined_worker, create_server
from meerkat_toolkit.adapters.agno_profiles import ProfileConfigError, build_profile, validate_configuration


KEY = "fixture-selected-key-never-disclose"
CANARY = "fixture-ambient-value-never-use"
REFERENCE = "TOOLKIT_TEST_NEXT_KEY"
HOSTILE = {
    REFERENCE: KEY, "OPENWEATHER_API_KEY": CANARY, "BITBUCKET_USERNAME": CANARY,
    "BITBUCKET_PASSWORD": CANARY, "BITBUCKET_TOKEN": CANARY, "GITLAB_ACCESS_TOKEN": CANARY,
    "GITLAB_BASE_URL": "https://ambient.invalid", "HTTP_PROXY": "http://ambient.invalid",
    "HTTPS_PROXY": "http://ambient.invalid", "REQUESTS_CA_BUNDLE": CANARY,
    "NETRC": "/nonexistent/ambient-netrc",
}
OPERATIONS = {
    "openweather": ["get_current_weather", "get_forecast", "get_air_pollution", "geocode_location"],
    "bitbucket": ["list_repositories", "get_repository_details", "list_all_pull_requests",
                  "get_pull_request_details"],
    "gitlab": ["list_projects", "get_project", "list_merge_requests", "get_merge_request", "list_issues"],
}
DONORS = {"openweather": OpenWeatherTools, "bitbucket": BitbucketTools, "gitlab": GitlabTools}


def config(name):
    extra = {"openweather": {"units": "imperial"},
             "bitbucket": {"account_email": "operator@example.invalid", "workspace": "fixture",
                           "repo_slug": "project"},
             "gitlab": {"host": "https://gitlab.example.invalid"}}[name]
    return {"timeout": 7, "api_key_env": REFERENCE, **extra}


def arguments(name, operation):
    if name == "openweather":
        return {"location": "Fixture City"}
    if operation == "get_pull_request_details":
        return {"pull_request_id": 2}
    if name == "gitlab" and operation != "list_projects":
        return {"project_id_or_path": "group/project",
                **({"merge_request_iid": 2} if operation == "get_merge_request" else {})}
    return {}


def response(request, value, status=200, headers=None):
    result = requests.Response()
    result.status_code = status
    result.url = request.url
    result.request = request
    result._content = json.dumps(value).encode()
    result.headers.update({"Content-Type": "application/json", **(headers or {})})
    return result


def provider_value(request):
    path = urlsplit(request.url).path
    if path == "/geo/1.0/direct":
        return [{"lat": 1.5, "lon": 2.5, "name": "Fixture City", "country": "XX"}]
    if path.startswith("/data/2.5/"):
        return {"fixture": path.rsplit("/", 1)[-1], "list": [], "main": {"temp": 0}}
    if path == "/2.0/repositories/fixture" or path.endswith("/pullrequests"):
        return {"values": [{"id": 2}], "next": "https://different-account.invalid/next"}
    if path.startswith("/2.0/"):
        return {"id": 2, "name": "Fixture", "is_private": False}
    project = {"id": 23, "name": "Fixture", "path": "project", "path_with_namespace": "group/project",
               "archived": False}
    if path == "/api/v4/projects":
        return [project]
    if path.endswith("/merge_requests"):
        return [{"id": 9, "iid": 2, "title": "Change", "author": {"username": "fixture"}}]
    if path.endswith("/merge_requests/2"):
        return {"id": 9, "iid": 2, "title": "Change", "author": {"username": "fixture"}}
    if path.endswith("/issues"):
        return [{"id": 8, "iid": 3, "title": "Issue", "labels": ["fixture"], "assignees": []}]
    return project


async def eventually(predicate):
    async with asyncio.timeout(4):
        while not predicate():
            await asyncio.sleep(0.01)


class OfflineMixin:
    def setUp(self):
        super().setUp()
        self.guards = ExitStack()
        self.addCleanup(self.guards.close)
        for target in ("socket.socket.connect", "socket.socket.connect_ex", "socket.getaddrinfo"):
            self.guards.enter_context(patch(target, side_effect=AssertionError("network forbidden")))


class ConfigurationTests(OfflineMixin, unittest.TestCase):
    def test_config_bounds_and_missing_key_never_fall_back(self):
        invalid = {
            "openweather": [{"units": "kelvin"}, {"api_key": KEY}],
            "bitbucket": [{"account_email": "user:password@example.invalid"}, {"workspace": "../other"},
                          {"repo_slug": "a/b"}, {"workspace": "x" * 129}, {"account_email": ""}],
            "gitlab": [{"host": "http://gitlab.invalid"}, {"host": "https://user:pass@gitlab.invalid"},
                       {"host": "https://gitlab.invalid/api/v4"}, {"host": "https://gitlab.invalid/?secret=x"}],
        }
        for name in OPERATIONS:
            selected = validate_configuration(name, operations=OPERATIONS[name], config=config(name))
            self.assertNotIn(KEY, repr(selected))
            for delta in invalid[name]:
                with self.subTest(name=name, delta=delta), self.assertRaises(ProfileConfigError) as caught:
                    validate_configuration(name, operations=OPERATIONS[name], config={**config(name), **delta})
                self.assertNotIn(KEY, str(caught.exception))
            for key in (None, "", KEY + "\n", "x" * 4097):
                with patch.dict(os.environ, HOSTILE, clear=True):
                    if key is None:
                        os.environ.pop(REFERENCE)
                    else:
                        os.environ[REFERENCE] = key
                    with self.assertRaises(ProfileConfigError) as caught:
                        build_profile(name, operations=OPERATIONS[name], config=config(name))
                    self.assertIsNone(caught.exception.__context__)
                    self.assertNotIn(KEY, "".join(traceback.format_exception(caught.exception)))

    def test_actual_inherited_methods_and_cleanup_on_partial_or_rejected_construction(self):
        closed = []
        original_close = requests.Session.close

        def close(session):
            closed.append(session)
            original_close(session)

        with patch.dict(os.environ, HOSTILE, clear=True), patch.object(requests.Session, "close", close):
            for name, donor in DONORS.items():
                profile = build_profile(name, operations=OPERATIONS[name], config=config(name))
                for op in OPERATIONS[name]:
                    original = inspect.unwrap(profile.adapter._bindings[op].entrypoint)
                    self.assertIs(original.__func__, getattr(donor, op))
                    self.assertIsInstance(original.__self__, donor)
                    self.assertIs(getattr(type(original.__self__), op), getattr(donor, op))
                if name == "gitlab":
                    self.assertIsNone(original.__self__._async_client)
                    self.assertFalse(original.__self__.client.session.trust_env)
                before = len(closed)
                profile.close()
                profile.close()
                self.assertEqual(len(closed), before + 1)
                original_init = donor.__init__

                def partial(toolkit, *args, **kwargs):
                    original_init(toolkit, *args, **kwargs)
                    raise ValueError(KEY)

                for target in (patch.object(donor, "__init__", partial),
                               patch("meerkat_toolkit.adapters.agno.AgnoToolkitAdapter", side_effect=ValueError(KEY))):
                    before = len(closed)
                    with target, self.assertRaises(ProfileConfigError) as caught:
                        build_profile(name, operations=OPERATIONS[name], config=config(name))
                    self.assertEqual(len(closed), before + 1)
                    self.assertNotIn(KEY, "".join(traceback.format_exception(caught.exception)))

    def test_json_failure_contract_is_exact_and_does_not_parse_other_values_as_failure(self):
        with patch.dict(os.environ, HOSTILE, clear=True):
            for name in OPERATIONS:
                profile = build_profile(name, operations=OPERATIONS[name][:1], config=config(name))
                try:
                    op = OPERATIONS[name][0]
                    for value, is_error in [('{"error":"' + CANARY + '"}', True),
                                            ('{ "error" : null }', True),
                                            ('{"data":{"error":"detail"}}', False),
                                            ('{"error":"field","id":1}', False),
                                            ('["error"]', False), ('not JSON', False)]:
                        with patch.object(profile.adapter, "invoke", return_value=value):
                            result = _invoke(profile, op, {})
                        self.assertEqual(result.is_error, is_error)
                        if is_error:
                            self.assertNotIn(CANARY, result.model_dump_json())
                        else:
                            self.assertEqual(result.structured_content["value"], value)
                finally:
                    profile.close()


class ProtocolTests(OfflineMixin, unittest.IsolatedAsyncioTestCase):
    async def test_two_profiles_keep_distinct_account_bindings_after_environment_changes(self):
        for name in OPERATIONS:
            seen = []

            def send(adapter, request, **kwargs):
                if name == "openweather":
                    seen.append(parse_qs(urlsplit(request.url).query)["appid"][0])
                elif name == "bitbucket":
                    seen.append(base64.b64decode(request.headers["Authorization"][6:]).decode().split(":", 1)[1])
                else:
                    seen.append(request.headers["PRIVATE-TOKEN"])
                return response(request, provider_value(request))

            profiles = []
            op = {"openweather": "geocode_location", "bitbucket": "get_repository_details",
                  "gitlab": "get_project"}[name]
            with patch.dict(os.environ, HOSTILE, clear=True), \
                    patch.object(requests.adapters.HTTPAdapter, "send", send):
                try:
                    for key in ("first-account-key", "second-account-key"):
                        os.environ[REFERENCE] = key
                        profiles.append(build_profile(name, operations=[op], config=config(name)))
                    os.environ[REFERENCE] = CANARY
                    for profile in profiles:
                        async with Client(create_server(profile)) as client:
                            result = await client.call_tool(op, arguments(name, op))
                            self.assertFalse(result.is_error)
                    self.assertEqual(seen, ["first-account-key", "second-account-key"])
                finally:
                    for profile in profiles:
                        profile.close()

    async def test_all_thirteen_real_operations_and_no_ambient_account_or_transport(self):
        calls = []
        current = None

        def send(adapter, request, **kwargs):
            calls.append(request)
            self.assertEqual(adapter.max_retries.total, 0)
            self.assertEqual(kwargs["proxies"], {})
            self.assertIs(kwargs["verify"], True)
            self.assertEqual(kwargs["timeout"], 7)
            self.assertEqual(request.method, "GET")
            self.assertNotIn(CANARY, str(request.headers) + request.url)
            parsed = urlsplit(request.url)
            query = parse_qs(parsed.query)
            if current == "openweather":
                self.assertEqual(parsed.netloc, "api.openweathermap.org")
                self.assertEqual(query["appid"], [KEY])
                self.assertNotIn("Authorization", request.headers)
                if parsed.path == "/data/2.5/forecast":
                    self.assertEqual(query["cnt"], ["16"])
                    self.assertEqual(query["units"], ["imperial"])
            elif current == "bitbucket":
                self.assertEqual(parsed.netloc, "api.bitbucket.org")
                expected = base64.b64encode(f"operator@example.invalid:{KEY}".encode()).decode()
                self.assertEqual(request.headers["Authorization"], "Basic " + expected)
                if parsed.path.endswith("/pullrequests"):
                    self.assertEqual(query, {"state": ["MERGED"], "page": ["1"], "pagelen": ["20"]})
            else:
                self.assertEqual(parsed.netloc, "gitlab.example.invalid")
                self.assertEqual(request.headers["PRIVATE-TOKEN"], KEY)
                self.assertNotIn("Authorization", request.headers)
                self.assertNotIn("JOB-TOKEN", request.headers)
                if "page" in query:
                    self.assertEqual(query["page"], ["2"])
                    self.assertEqual(query["per_page"], ["3"])
            return response(request, provider_value(request), headers={
                "Link": '<https://different-account.invalid/next>; rel="next"',
            } if current == "gitlab" else {})

        with patch.dict(os.environ, HOSTILE, clear=True), \
                patch("requests.sessions.get_netrc_auth", side_effect=AssertionError("netrc forbidden")), \
                patch.object(requests.adapters.HTTPAdapter, "send", send):
            for current in OPERATIONS:
                profile = build_profile(current, operations=OPERATIONS[current], config=config(current))
                os.environ[REFERENCE] = CANARY
                before = len(calls)
                try:
                    async with Client(create_server(profile)) as client:
                        listed = await client.list_tools()
                        self.assertEqual({t.name for t in listed.tools}, set(OPERATIONS[current]))
                        for op in OPERATIONS[current]:
                            args = arguments(current, op)
                            if op == "get_forecast":
                                args["days"] = 2.0
                            elif op == "list_all_pull_requests":
                                args["state"] = "MERGED"
                            elif current == "gitlab" and op.startswith("list_"):
                                args.update(page=2, per_page=3)
                            result = await client.call_tool(op, args)
                            self.assertFalse(result.is_error, result.model_dump_json())
                            value = json.loads(result.structured_content["value"])
                            if current == "openweather" and op != "geocode_location":
                                self.assertEqual(value["location_name"], "Fixture City")
                            if current == "gitlab" and op.startswith("list_"):
                                self.assertEqual(value["meta"], {"current_page": 2, "per_page": 3, "returned_items": 1})
                            if op == "get_project":
                                self.assertIs(value["archived"], False)
                            if op == "list_all_pull_requests":
                                self.assertEqual(value["next"], "https://different-account.invalid/next")
                                description = next(t.description for t in listed.tools if t.name == op)
                                self.assertIn("first pull-request page", description)
                    self.assertEqual(len(calls) - before, {"openweather": 7, "bitbucket": 4, "gitlab": 8}[current])
                finally:
                    profile.close()
                    os.environ[REFERENCE] = KEY

    async def test_profile_limits_reject_before_any_provider_entry(self):
        bad = {
            "openweather": [("get_current_weather", {"location": ""}),
                            ("get_forecast", {"location": "q", "days": 6}),
                            ("geocode_location", {"location": "q", "limit": 0}),
                            ("get_air_pollution", {"location": "q" * 257})],
            "bitbucket": [("list_repositories", {"count": 9}), ("list_repositories", {"count": 51}),
                          ("list_all_pull_requests", {"state": "ANY"}),
                          ("get_pull_request_details", {"pull_request_id": 0})],
            "gitlab": [("list_projects", {"page": 0}), ("list_projects", {"per_page": 51}),
                       ("list_projects", {"search": "q" * 257}), ("list_projects", {"search": None}),
                       ("get_project", {"project_id_or_path": "https://other.invalid/project"}),
                       ("get_project", {"project_id_or_path": "group%2Fproject"}),
                       ("list_merge_requests", {"project_id_or_path": "group/repo", "state": "invalid"}),
                       ("get_merge_request", {"project_id_or_path": "group/repo", "merge_request_iid": 0}),
                       ("list_issues", {"project_id_or_path": "group/repo", "state": "merged"})],
        }
        with patch.dict(os.environ, HOSTILE, clear=True), \
                patch.object(requests.adapters.HTTPAdapter, "send", side_effect=AssertionError("HTTP entry forbidden")) as send:
            for name in OPERATIONS:
                profile = build_profile(name, operations=OPERATIONS[name], config=config(name))
                try:
                    async with Client(create_server(profile)) as client:
                        for op, args in bad[name]:
                            result = await client.call_tool(op, args)
                            self.assertTrue(result.is_error)
                            self.assertFalse(result.structured_content["error"]["entered"])
                    send.assert_not_called()
                finally:
                    profile.close()

    async def test_provider_errors_redirects_and_retry_after_never_retry_or_leak(self):
        for name in OPERATIONS:
            for status in (401, 429, 500, 302):
                calls = []

                def send(adapter, request, **kwargs):
                    calls.append(request)
                    return response(request, {"message": KEY + CANARY}, status=status,
                                    headers={"Retry-After": "300", "Location": "https://other-account.invalid/"})

                with self.subTest(name=name, status=status), patch.dict(os.environ, HOSTILE, clear=True), \
                        patch.object(requests.adapters.HTTPAdapter, "send", send), \
                        patch("gitlab.utils.time.sleep", side_effect=AssertionError("SDK retry forbidden")) as sleep:
                    profile = build_profile(name, operations=OPERATIONS[name][:1], config=config(name))
                    try:
                        async with Client(create_server(profile)) as client:
                            op = OPERATIONS[name][0]
                            result = await client.call_tool(op, arguments(name, op))
                            self.assertTrue(result.is_error)
                            self.assertEqual(result.structured_content["error"]["kind"], "upstream_operation_error")
                            self.assertNotIn(KEY, result.model_dump_json())
                            self.assertNotIn(CANARY, result.model_dump_json())
                        self.assertEqual(len(calls), 1)
                        sleep.assert_not_called()
                    finally:
                        profile.close()

    async def test_request_exceptions_and_empty_geocoding_use_json_error_contract(self):
        with patch.dict(os.environ, HOSTILE, clear=True):
            for name in OPERATIONS:
                profile = build_profile(name, operations=OPERATIONS[name], config=config(name))
                try:
                    for op in OPERATIONS[name]:
                        with patch.object(requests.adapters.HTTPAdapter, "send",
                                          side_effect=requests.ReadTimeout(KEY + CANARY)) as send:
                            async with Client(create_server(profile)) as client:
                                result = await client.call_tool(op, arguments(name, op))
                                self.assertTrue(result.is_error)
                                self.assertNotIn(CANARY, result.model_dump_json())
                            self.assertEqual(send.call_count, 1)
                finally:
                    profile.close()
            profile = build_profile("openweather", operations=OPERATIONS["openweather"], config=config("openweather"))
            try:
                with patch.object(requests.adapters.HTTPAdapter, "send",
                                  lambda adapter, request, **kwargs: response(request, [])):
                    async with Client(create_server(profile)) as client:
                        result = await client.call_tool("get_current_weather", {"location": CANARY})
                        self.assertTrue(result.is_error)
                        self.assertNotIn(CANARY, result.model_dump_json())
            finally:
                profile.close()

    async def test_gitlab_client_finishes_before_session_cleanup_through_repeated_cancel(self):
        entered, release, closing, close_release = [threading.Event() for _ in range(4)]
        events = []
        original_close = requests.Session.close

        def send(adapter, request, **kwargs):
            events.append("entered")
            entered.set()
            if not release.wait(5):
                raise RuntimeError("fixture timeout")
            events.append("returned")
            return response(request, provider_value(request))

        def close(session):
            events.append("closing")
            closing.set()
            if not close_release.wait(5):
                raise RuntimeError("fixture timeout")
            original_close(session)
            events.append("closed")

        with patch.dict(os.environ, HOSTILE, clear=True), \
                patch.object(requests.adapters.HTTPAdapter, "send", send), \
                patch.object(requests.Session, "close", close):
            profile = build_profile("gitlab", operations=["list_projects"], config=config("gitlab"))
            active = cleanup = None
            try:
                async with Client(create_server(profile)) as client:
                    active = asyncio.create_task(client.call_tool("list_projects", {}))
                    await eventually(entered.is_set)
                    active.cancel()
                    await asyncio.sleep(0.03)
                    active.cancel()
                    self.assertFalse(active.done())
                    self.assertFalse(closing.is_set())
                    release.set()
                    with self.assertRaises(asyncio.CancelledError):
                        await active
                cleanup = asyncio.create_task(_joined_worker(profile.close))
                await eventually(closing.is_set)
                cleanup.cancel()
                await asyncio.sleep(0.03)
                cleanup.cancel()
                self.assertFalse(cleanup.done())
                close_release.set()
                with self.assertRaises(asyncio.CancelledError):
                    await cleanup
                self.assertEqual(events, ["entered", "returned", "closing", "closed"])
            finally:
                release.set()
                close_release.set()
                await asyncio.gather(*[t for t in (active, cleanup) if t], return_exceptions=True)
                profile.close()


STDIO = r'''
import json, sys
from pathlib import Path
from unittest.mock import patch
import requests
from meerkat_toolkit.adapters.agno_mcp import main
config, marker = map(Path, sys.argv[1:])
original_close = requests.Session.close
def close(session):
    with marker.open('a') as output:
        output.write('closed\n')
    original_close(session)
def send(adapter, request, **kwargs):
    assert kwargs['proxies'] == {} and kwargs['verify'] is True
    assert adapter.max_retries.total == 0
    result = requests.Response()
    result.status_code = 200
    result.url, result.request = request.url, request
    result.headers['Content-Type'] = 'application/json'
    value = ([{'name':'Fixture','lat':1,'lon':2}] if '/geo/' in request.url else
             [] if '/api/v4/' in request.url else {'values':[]})
    result._content = json.dumps(value).encode()
    return result
with patch('socket.socket.connect', side_effect=AssertionError('network forbidden')), \
     patch('socket.getaddrinfo', side_effect=AssertionError('network forbidden')), \
     patch.object(requests.adapters.HTTPAdapter, 'send', send), \
     patch.object(requests.Session, 'close', close):
    raise SystemExit(main(['serve', '--config', str(config)]))
'''


class StdioTests(OfflineMixin, unittest.IsolatedAsyncioTestCase):
    async def test_all_three_profiles_ordinary_stdio_config_call_and_owned_close(self):
        for name, op in (("openweather", "geocode_location"), ("bitbucket", "list_repositories"),
                         ("gitlab", "list_projects")):
            with self.subTest(name=name), tempfile.TemporaryDirectory() as directory:
                path, marker = Path(directory) / "config.json", Path(directory) / "closed"
                path.write_text(json.dumps({"profile": name, "operations": [op], "config": config(name)}))
                params = StdioServerParameters(command=sys.executable,
                                               args=["-c", STDIO, str(path), str(marker)],
                                               env={**os.environ, **HOSTILE})
                async with Client(params) as client:
                    listed = await client.list_tools()
                    self.assertEqual([t.name for t in listed.tools], [op])
                    self.assertNotIn(KEY, listed.model_dump_json())
                    result = await client.call_tool(op, arguments(name, op))
                    self.assertFalse(result.is_error, result.model_dump_json())
                await eventually(marker.exists)
                self.assertEqual(marker.read_text(), "closed\n")

    async def test_describe_has_no_request_or_secret_in_stdout(self):
        command = r'''
import sys
from unittest.mock import patch
from meerkat_toolkit.adapters.agno_mcp import main
with patch('requests.sessions.Session.request', side_effect=AssertionError('request forbidden')), \
     patch('socket.socket.connect', side_effect=AssertionError('network forbidden')):
    raise SystemExit(main(['describe', '--config', sys.argv[1]]))
'''
        for name in OPERATIONS:
            with tempfile.TemporaryDirectory() as directory:
                path = Path(directory) / "config.json"
                path.write_text(json.dumps({"profile": name, "operations": OPERATIONS[name], "config": config(name)}))
                completed = await asyncio.to_thread(subprocess.run, [sys.executable, "-c", command, str(path)],
                                                    env={**os.environ, **HOSTILE}, capture_output=True, text=True,
                                                    check=False, timeout=10)
                self.assertEqual(completed.returncode, 0, completed.stderr)
                self.assertNotIn(KEY, completed.stdout + completed.stderr)
                self.assertEqual(len(json.loads(completed.stdout)["tools"]), len(OPERATIONS[name]))


if __name__ == "__main__":
    unittest.main()
