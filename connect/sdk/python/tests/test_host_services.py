"""Isolation at the shared handle boundary, without fake policy authority."""

import unittest
from collections.abc import Mapping
from concurrent.futures import ThreadPoolExecutor
from threading import Barrier

from meerkat_toolkit.host_services import HostServiceError, StagedHostServices


class StagedHostServicesTests(unittest.TestCase):
    def assert_kind(self, kind, operation):
        with self.assertRaises(HostServiceError) as error:
            operation()
        self.assertEqual(error.exception.kind, kind)

    def test_optional_absence_and_undeclared_handles_cannot_be_added(self):
        view = StagedHostServices(declared=["media", "optional"], available=["media"])
        self.assert_kind("services_not_bound", lambda: view.get("media"))
        self.assert_kind("service_unavailable", lambda: view.get("optional"))
        self.assert_kind("service_not_declared", lambda: view.get("shell"))
        for handles in ({}, {"media": None}, {"media": object(), "optional": object()},
                        {"media": object(), "shell": object()}):
            self.assert_kind("service_handles_differ_from_selection", lambda: view.bind(handles))
            self.assertFalse(view.ready)
        media = object()
        view.bind({"media": media})
        self.assertIs(view.get("media"), media)
        self.assert_kind("service_unavailable", lambda: view.get("optional"))
        self.assert_kind("service_not_declared", lambda: view.get("shell"))
        self.assert_kind("services_already_bound", lambda: view.bind({"media": media}))

    def test_inputs_cannot_retarget_an_existing_view(self):
        declared = ["media"]
        available = ["media"]
        view = StagedHostServices(declared=declared, available=available)
        declared.append("shell")
        available.append("shell")
        handle = object()
        handles = {"media": handle}
        view.bind(handles)
        handles["media"] = object()
        handles["shell"] = object()
        self.assertIs(view.get("media"), handle)
        self.assertEqual(view.declared, frozenset(["media"]))
        self.assert_kind("service_not_declared", lambda: view.get("shell"))
        with self.assertRaises(AttributeError):
            view.available = frozenset(["shell"])

    def test_two_views_keep_distinct_scoped_handles(self):
        first = StagedHostServices(declared=["media"], available=["media"])
        second = StagedHostServices(declared=["media"], available=["media"])
        one, two = object(), object()
        first.bind({"media": one})
        second.bind({"media": two})
        self.assertIs(first.get("media"), one)
        self.assertIs(second.get("media"), two)

    def test_empty_selection_still_has_single_assignment(self):
        view = StagedHostServices(declared=[], available=[])
        view.bind({})
        self.assertTrue(view.ready)
        self.assert_kind("services_already_bound", lambda: view.bind({}))

    def test_reentrant_mapping_cannot_replace_an_already_bound_handle(self):
        view = StagedHostServices(declared=["media"], available=["media"])
        winner, rejected = object(), object()

        class ReentrantMapping(Mapping):
            def __iter__(self):
                view.bind({"media": winner})
                return iter(["media"])

            def __len__(self):
                return 1

            def __getitem__(self, name):
                return rejected

        self.assert_kind("services_already_bound", lambda: view.bind(ReentrantMapping()))
        self.assertIs(view.get("media"), winner)

    def test_concurrent_bind_has_one_winner(self):
        view = StagedHostServices(declared=["media"], available=["media"])
        barrier = Barrier(2)

        def bind(handle):
            barrier.wait(timeout=5)
            try:
                view.bind({"media": handle})
                return handle
            except HostServiceError as error:
                self.assertEqual(error.kind, "services_already_bound")
                return None

        with ThreadPoolExecutor(max_workers=2) as pool:
            outcomes = list(pool.map(bind, [object(), object()]))
        winners = [handle for handle in outcomes if handle is not None]
        self.assertEqual(len(winners), 1)
        self.assertIs(view.get("media"), winners[0])

    def test_invalid_names_and_unselected_availability_are_refused(self):
        for names in ("media", b"media", [""], [None], [1]):
            self.assert_kind("invalid_service_names", lambda: StagedHostServices(
                declared=names, available=[]))
        self.assert_kind("available_service_not_declared", lambda: StagedHostServices(
            declared=["media"], available=["shell"]))


if __name__ == "__main__":
    unittest.main()
