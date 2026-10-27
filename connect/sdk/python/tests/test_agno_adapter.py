"""Real Agno functions at a controlled protocol boundary, never live providers."""

from __future__ import annotations

import inspect
import json
import traceback
import unittest
from unittest.mock import patch

from agno.run import RunContext
from agno.tools import Toolkit
from agno.tools.function import Function
from agno.tools.hackernews import HackerNewsTools
from agno.tools.pubmed import PubmedTools
import httpx
from pydantic import BaseModel

from meerkat_toolkit.adapters.agno import AgnoAdapterError, AgnoToolkitAdapter


CANARY = "fixture-upstream-secret-not-for-error-diagnostics"


class StructuredInput(BaseModel):
    label: str
    enabled: bool


class AgnoAdapterTests(unittest.TestCase):
    def safe(self, error: BaseException) -> None:
        self.assertNotIn(CANARY, "".join(traceback.format_exception(error)))
        self.assertNotIn(CANARY, repr(vars(error)))
        self.assertIsNone(error.__context__)
        self.assertIsNone(error.__cause__)

    def test_actual_hackernews_operations_preserve_upstream_behavior(self) -> None:
        toolkit = HackerNewsTools(timeout=7)
        original = toolkit.get_functions()["get_top_hackernews_stories"]
        entrypoint = original.entrypoint
        parameters = original.parameters.copy()
        adapter = AgnoToolkitAdapter(toolkit, functions={
            "get_top_hackernews_stories": "read", "get_user_details": "read",
        })
        requests: list[str] = []

        def get(url: str, *, timeout: int) -> httpx.Response:
            self.assertEqual(timeout, 7)
            requests.append(url)
            responses = {
                "https://hacker-news.firebaseio.com/v0/topstories.json": [10, 20, 30],
                "https://hacker-news.firebaseio.com/v0/item/10.json": {
                    "id": 10, "by": "selected-user", "title": "Story", "deleted": False,
                },
                "https://hacker-news.firebaseio.com/v0/item/20.json": None,
                "https://hacker-news.firebaseio.com/v0/user/reader.json": {
                    "id": "reader", "karma": 12, "about": None, "submitted": [10, 20],
                },
            }
            return httpx.Response(
                200, content=json.dumps(responses[url]).encode(), request=httpx.Request("GET", url),
            )

        with patch("agno.tools.hackernews.httpx.get", side_effect=get):
            with patch("agno.tools.function.FunctionCall.execute", side_effect=AssertionError):
                raw = adapter.invoke("get_top_hackernews_stories", {"num_stories": 2.0})
                self.assertIsInstance(raw, str)
                self.assertEqual(json.loads(raw), [{
                    "id": 10, "by": "selected-user", "title": "Story", "deleted": False,
                    "username": "selected-user",
                }])
                user = adapter.invoke("get_user_details", {"username": "reader"})
        self.assertEqual(json.loads(user), {
            "id": "reader", "karma": 12, "about": None, "total_items_submitted": 2,
        })
        self.assertEqual(len(requests), 4)
        self.assertIs(original.entrypoint, entrypoint)
        self.assertIs(
            inspect.unwrap(adapter._bindings[original.name].entrypoint), inspect.unwrap(entrypoint),
        )
        self.assertEqual(original.parameters, parameters)
        schema = adapter.describe()[0]["parameters"]
        self.assertEqual(schema["properties"]["num_stories"]["type"], "integer")

    def test_actual_pubmed_search_uses_host_configuration_and_real_xml_parser(self) -> None:
        toolkit = PubmedTools(
            email="fixture@example.invalid", max_results=3, results_expanded=True, timeout=9,
        )
        adapter = AgnoToolkitAdapter(toolkit, functions={"search_pubmed": "read"})
        calls: list[tuple[str, dict]] = []
        article = b"""<PubmedArticleSet><PubmedArticle><MedlineCitation><PMID>123</PMID>
          <Article><ArticleTitle>Fixture result</ArticleTitle>
            <Journal><Title>Fixture journal</Title><JournalIssue><PubDate><Year>2026</Year>
            </PubDate></JournalIssue></Journal>
            <Abstract><AbstractText Label="RESULTS">A bounded result.</AbstractText></Abstract>
            <AuthorList><Author><LastName>Reader</LastName><ForeName>Sam</ForeName></Author></AuthorList>
          </Article></MedlineCitation><PubmedData><ArticleIdList>
            <ArticleId IdType="doi">10.0000/fixture</ArticleId>
          </ArticleIdList></PubmedData></PubmedArticle></PubmedArticleSet>"""

        def get(url: str, *, params: dict, timeout: int) -> httpx.Response:
            self.assertEqual(timeout, 9)
            calls.append((url, params))
            self.assertIn(url, (
                "https://eutils.ncbi.nlm.nih.gov/entrez/eutils/esearch.fcgi",
                "https://eutils.ncbi.nlm.nih.gov/entrez/eutils/efetch.fcgi",
            ))
            content = b"<eSearchResult><IdList><Id>123</Id></IdList></eSearchResult>"
            if url.endswith("efetch.fcgi"):
                content = article
            return httpx.Response(200, content=content, request=httpx.Request("GET", url))

        with patch("agno.tools.pubmed.httpx.get", side_effect=get):
            raw = adapter.invoke("search_pubmed", {"query": "fixture query"})
        self.assertEqual(calls[0][1], {
            "db": "pubmed", "term": "fixture query", "retmax": 3,
            "email": "fixture@example.invalid", "usehistory": "y",
        })
        self.assertEqual(calls[1][1], {"db": "pubmed", "id": "123", "retmode": "xml"})
        article_text = json.loads(raw)[0]
        self.assertIn("Title: Fixture result", article_text)
        self.assertIn("First Author: Reader, Sam", article_text)
        self.assertIn("PubMed URL: https://pubmed.ncbi.nlm.nih.gov/123/", article_text)
        self.assertIn("RESULTS: A bounded result.", article_text)
        with self.assertRaises(AgnoAdapterError) as error:
            adapter.invoke("fetch_details", {"pubmed_ids": ["123"]})
        self.assertEqual(error.exception.kind, "unknown_function")

    def test_argument_validation_precedes_entry_and_schema_is_detached(self) -> None:
        calls: list[tuple] = []

        def selected(amount: int, label: str = "default") -> dict:
            calls.append((amount, label))
            return {"amount": amount, "label": label}

        function = Function(
            name="selected", entrypoint=selected, skip_entrypoint_processing=True,
            parameters={
                "type": "object", "properties": {
                    "amount": {"type": "integer", "minimum": 1},
                    "label": {"type": "string", "enum": ["default", "approved"]},
                }, "required": ["amount"], "additionalProperties": False,
            },
        )
        toolkit = Toolkit(tools=[function])
        adapter = AgnoToolkitAdapter(toolkit, functions={"selected": "write"})
        schema = adapter.describe()[0]["parameters"]
        self.assertEqual(schema, function.parameters)
        schema["properties"]["amount"]["minimum"] = -100
        for arguments in (
            {}, {"amount": "1"}, {"amount": True}, {"amount": 0},
            {"amount": 1, "label": "unapproved"}, {"amount": 1, "unexpected": CANARY},
        ):
            with self.subTest(arguments=arguments):
                with self.assertRaises(AgnoAdapterError) as error:
                    adapter.invoke("selected", arguments)
                self.assertEqual(error.exception.kind, "invalid_arguments")
                self.assertEqual(error.exception.write_outcome, "not_started")
                self.safe(error.exception)
        self.assertEqual(calls, [])
        self.assertEqual(adapter.invoke("selected", {"amount": 2}), {
            "amount": 2, "label": "default",
        })
        self.assertEqual(calls, [(2, "default")])

    def test_effect_is_explicit_and_conflicting_write_hint_is_rejected(self) -> None:
        toolkit = Toolkit(tools=[lambda: None])
        name = next(iter(toolkit.get_functions()))
        for effects in ({}, {name: None}, {name: "unknown"}, {"missing": "read"}):
            with self.subTest(effects=effects), self.assertRaises(AgnoAdapterError):
                AgnoToolkitAdapter(toolkit, functions=effects)
        toolkit.functions[name].has_side_effects = True
        with self.assertRaises(AgnoAdapterError):
            AgnoToolkitAdapter(toolkit, functions={name: "read"})
        adapter = AgnoToolkitAdapter(toolkit, functions={name: "write"})
        self.assertEqual(adapter.describe()[0]["effect"], "write")

    def test_unsupported_lifecycle_features_are_refused_without_hooks(self) -> None:
        hook_calls = []

        def selected() -> None:
            hook_calls.append("entry")

        for field, value in (
            ("cache_results", True), ("pre_hook", lambda: hook_calls.append("hook")),
            ("post_hook", lambda: hook_calls.append("hook")),
            ("tool_hooks", [lambda: hook_calls.append("hook")]),
            ("requires_confirmation", True), ("requires_user_input", True),
            ("external_execution", True), ("approval_type", "audit"),
            ("stop_after_tool_call", True), ("show_result", True),
            ("instructions", "Framework-specific invocation instructions"),
        ):
            toolkit = Toolkit(tools=[selected])
            setattr(toolkit.functions["selected"], field, value)
            with self.subTest(field=field), self.assertRaises(AgnoAdapterError):
                AgnoToolkitAdapter(toolkit, functions={"selected": "read"})
        self.assertEqual(hook_calls, [])

    def test_injected_async_generator_and_variadic_signatures_are_refused(self) -> None:
        def by_name(agent: str = "unused") -> str:
            return agent

        def by_type(context: RunContext | None = None) -> None:
            pass

        def variadic(**kwargs) -> None:
            pass

        def generator():
            yield "never enter"

        async def asynchronous() -> None:
            pass

        for function in (by_name, by_type, variadic, generator, asynchronous):
            toolkit = Toolkit(tools=[function])
            with self.subTest(function=function.__name__), self.assertRaises(AgnoAdapterError):
                AgnoToolkitAdapter(toolkit, functions={function.__name__: "read"})

    def test_failures_after_write_are_unknown_and_never_replayed(self) -> None:
        calls = []

        def failing() -> None:
            calls.append("entered")
            raise RuntimeError(CANARY)

        toolkit = Toolkit(tools=[failing])
        for effect, outcome in (("write", "unknown"), ("read", "not_applicable")):
            adapter = AgnoToolkitAdapter(toolkit, functions={"failing": effect})
            with self.assertRaises(AgnoAdapterError) as error:
                adapter.invoke("failing", {})
            self.assertEqual(error.exception.kind, "entrypoint_error")
            self.assertTrue(error.exception.entered)
            self.assertEqual(error.exception.write_outcome, outcome)
            self.safe(error.exception)
        self.assertEqual(calls, ["entered", "entered"])

    def test_result_bounds_preserve_uncertainty_and_never_truncate(self) -> None:
        calls = []

        def large() -> str:
            calls.append(1)
            return "x" * 20

        adapter = AgnoToolkitAdapter(
            Toolkit(tools=[large]), functions={"large": "write"}, max_result_bytes=10,
        )
        with self.assertRaises(AgnoAdapterError) as error:
            adapter.invoke("large", {})
        self.assertEqual(error.exception.kind, "result_too_large")
        self.assertEqual(error.exception.write_outcome, "unknown")
        self.assertEqual(calls, [1])

    def test_results_preserve_values_and_do_not_parse_embedded_error_strings(self) -> None:
        state = {"items": [None, False, {"nested": [1, 1]}]}

        def values() -> dict:
            return state

        def upstream_error() -> str:
            return '{"error": "upstream reports failure", "ok": false}'

        adapter = AgnoToolkitAdapter(Toolkit(tools=[values, upstream_error]), functions={
            "values": "read", "upstream_error": "write",
        })
        returned = adapter.invoke("values", {})
        self.assertEqual(returned, state)
        returned["items"].append("caller change")
        self.assertEqual(len(state["items"]), 3)
        self.assertEqual(adapter.invoke("upstream_error", {}), upstream_error())

    def test_accounts_stay_with_each_host_instance_and_lifecycle_is_not_called(self) -> None:
        class NoncopyableClient:
            def __deepcopy__(self, memo):
                raise AssertionError("host client cannot be copied")

        class AccountToolkit(Toolkit):
            _requires_connect = True

            def __init__(self, account: str) -> None:
                self.account = account
                self.client = NoncopyableClient()
                super().__init__(tools=[self.read_account])

            def read_account(self) -> str:
                return self.account

            def connect(self) -> None:
                raise AssertionError("host owns connection")

            def close(self) -> None:
                raise AssertionError("host owns lifetime")

        first, second = AccountToolkit("first"), AccountToolkit("second")
        first_adapter = AgnoToolkitAdapter(first, functions={"read_account": "read"})
        second_adapter = AgnoToolkitAdapter(second, functions={"read_account": "read"})
        self.assertEqual(first_adapter.invoke("read_account", {}), "first")
        self.assertEqual(second_adapter.invoke("read_account", {}), "second")
        self.assertEqual(first_adapter.invoke("read_account", {}), "first")
        self.assertIs(inspect.unwrap(first_adapter._bindings["read_account"].entrypoint).__self__, first)
        self.assertIs(inspect.unwrap(second_adapter._bindings["read_account"].entrypoint).__self__, second)

    def test_upstream_validation_wrapper_preserves_structured_argument_conversion(self) -> None:
        received = []

        def selected(value: StructuredInput) -> dict:
            received.append(type(value))
            return {"label": value.label, "enabled": value.enabled}

        adapter = AgnoToolkitAdapter(Toolkit(tools=[selected]), functions={"selected": "write"})
        self.assertEqual(adapter.invoke("selected", {"value": {
            "label": "fixture", "enabled": False,
        }}), {"label": "fixture", "enabled": False})
        self.assertEqual(received, [StructuredInput])

    def test_incomplete_schema_processing_is_not_treated_as_valid(self) -> None:
        def selected(value: str) -> str:
            raise AssertionError("not entered")

        toolkit = Toolkit(tools=[selected])
        with patch.object(Function, "process_entrypoint", return_value=None):
            with self.assertRaises(AgnoAdapterError):
                AgnoToolkitAdapter(toolkit, functions={"selected": "read"})

    def test_schema_external_references_are_rejected_before_use(self) -> None:
        def selected(value: str) -> str:
            raise AssertionError("not entered")

        function = Function(
            name="selected", entrypoint=selected, skip_entrypoint_processing=True,
            parameters={"type": "object", "properties": {
                "value": {"$ref": "https://example.invalid/schema"},
            }},
        )
        with self.assertRaises(AgnoAdapterError):
            AgnoToolkitAdapter(Toolkit(tools=[function]), functions={"selected": "read"})

    def test_non_json_and_oversized_arguments_never_enter(self) -> None:
        calls = []

        def selected(value: str) -> str:
            calls.append(value)
            return value

        adapter = AgnoToolkitAdapter(
            Toolkit(tools=[selected]), functions={"selected": "write"}, max_argument_bytes=30,
        )
        for value in ("x" * 30, float("nan"), {1: "invalid key"}, object()):
            with self.subTest(value=type(value)), self.assertRaises(AgnoAdapterError) as error:
                adapter.invoke("selected", {"value": value})
            self.assertEqual(error.exception.write_outcome, "not_started")
        self.assertEqual(calls, [])

    def test_host_interruption_propagates_without_replay_or_nonexecution_claim(self) -> None:
        calls = []

        def interrupted() -> None:
            calls.append("entry reached")
            raise KeyboardInterrupt()

        adapter = AgnoToolkitAdapter(Toolkit(tools=[interrupted]), functions={"interrupted": "write"})
        with self.assertRaises(KeyboardInterrupt):
            adapter.invoke("interrupted", {})
        self.assertEqual(calls, ["entry reached"])


if __name__ == "__main__":
    unittest.main()
