"""Saved provider-key corrections reach local knowledge without external work."""
import ast
import base64
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import sys
from types import SimpleNamespace

import pytest

from launchkit.blueprint import starter_blueprint
from launchkit.compiler import generate_files
from launchkit.elephant_setup import generate_elephant_files
from launchkit.schema import DEFAULT_MODELS
from test_generated_setup import console_metadata, private, setup


EXTRACTION_KEYS = {"anthropic": "ANTHROPIC_API_KEY", "gemini": "GEMINI_API_KEY"}


def load_manager(root):
    definition = importlib.util.spec_from_file_location("knowledge_key_recovery_manager", root / "knowledge/manage.py")
    manager = importlib.util.module_from_spec(definition)
    definition.loader.exec_module(manager)
    if os.environ.get("FORGE_TEST_KNOWLEDGE_KEY_BASELINE") == "1":
        source = Path(__file__).parents[1] / ".artifacts/forge-knowledge-key-recovery-20261010/manage.py.before"
        assert hashlib.sha256(source.read_bytes()).hexdigest() == "01b42737cdf1c91a7b35e426ddd6ddf94a93f68e16e177f25d8e90d4e05b8918"
        original = next(node for node in ast.parse(source.read_bytes()).body
                        if isinstance(node, ast.FunctionDef) and node.name == "setup")
        exec(compile(ast.Module(body=[original], type_ignores=[]), str(source), "exec"), manager.__dict__)
    return manager


def export_manager(root, provider="anthropic"):
    for name, content in generate_elephant_files({"mode": "local"}, "test-project", provider).items():
        target = root / name
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(content)
    return load_manager(root)


def forbid_external(manager, monkeypatch):
    def refuse(*args, **kwargs):
        pytest.fail("Credential recovery must not enter Git, Docker, a provider, MCP or a service.")
    for name in ("command", "compose", "source_checkout", "local_preflight", "mcp_operation"):
        monkeypatch.setattr(manager, name, refuse)
    monkeypatch.setattr(manager.subprocess, "Popen", refuse)
    monkeypatch.setattr(manager.subprocess, "run", refuse)


def claims(token):
    body = token.split(".")[1]
    return json.loads(base64.urlsafe_b64decode(body + "=" * (-len(body) % 4)))


