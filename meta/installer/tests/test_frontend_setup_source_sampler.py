"""The actual bounded public-source sampler never opens a symlink target."""
from __future__ import annotations

import ast
import copy
import os
from pathlib import Path

import pytest

APP_SOURCE = Path(os.environ.get("FORGE_PRIVATE_DECLARATIONS_APP",
    Path(__file__).resolve().parents[1] / "launchkit/templates_v2/app.py"))


def test_actual_setup_sampler_refuses_both_symlinks_before_open_with_missing_and_ordinary_preserved(tmp_path, monkeypatch):
    # Execute only the actual product sampler. No application imports,
    # runtime, SDK, provider, server, process or installed action occurs.
    tree = ast.parse(APP_SOURCE.read_text())
    node = next(node for node in tree.body if isinstance(node, ast.FunctionDef)
                and node.name == "console_setup_source_bytes")
    namespace = {}
    exec(compile(ast.fix_missing_locations(ast.Module(body=[copy.deepcopy(node)], type_ignores=[])),
                 str(APP_SOURCE), "exec"), namespace)
    sample = namespace["console_setup_source_bytes"]
    directory = tmp_path / "application"
    directory.mkdir()
    configuration = directory / "configuration.json"
    authentication = directory / "console.json"
    assert sample(tmp_path) == {}
    expected = {"application/configuration.json": b'{"fields":[]}',
                "application/console.json": b'{"authentication":"local-password"}'}
    for name, contents in expected.items():
        (tmp_path / name).write_bytes(contents)
    assert sample(tmp_path) == expected
    target = tmp_path / "inert-target-not-to-be-read"
    target.write_bytes(b"inert-target-opening-canary")
    original_open = Path.open
    for source in (configuration, authentication):
        source.unlink()
        source.symlink_to(target)
        opened = []
        def no_symlink_open(path, *args, **kwargs):
            opened.append(path)
            if path == source or path == target:
                pytest.fail("The sampler attempted to open the symlink or its target.")
            return original_open(path, *args, **kwargs)
        with monkeypatch.context() as control:
            control.setattr(Path, "open", no_symlink_open)
            with pytest.raises(ValueError, match="bounded ordinary JSON"):
                sample(tmp_path)
        assert source not in opened and target not in opened
        source.unlink()
        source.write_bytes(expected[source.relative_to(tmp_path).as_posix()])
