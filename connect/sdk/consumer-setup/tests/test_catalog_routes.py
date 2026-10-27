"""Pinned catalog handoff through Controller and the public MCP owner."""
import asyncio
import copy
import unittest
from test_backend import FixtureMixin, backend, profiles
from test_catalog import entries, pin


def route_fixture(fixture, change=None):
    catalog = entries(); pin(fixture, catalog)
    components = {}
    for family in backend.FAMILIES:
        name = family + "/inert-artifact"
        raw = (family + " inert bytes").encode()
        (fixture.bundle / name).write_bytes(raw)
        fixture.manifest["files"][name] = {"sha256": backend.digest(raw)}
        components[family] = {"path": name, "sha256": backend.digest(raw),
                              "profiles": [p["profile"] for p in profiles(family)["profiles"]]}
    value = {"schema_version": 1, "catalog_sha256": fixture.manifest["files"]["catalog.json"]["sha256"],
        "runtime_target": fixture.manifest.get("runtime_target"),
        "provenance": {"inventory": {"path": "fixture/inventory.json", "sha256": "a" * 64},
            "profile_captures": {f: {"path": "fixture/" + f, "sha256": "b" * 64} for f in backend.FAMILIES}},
        "components": components, "routes": [{"identity": catalog[0]["identity"], "strategy_id": "primary", "family": "agno", "profile": "searxng"},
            {"identity": catalog[0]["identity"], "strategy_id": "alternative", "family": "services", "profile": "slack"}]}
    if change: change(value)
    raw = backend.encoded(value); (fixture.bundle / "routes.json").write_bytes(raw)
    fixture.manifest["files"]["routes.json"] = {"sha256": backend.digest(raw)}; fixture.write_manifest()
    return value


def request(controller, strategy="primary"):
    return {**entries()[0]["identity"], "strategy_id": strategy, "routes_sha256": controller.routes_sha256}


