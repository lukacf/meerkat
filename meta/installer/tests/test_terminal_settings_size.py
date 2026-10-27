"""Generated terminal setup keeps its serialized settings readable by Forge."""
import ast
import hashlib
import json
import os
from pathlib import Path

import pytest

from launchkit.deployment import private_configuration_snapshot
from test_generated_setup import setup


MAX_BYTES = 256 * 1024
HEADER = "# Local private configuration. Never commit this file.\n"


def serialized(values):
    return (HEADER + "".join(f"{key}={json.dumps(value)}\n"
                            for key, value in sorted(values.items()))).encode("ascii")


@pytest.fixture
def interview(setup):
    module, root, spec = setup
    if os.environ.get("FORGE_TEST_TERMINAL_SIZE_BASELINE") == "1":
        source = Path(__file__).parents[1] / ".artifacts/forge-terminal-settings-size-20261010/setup.py.before"
        assert hashlib.sha256(source.read_bytes()).hexdigest() == "a10d8460c5f418985db46ec770d161ab8871ad1db0495f1ad4265698149c0b9f"
        original = next(node for node in ast.parse(source.read_bytes()).body
                        if isinstance(node, ast.FunctionDef) and node.name == "save_values")
        namespace = {}
        exec(compile(ast.Module(body=[original], type_ignores=[]), str(source), "exec"), module.__dict__, namespace)
        module.save_values = namespace["save_values"]
    return module, root, spec


def declare_fields(root, values, *, required=True):
    application = root / "application"
    application.mkdir(exist_ok=True)
    (application / "configuration.json").write_text(json.dumps({"fields": [
        {"key": key, "label": f"Private setting {index}", "secret": True,
         "required": required, "max_length": 32768}
        for index, key in enumerate(values)]}))


def retain_private(root):
    path = root / ".env"
    prior = b'UNRELATED_PRIVATE="retained-private-canary"\n'
    path.write_bytes(prior)
    path.chmod(0o600)
    return path, prior, (path.stat().st_dev, path.stat().st_ino)


@pytest.mark.parametrize("encoding,retained", [
    ("ascii", False), ("ascii", True), ("unicode", False), ("unicode", True)
], ids=["first-ascii", "retained-ascii", "first-unicode", "retained-unicode"])
def test_generated_interview_refuses_aggregate_before_creating_pending_or_replacing_private(
        interview, monkeypatch, capsys, encoding, retained):
    module, root, _ = interview
    if encoding == "ascii":
        value = "private-size-canary-" + "x" * (32768 - len("private-size-canary-"))
        values = {f"APP_SIZE_{index}": value for index in range(9)}
    else:
        value = "private-unicode-canary-" + chr(0x1F680) * (32768 - len("private-unicode-canary-"))
        values = {"APP_SIZE_UNICODE": value}
    assert all(len(value) == 32768 for value in values.values())
    declare_fields(root, values)
    for key in values:
        monkeypatch.delenv(key, raising=False)
    path = root / ".env"
    prior, identity = None, None
    if retained:
        path, prior, identity = retain_private(root)
    expected_values = {**({"UNRELATED_PRIVATE": "retained-private-canary"} if retained else {}), **values}
    assert len(serialized(expected_values)) > MAX_BYTES
    replies = iter(values.values())
    prompts, created = [], []
    original_mkstemp = module.tempfile.mkstemp

    def answer(prompt):
        prompts.append(prompt)
        assert (path.read_bytes() if path.exists() else None) == prior
        return next(replies)

    def mkstemp(*args, **kwargs):
        created.append((args, kwargs))
        return original_mkstemp(*args, **kwargs)

    monkeypatch.setattr(module.getpass, "getpass", answer)
    monkeypatch.setattr(module.tempfile, "mkstemp", mkstemp)
    with pytest.raises(ValueError, match="private configuration exceeded its safe size") as failure:
        module.main()
    assert len(prompts) == len(values), "Each individually valid answer reached the aggregate check."
    assert created == [] and not list(root.glob(".env.*.pending"))
    assert (path.read_bytes() if path.exists() else None) == prior
    if retained:
        assert (path.stat().st_dev, path.stat().st_ino) == identity
        assert path.stat().st_mode & 0o777 == 0o600
    output = capsys.readouterr().out + "".join(prompts) + str(failure.value)
    assert "No values were changed" in str(failure.value) and "run setup again" in str(failure.value)
    assert "Saved." not in output
    assert all(canary not in output for canary in (
        "private-size-canary", "private-unicode-canary", "retained-private-canary"))