@pytest.mark.parametrize("provider", ["anthropic", "gemini"])
def test_generated_interview_replaces_saved_embedding_and_extraction_keys_without_leakage(
        setup, monkeypatch, capsys, provider):
    interview, root, _ = setup
    data = starter_blueprint().model_dump()
    data.update(provider=provider, model=DEFAULT_MODELS[provider])
    data["knowledge"]["mode"] = "local"
    for name, content in generate_files(data).items():
        target = root / name
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(content)
    console_metadata(root)
    manager = load_manager(root)
    forbid_external(manager, monkeypatch)
    settings = manager.load_settings()
    definition = json.loads((root / "deployment.json").read_text())
    labels = {field["key"]: field["label"] for field in definition["environment"]}
    extraction = EXTRACTION_KEYS[provider]
    old = {"OPENAI_API_KEY": "old-embedding-private-canary", extraction: "old-extraction-private-canary"}
    new = {"OPENAI_API_KEY": "new-${LITERAL}-embedding-private-canary", extraction: "new-extraction-private-canary"}
    retained = {"CONSOLE_USER_ID": "owner@example.test", "CONSOLE_LOGIN_PASSWORD": "private apostrophe's valid passphrase",
                "UNRELATED_VALUE": "retain quotes ' and\\slashes\nand newlines exactly"}
    private(root, "".join(f"{key}={json.dumps(value)}\n" for key, value in {**old, **retained}.items()))
    for key in old:
        monkeypatch.setenv(key, "inherited-private-canary")
    monkeypatch.setattr(manager.getpass, "getpass", lambda prompt: pytest.fail("Saved first setup keys need no prompt."))
    monkeypatch.setattr(manager.time, "time", lambda: 1000)
    manager.setup(settings)
    local_before = manager.read_env(root / "knowledge/.env")
    local_before["KNOWLEDGE_PRIVATE_EXTRA"] = "keep-knowledge-private-canary"
    manager.write_env(root / "knowledge/.env", local_before)
    unchanged_paths = [root / "knowledge/.source/retained.txt", root / "data/history.bin", root / ".mobkit/state/private.bin"]
    for path in unchanged_paths:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(b"Retained inert private state, never opened as a runtime.\n")
        path.chmod(0o600)
    unchanged = {path: (path.read_bytes(), path.stat().st_ino) for path in unchanged_paths}
    prompts, handoffs = [], []

    def answer(prompt):
        prompts.append(prompt)
        for key, value in new.items():
            if prompt.startswith(labels[key] + " "):
                return value
        return ""

    monkeypatch.setattr(interview.getpass, "getpass", answer)
    monkeypatch.setattr("builtins.input", lambda prompt: prompts.append(prompt) or "")
    monkeypatch.setattr(manager.time, "time", lambda: 1001)

    def handoff(argv, *, check):
        assert argv == [sys.executable, str(root / "knowledge/manage.py"), "setup"] and check is True
        saved, _ = interview.private_values(root / ".env")
        assert all(saved[key] == value for key, value in new.items())
        handoffs.append(argv)
        manager.setup(settings)
        return SimpleNamespace(returncode=0)

    monkeypatch.setattr(interview.subprocess, "run", handoff)
    assert interview.main() == 0 and len(handoffs) == 1
    project, _ = interview.private_values(root / ".env")
    local = manager.read_env(root / "knowledge/.env")
    assert {key: local[key] for key in new} == new, "Freshly saved project keys must replace revoked knowledge copies."
    assert {key: project[key] for key in new} == new
    assert {key: project[key] for key in retained} == retained
    assert local["KNOWLEDGE_PRIVATE_EXTRA"] == local_before["KNOWLEDGE_PRIVATE_EXTRA"]
    for key in ("JWT_SECRET", "SURREALDB_PASS"):
        assert local[key] == local_before[key]
    token_key, space_key = settings["token_env"], settings["space_env"]
    assert project[token_key] == local[token_key] != local["ELEPHANT_OPERATOR_TOKEN"]
    assert project[space_key] == local[space_key] == settings["project"]
    assert "*" not in claims(local[token_key])["scopes"]
    assert claims(local["ELEPHANT_OPERATOR_TOKEN"])["scopes"] == ["*"]
    assert not {"JWT_SECRET", "SURREALDB_PASS", "ELEPHANT_OPERATOR_TOKEN", "KNOWLEDGE_PRIVATE_EXTRA"} & project.keys()
    assert not set(retained) & local.keys()
    assert all(path.read_bytes() == content and path.stat().st_ino == inode
               for path, (content, inode) in unchanged.items())
    assert all(path.stat().st_mode & 0o777 == 0o600 for path in (root / ".env", root / "knowledge/.env"))
    captured = capsys.readouterr()
    output = captured.out + captured.err + "".join(prompts)
    sensitive = [*old.values(), *new.values(), retained["CONSOLE_LOGIN_PASSWORD"], retained["UNRELATED_VALUE"],
                 "inherited-private-canary", local["JWT_SECRET"], local["SURREALDB_PASS"], local[token_key],
                 local["ELEPHANT_OPERATOR_TOKEN"], local["KNOWLEDGE_PRIVATE_EXTRA"]]
    assert all(value not in output for value in sensitive)
    assert "Saved. Run bash start.sh --doctor, then bash start.sh." in output


@pytest.mark.parametrize("provider", ["anthropic", "gemini"])
def test_token_renewal_keeps_existing_knowledge_provider_keys(tmp_path, monkeypatch, capsys, provider):
    manager = export_manager(tmp_path, provider)
    forbid_external(manager, monkeypatch)
    extraction = EXTRACTION_KEYS[provider]
    retained = {"OPENAI_API_KEY": "knowledge-embedding-canary", extraction: "knowledge-extraction-canary",
                "JWT_SECRET": "retained-signing-canary", "SURREALDB_PASS": "retained-database-canary",
                "KNOWLEDGE_PRIVATE_EXTRA": "retained-knowledge-canary"}
    project = {"OPENAI_API_KEY": "different-project-embedding-canary", extraction: "different-project-extraction-canary",
               "UNRELATED_VALUE": "retain private host\\value\nexactly"}
    manager.write_env(manager.ROOT / ".env", retained)
    manager.write_env(tmp_path / ".env", project, compose=False)
    monkeypatch.setattr(manager.getpass, "getpass", lambda prompt: pytest.fail("Renew keeps stored knowledge keys."))
    manager.setup(manager.load_settings(), renew=True)
    local = manager.read_env(manager.ROOT / ".env")
    assert {key: local[key] for key in retained} == retained
    saved_project = manager.read_env(tmp_path / ".env")
    assert {key: saved_project[key] for key in project} == project
    output = capsys.readouterr().out
    assert all(value not in output for value in [*retained.values(), *project.values()])


