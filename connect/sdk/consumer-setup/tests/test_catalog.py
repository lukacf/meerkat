"""Pinned catalog presentation, with no installed-code or route inference."""
import copy
import http.client
import json
import threading
import unittest

from test_backend import FixtureMixin, backend


def entries():
    return [{"schema_version": 1,
             "identity": {"publisher": "fixture", "name": name, "version": "metadata-sha256:" + "a" * 64},
             "display_name": name.title(), "description": "Catalog description, not an account identity",
             "provenance": {"source": "https://example.invalid/source", "license": "MIT"},
             "strategies": [{"id": "primary", "kind": "configuration_recipe", "instructions": "Choose a declared route."},
                            {"id": "alternative", "kind": "configuration_recipe", "instructions": "<script>not authority</script>"}],
             "known_exports": [], "assessments": [{"status": "experimental"}]}
            for name in ("slack", "gmail")]


def pin(fixture, value, raw=None):
    data = backend.encoded(value) if raw is None else raw
    (fixture.bundle / "catalog.json").write_bytes(data)
    fixture.manifest["files"]["catalog.json"] = {"sha256": backend.digest(data)}
    fixture.write_manifest()


class CatalogTests(FixtureMixin, unittest.TestCase):
    def test_complete_records_before_preparation_without_effects_or_mapping(self):
        expected = entries(); pin(self, expected)
        controller = self.controller(families=("agno",))
        before = controller.state()
        listed = controller.dispatch("/api/catalog", {})
        self.assertEqual(listed["catalog_sha256"], self.manifest["files"]["catalog.json"]["sha256"])
        self.assertEqual(listed["contents_sha256"], self.sha)
        self.assertEqual(listed["scope"], "catalog_metadata_only")
        self.assertEqual([r["identity"] for r in listed["entries"]], [r["identity"] for r in expected])
        for row in expected:
            actual = controller.dispatch("/api/catalog_entry", row["identity"])
            self.assertEqual(actual["entry"], row)
            self.assertNotIn("family", actual); self.assertNotIn("profile", actual)
            actual["entry"]["strategies"].clear()
            self.assertEqual(controller.catalog_entry(row["identity"])["entry"], row)
        self.assertEqual(controller.state(), before)
        self.assertEqual(controller.runner.calls, [])
        self.assertEqual(controller.validations, {}); self.assertEqual(controller.jobs, {})
        self.assertEqual(list((controller.workspace / "requests").iterdir()), [])
        for bad in (dict(expected[0]["identity"], version="other"), dict(expected[0]["identity"], publisher="other")):
            with self.assertRaisesRegex(backend.SetupError, "unknown_catalog_entry"):
                controller.catalog_entry(bad)

    def test_missing_catalog_is_unknown_not_empty_and_preparation_still_works(self):
        controller = self.controller()
        self.assertFalse(controller.state()["catalog"]["available"])
        with self.assertRaisesRegex(backend.SetupError, "catalog_unavailable"):
            controller.catalog({})
        self.assertEqual(controller.runner.calls, [])
        controller.prepare({"family": "agno"}); controller.join()
        self.assertEqual(controller.families["agno"]["status"], "prepared")

    def test_changed_file_symlink_and_deleted_catalog_refused_without_effects(self):
        pin(self, entries()); controller = self.controller()
        path = controller.bundle / "catalog.json"
        original = path.read_bytes()
        path.write_bytes(original + b" ")
        with self.assertRaisesRegex(backend.SetupError, "catalog_digest_mismatch"):
            controller.catalog({})
        path.unlink(); path.symlink_to(self.bundle / "catalog.json")
        with self.assertRaisesRegex(backend.SetupError, "catalog_unavailable"):
            controller.catalog({})
        path.unlink()
        with self.assertRaisesRegex(backend.SetupError, "catalog_unavailable"):
            controller.catalog({})
        self.assertEqual(controller.runner.calls, [])

    def test_ambiguous_or_unsupported_presentation_refuses_safely(self):
        base = entries()
        bad = [[], {}, base + [copy.deepcopy(base[0])], [dict(base[0], schema_version=True)],
               [dict(base[0], schema_version=2)], [dict(base[0], description="x" * backend.MAX_BODY)]]
        for value in bad:
            with self.subTest(value_type=type(value).__name__):
                pin(self, value); controller = self.controller()
                with self.assertRaisesRegex(backend.SetupError, "invalid_catalog"):
                    controller.catalog({})
                self.assertEqual(controller.runner.calls, [])
        pin(self, None, b'[{"PRIVATE":"bad","PRIVATE":"duplicate"}]'); controller = self.controller()
        with self.assertRaisesRegex(backend.SetupError, "^invalid_catalog$"):
            controller.catalog({})

    def test_recovery_and_closing_admission_refuse_all_catalog_routes(self):
        pin(self, entries()); controller = self.controller()
        controller.recovery_only = True
        self.assertFalse(controller.state()["catalog"]["available"])
        for route, args in (("catalog", {}), ("catalog_entry", entries()[0]["identity"])):
            with self.assertRaisesRegex(backend.SetupError, "recovery_read_only"):
                controller.dispatch("/api/" + route, args)
        controller.recovery_only = False; controller.close_admission()
        with self.assertRaisesRegex(backend.SetupError, "host_closing"):
            controller.catalog({})
        self.assertEqual(controller.runner.calls, [])

    def test_real_http_catalog_requires_session_exact_keys_and_preserves_records(self):
        pin(self, entries()); controller = self.controller()
        server = backend.Server(controller)
        thread = threading.Thread(target=server.serve_forever); thread.start()
        def call(path, payload, authorized=True):
            connection = http.client.HTTPConnection(server.host, timeout=3)
            headers = {"Origin": server.origin, "Content-Type": "application/json", "X-Toolkit-Session": server.capability if authorized else "wrong"}
            connection.request("POST", path, backend.encoded(payload), headers)
            result = connection.getresponse(); data = json.loads(result.read()); connection.close()
            return result.status, data
        try:
            self.assertEqual(call("/api/catalog", {}, False)[0], 403)
            self.assertEqual(call("/api/catalog", {"query": "slack"})[0], 400)
            self.assertEqual(call("/api/catalog_entry", {"name": "slack"})[0], 400)
            self.assertEqual(call("/api/catalog", {})[0], 200)
            self.assertEqual(call("/api/catalog_entry", entries()[0]["identity"])[1]["entry"], entries()[0])
            controller.recovery_only = True
            self.assertEqual(call("/api/catalog", {}), (409, {"error": "recovery_read_only"}))
            self.assertEqual(call("/api/catalog_entry", entries()[0]["identity"]), (409, {"error": "recovery_read_only"}))
            self.assertEqual(controller.runner.calls, [])
        finally:
            server.shutdown(); server.server_close(); thread.join()
