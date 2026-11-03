"""Bounded recipe page bridge tests; canonical store coverage is separate."""
import unittest

from test_local_recipes import RecipeFixture, backend, connection, envelope, record


def page(**changes):
    value = envelope("listed_page", recipes=[record()], unverified=[], oversized=[],
                     next_cursor="ab12", request_cursor=None, scanned_records=1, limit=2)
    value.update(changes)
    return value


class RecipePageTests(RecipeFixture, unittest.TestCase):
    def test_pages_pass_exact_cursor_without_collecting_or_reordering(self):
        c = self.controller()
        cursor = None
        for raw_request, raw_next, rows, scanned in [(None, "ab12", [record()], 1),
                                                     ("ab12", "cd34", [], 200),
                                                     ("cd34", None, [record()], 1)]:
            request = {"limit": 2, **({"cursor": cursor} if cursor else {})}
            value = page(request_cursor=raw_request, next_cursor=raw_next, recipes=rows, scanned_records=scanned)
            c.runner.output = backend.encoded(value)
            result = c.recipes(request)
            self.assertEqual(result["recipes"], rows)
            self.assertEqual(result["request_cursor"], cursor)
            self.assertNotIn("ab12", str(result))
            cursor = result["next_cursor"]
            self.assertEqual(cursor is None, raw_next is None)
            if cursor is not None:
                self.assertRegex(cursor, r"^[0-9a-f]{32}$")
            argv = c.runner.calls[-1][0]
            self.assertEqual(argv[1], "list-page")
            self.assertEqual(argv[argv.index("--limit") + 1], "2")
            if raw_request:
                self.assertEqual(argv[argv.index("--cursor") + 1], raw_request)
        self.assertEqual(len(c.runner.calls), 3)
        self.assertIsNone(c.state()["job"])

    def test_invalid_requests_do_not_enter_owner(self):
        c = self.controller()
        for request in [None, [], {"cursor": "ab12"}, {"limit": True}, {"limit": 0},
                        {"limit": 51}, {"limit": 1.0}, {"limit": 1, "owner": "other"},
                        *({"limit": 1, "cursor": value} for value in [None, "", "abc", "AB12", "a" * 4098, "--owner"])]:
            self.assert_refused(lambda: c.recipes(request), "invalid_request")
        self.assertEqual(c.runner.calls, [])

    def test_invalid_or_uncorrelated_pages_never_become_complete_lists(self):
        c = self.controller()
        cases = [page(request_cursor="ab12"), page(next_cursor="abc"), page(scanned_records=True),
                 page(scanned_records=201), page(scanned_records=0), page(recipes=[record()] * 3),
                 page(unknown="private"), page(unverified=[{"error": "private"}]),
                 page(oversized=[{"identity": record()["identity"], "digest": "a" * 64, "error": "private"}]),
                 page(recipes=[{"body": "a" * backend.LOCAL_RECIPE_PAGE_BYTES}])]
        for value in cases:
            c.runner.output = backend.encoded(value)
            self.assert_refused(lambda: c.recipes({"limit": 2}), "local_recipes_failed")
        c.recipe_cursors["a" * 32] = "ab12"
        c.runner.output = backend.encoded(page(request_cursor="ab12", next_cursor="ab12"))
        self.assert_refused(lambda: c.recipes({"limit": 2, "cursor": "a" * 32}), "local_recipes_failed")
        self.assertEqual(len(c.runner.calls), len(cases) + 1)

    def test_oversized_and_unverified_observations_are_preserved(self):
        c = self.controller()
        value = page(recipes=[], unverified=[{"error": "stored_recipe_unverified"}],
                     oversized=[{"identity": record()["identity"], "digest": "a" * 64,
                                 "error": "summary_too_large"}], scanned_records=2)
        c.runner.output = backend.encoded(value)
        result = c.recipes({"limit": 2})
        self.assertEqual(result["oversized"], value["oversized"])
        self.assertEqual(result["unverified"], value["unverified"])

    def test_legacy_owner_refusal_has_no_implicit_full_list_retry(self):
        c = self.controller()
        c.runner.code = 2
        c.runner.output = b"unsupported list-page"
        self.assert_refused(lambda: c.recipes({"limit": 20}), "local_recipes_failed")
        self.assertEqual(len(c.runner.calls), 1)
        self.assertEqual(c.runner.calls[0][0][1], "list-page")


class RecipePageMcpTests(RecipeFixture, unittest.IsolatedAsyncioTestCase):
    async def test_actual_mcp_pages_and_rejects_cursor_without_limit(self):
        c = self.controller()
        c.runner.output = backend.encoded(page(limit=20))
        async with connection(c) as (client, _):
            result = await client.call_tool("setup_recipes", {"limit": 20})
            self.assertFalse(result.is_error)
            self.assertEqual(result.structured_content["recipes"], page(limit=20)["recipes"])
            self.assertRegex(result.structured_content["next_cursor"], r"^[0-9a-f]{32}$")
            result = await client.call_tool("setup_recipes", {"cursor": "ab12"})
            self.assertTrue(result.is_error)
            self.assertEqual(len(c.runner.calls), 1)


if __name__ == "__main__":
    unittest.main()