def test_standalone_setup_keeps_knowledge_keys_when_project_keys_are_absent(tmp_path, monkeypatch):
    manager = export_manager(tmp_path)
    forbid_external(manager, monkeypatch)
    retained = {"OPENAI_API_KEY": "standalone-embedding-canary", "ANTHROPIC_API_KEY": "standalone-extraction-canary",
                "JWT_SECRET": "retained-signing-canary", "SURREALDB_PASS": "retained-database-canary"}
    manager.write_env(manager.ROOT / ".env", retained)
    manager.write_env(tmp_path / ".env", {"UNRELATED_VALUE": "retain host value"}, compose=False)
    for key in ("OPENAI_API_KEY", "ANTHROPIC_API_KEY"):
        monkeypatch.setenv(key, "different-inherited-canary")
    monkeypatch.setattr(manager.getpass, "getpass", lambda prompt: pytest.fail("Standalone setup keeps its saved keys."))
    manager.setup(manager.load_settings())
    local = manager.read_env(manager.ROOT / ".env")
    assert {key: local[key] for key in retained} == retained
    assert manager.read_env(tmp_path / ".env")["UNRELATED_VALUE"] == "retain host value"


@pytest.mark.parametrize("renew", [False, True])
def test_missing_saved_keys_keep_environment_then_hidden_prompt_fallback(tmp_path, monkeypatch, capsys, renew):
    manager = export_manager(tmp_path)
    forbid_external(manager, monkeypatch)
    manager.write_env(manager.ROOT / ".env", {"JWT_SECRET": "retained-signing-canary", "SURREALDB_PASS": "retained-database-canary"})
    monkeypatch.setenv("OPENAI_API_KEY", "environment-embedding-canary")
    monkeypatch.delenv("ANTHROPIC_API_KEY", raising=False)
    prompts = []
    monkeypatch.setattr(manager.getpass, "getpass", lambda prompt: prompts.append(prompt) or " prompt-extraction-canary ")
    manager.setup(manager.load_settings(), renew=renew)
    local = manager.read_env(manager.ROOT / ".env")
    assert local["OPENAI_API_KEY"] == "environment-embedding-canary"
    assert local["ANTHROPIC_API_KEY"] == "prompt-extraction-canary"
    assert local["JWT_SECRET"] == "retained-signing-canary" and local["SURREALDB_PASS"] == "retained-database-canary"
    assert len(prompts) == 1 and prompts[0].startswith("ANTHROPIC_API_KEY ")
    output = capsys.readouterr().out + "".join(prompts)
    assert "environment-embedding-canary" not in output and "prompt-extraction-canary" not in output


def test_missing_required_key_refuses_without_replacing_private_files(tmp_path, monkeypatch, capsys):
    manager = export_manager(tmp_path)
    forbid_external(manager, monkeypatch)
    manager.write_env(manager.ROOT / ".env", {"JWT_SECRET": "retained-signing-canary", "SURREALDB_PASS": "retained-database-canary"})
    manager.write_env(tmp_path / ".env", {"UNRELATED_VALUE": "retain host value"}, compose=False)
    before = {path: (path.read_bytes(), path.stat().st_ino) for path in (manager.ROOT / ".env", tmp_path / ".env")}
    for key in ("OPENAI_API_KEY", "ANTHROPIC_API_KEY"):
        monkeypatch.delenv(key, raising=False)
    monkeypatch.setattr(manager.getpass, "getpass", lambda prompt: " ")
    with pytest.raises(ValueError, match="ANTHROPIC_API_KEY is required for the knowledge pipeline"):
        manager.setup(manager.load_settings())
    assert all(path.read_bytes() == content and path.stat().st_ino == inode for path, (content, inode) in before.items())
    assert not capsys.readouterr().out