def boundary_values(size):
    # Eight individually valid values can straddle the exact aggregate byte limit.
    values = {f"BOUNDARY_{index}": "x" * 32768 for index in range(8)}
    final = "BOUNDARY_7"
    excess = len(serialized(values)) - size
    assert 0 <= excess < len(values[final])
    values[final] = values[final][:-excess] if excess else values[final]
    assert len(serialized(values)) == size
    assert all(len(value) <= 32768 for value in values.values())
    return values


@pytest.mark.parametrize("size", [MAX_BYTES - 1, MAX_BYTES], ids=["one-byte-below", "exact-limit"])
def test_exact_serialized_boundary_preserves_format_and_is_readable_by_existing_consumer(
        interview, size):
    module, root, _ = interview
    path, prior, _ = retain_private(root)
    values = boundary_values(size)
    module.save_values(path, values, prior)
    assert path.read_bytes() == serialized(values)
    assert path.stat().st_mode & 0o777 == 0o600
    assert module.private_values(path)[0] == values
    assert private_configuration_snapshot(root)[0] == values
    assert not list(root.glob(".env.*.pending"))


def test_one_serialized_byte_above_limit_refuses_before_tempfile_and_preserves_identity(
        interview, monkeypatch, capsys):
    module, root, _ = interview
    path, prior, identity = retain_private(root)
    values = boundary_values(MAX_BYTES + 1)

    def unexpected(*args, **kwargs):
        pytest.fail("An oversized configuration must be refused before creating a pending file.")

    monkeypatch.setattr(module.tempfile, "mkstemp", unexpected)
    with pytest.raises(ValueError, match="private configuration exceeded its safe size") as failure:
        module.save_values(path, values, prior)
    assert path.read_bytes() == prior and (path.stat().st_dev, path.stat().st_ino) == identity
    assert not list(root.glob(".env.*.pending"))
    assert "retained-private-canary" not in str(failure.value) + capsys.readouterr().out


def test_existing_oversized_settings_can_be_deliberately_reduced_through_interview(
        interview, monkeypatch, capsys):
    module, root, _ = interview
    values = {f"APP_SIZE_{index}": "private-size-canary-" + "x" * (32768 - len("private-size-canary-"))
              for index in range(9)}
    declare_fields(root, values, required=False)
    for key in values:
        monkeypatch.delenv(key, raising=False)
    path = root / ".env"
    path.write_bytes(serialized(values))
    path.chmod(0o600)
    assert path.stat().st_size > MAX_BYTES
    replies = iter([":remove", ":remove", *("" for _ in range(7))])
    prompts = []

    def answer(prompt):
        prompts.append(prompt)
        return next(replies)

    monkeypatch.setattr(module.getpass, "getpass", answer)
    assert module.main() == 0
    expected = {key: value for key, value in values.items() if key not in {"APP_SIZE_0", "APP_SIZE_1"}}
    assert len(path.read_bytes()) < MAX_BYTES and path.read_bytes() == serialized(expected)
    assert private_configuration_snapshot(root)[0] == expected
    assert path.stat().st_mode & 0o777 == 0o600 and not list(root.glob(".env.*.pending"))
    output = capsys.readouterr().out + "".join(prompts)
    assert "Saved." in output and "private-size-canary" not in output