class RouteTests(FixtureMixin, unittest.TestCase):
    def test_readonly_route_then_exact_installed_profile_help(self):
        route_fixture(self); c = self.controller()
        entry = c.catalog_entry(entries()[0]["identity"])
        self.assertEqual([r["family"] for r in entry["setup_options"]], ["agno", "services"])
        self.assertEqual(c.catalog_route(request(c))["status"], "preparation_required")
        self.assertEqual(c.runner.calls, []); self.assertEqual(c.jobs, {}); self.assertEqual(c.validations, {})
        c.prepare({"family": "agno"}); c.join()
        result = c.catalog_route(request(c))
        self.assertEqual(result["scope"], "catalog_setup_handoff"); self.assertEqual(result["status"], "prepared"); self.assertEqual(result["help"], c.help({"family": "agno", "profile": "searxng"}))
        self.assertEqual(c.validations, {}); self.assertEqual(result["runtime_target"], None)
        self.assertEqual(len([x for x in c.runner.calls if "--destination" in x[0]]), 1)

    def test_unmapped_disabled_pending_and_unknown_installed_profile(self):
        route_fixture(self); c = self.controller(families=("agno",))
        self.assertEqual(c.catalog_entry(entries()[0]["identity"])["setup_options"][1]["status"], "disabled")
        self.assertEqual(c.catalog_entry(entries()[1]["identity"])["setup_options"][0]["status"], "manual")
        with self.assertRaises(backend.SetupError): c.catalog_route(request(c, "alternative"))
        for status in ("preparing", "unconfirmed"):
            c.families["agno"]["status"] = status
            with self.assertRaisesRegex(backend.SetupError, "preparation_unconfirmed"): c.catalog_route(request(c))
        c.families["agno"].update(status="prepared", profiles=[])
        with self.assertRaises(backend.SetupError): c.catalog_route(request(c))
        self.assertEqual(c.runner.calls, [])

    def test_bad_present_index_never_degrades_to_manual(self):
        def extra(v): v["extra"] = True
        def duplicate(v): v["routes"].append(copy.deepcopy(v["routes"][0]))
        changes = [extra, duplicate,
            lambda v: v["routes"][0].update(strategy_id="unknown"),
            lambda v: v["routes"][0]["identity"].update(name="unknown"),
            lambda v: v["routes"][0].update(profile="unknown"),
            lambda v: v["routes"][0].update(family="official-github"),
            lambda v: v["routes"][0].update(extra=1),
            lambda v: v["components"]["agno"].update(path=v["components"]["services"]["path"]),
            lambda v: v.update(catalog_sha256="f" * 64),
            lambda v: v.update(runtime_target={"not": "the selected bundle"}),
            lambda v: v.update(schema_version=True)]
        for change in changes:
            with self.subTest(change=changes.index(change)):
                route_fixture(self, change); c = self.controller()
                with self.assertRaises(backend.SetupError): c.catalog_entry(entries()[0]["identity"])
                with self.assertRaises(backend.SetupError): c.catalog_route(request(c))
                self.assertEqual(c.runner.calls, [])

    def test_malformed_catalog_strategies_are_fixed_refusals(self):
        for strategies in ([None], ["bad"], [[]], [{}], [{"id": True}], [{"id": "x"}, {"id": "x"}]):
            route_fixture(self)
            catalog = entries(); catalog[1]["strategies"] = strategies; pin(self, catalog)
            raw = backend.decode((self.bundle / "routes.json").read_bytes())
            raw["catalog_sha256"] = self.manifest["files"]["catalog.json"]["sha256"]
            data = backend.encoded(raw); (self.bundle / "routes.json").write_bytes(data)
            self.manifest["files"]["routes.json"] = {"sha256": backend.digest(data)}; self.write_manifest()
            c = self.controller()
            with self.assertRaises(backend.SetupError): c.catalog_entry(entries()[0]["identity"])
            self.assertEqual(c.runner.calls, [])

    def test_target_boolean_integer_substitution_is_not_equal(self):
        self.manifest["runtime_target"] = {"bits": 1}
        route_fixture(self, lambda v: v.update(runtime_target={"bits": True}))
        c = self.controller()
        with self.assertRaisesRegex(backend.SetupError, "binding_mismatch"): c.catalog_route(request(c))

    def test_exact_payload_tampering_index_pin_and_host_isolation(self):
        route_fixture(self); c = self.controller(); other = self.controller()
        (c.bundle / "agno/inert-artifact").write_bytes(b"replacement")
        with self.assertRaisesRegex(backend.SetupError, "artifact_mismatch"): c.catalog_route(request(c))
        self.assertEqual(other.catalog_route(request(other))["status"], "preparation_required")
        with self.assertRaisesRegex(backend.SetupError, "routes_changed"):
            other.catalog_route({**request(other), "routes_sha256": "f" * 64})
        (other.bundle / "routes.json").write_bytes(b"{}")
        with self.assertRaisesRegex(backend.SetupError, "routes_changed"): other.catalog_route(request(other))
        self.assertEqual(c.runner.calls, []); self.assertEqual(other.runner.calls, [])

    def test_legacy_absence_exact_shapes_and_recovery_admission(self):
        pin(self, entries()); c = self.controller()
        self.assertNotIn("setup_options", c.catalog_entry(entries()[0]["identity"]))
        with self.assertRaisesRegex(backend.SetupError, "routes_unavailable"): c.catalog_route({**request(c), "routes_sha256": "f" * 64})
        route_fixture(self); c = self.controller(); c.recovery_only = True
        with self.assertRaisesRegex(backend.SetupError, "recovery_read_only"): c.catalog_route(request(c))
        c.recovery_only = False; c.close_admission()
        with self.assertRaisesRegex(backend.SetupError, "host_closing"): c.catalog_route(request(c))

    def test_explicit_target_is_bound_not_inferred(self):
        self.manifest["runtime_target"] = {"python": {"platform": "linux"}}
        route_fixture(self); c = self.controller()
        self.assertEqual(c.catalog_route(request(c))["runtime_target"], self.manifest["runtime_target"])


class RouteMcpTests(FixtureMixin, unittest.IsolatedAsyncioTestCase):
    async def test_public_mcp_resolves_then_uses_existing_prepare_help_check(self):
        import test_mcp_adapter as m
        route_fixture(self)
        # Use the adapter's own Controller class, preserving the same fixture runner.
        c = self.controller(); original = m.adapter.backend
        m.adapter.backend = backend
        try:
            async with m.connection(c) as (client, owner):
                listing = await client.list_tools()
                self.assertIn("setup_catalog_route", [t.name for t in listing.tools])
                async def call(name, args):
                    result = await client.call_tool(name, args)
                    self.assertFalse(result.is_error, result)
                    return result.structured_content
                entry = await call("setup_catalog_entry", entries()[0]["identity"])
                self.assertEqual(entry["routes_sha256"], c.routes_sha256)
                self.assertEqual((await call("setup_catalog_route", request(c)))["status"], "preparation_required")
                self.assertEqual(c.runner.calls, [])
                await call("setup_prepare", {"family": "agno"})
                await asyncio.to_thread(c.join)
                result = await call("setup_catalog_route", request(c))
                self.assertEqual(result["help"]["profile"], "searxng")
                checked = await call("setup_check", self.request())
                self.assertEqual(checked["scope"], "configuration_only")
                self.assertTrue((await client.call_tool("setup_catalog_route", {**request(c), "extra": True})).is_error)
                c.recovery_only = True
                self.assertTrue((await client.call_tool("setup_catalog_route", request(c))).is_error)
        finally: m.adapter.backend = original
