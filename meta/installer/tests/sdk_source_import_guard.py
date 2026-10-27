"""Forbid SDK imports during selected byte installation, preserving host imports."""
import sys

import pytest


@pytest.fixture(autouse=True)
def no_selected_sdk_import(monkeypatch):
    import builtins
    import importlib

    roots = {"meerkat_meta", "meerkat_toolkit", "fixture_sequence", "fixture_actual_pip"}
    before = {name: module for name, module in sys.modules.items()
              if name.split(".")[0] in roots}
    original_import, original_module = builtins.__import__, importlib.import_module
    def guarded_import(name, *args, **kwargs):
        if name.split(".")[0] in roots:
            pytest.fail("Selected SDK byte installation must not import SDK source.")
        return original_import(name, *args, **kwargs)
    def guarded_module(name, *args, **kwargs):
        if name.split(".")[0] in roots:
            pytest.fail("Selected SDK byte installation must not load an SDK module.")
        return original_module(name, *args, **kwargs)
    monkeypatch.setattr(builtins, "__import__", guarded_import)
    monkeypatch.setattr(importlib, "import_module", guarded_module)
    yield
    assert {name: module for name, module in sys.modules.items()
            if name.split(".")[0] in roots} == before
