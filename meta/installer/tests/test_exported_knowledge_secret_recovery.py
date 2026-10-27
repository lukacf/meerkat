"""Retained exported knowledge credentials cannot be silently reinitialized."""
import importlib.util
import json
from pathlib import Path
import runpy
import sys
from types import SimpleNamespace

import pytest

from launchkit.blueprint import starter_blueprint
from launchkit.compiler import generate_files
from launchkit.elephant_setup import generate_elephant_files
from launchkit.schema import DEFAULT_MODELS
from test_generated_setup import console_metadata, setup


TOKEN = "OBS_KNOWLEDGE_TOKEN"
SPACE = "OBS_KNOWLEDGE_SPACE"
PROVIDERS = {"OPENAI_API_KEY": "saved-embedding-canary", "ANTHROPIC_API_KEY": "saved-extraction-canary"}
SECRETS = {"JWT_SECRET": "retained-signing-canary", "SURREALDB_PASS": "retained-database-canary"}


@pytest.fixture
def exported(tmp_path, monkeypatch):
    knowledge = {"mode": "local", "token_env": TOKEN, "space_env": SPACE}
    for name, content in generate_elephant_files(knowledge, "observing-team", "anthropic").items():
        path = tmp_path / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content)
    spec = importlib.util.spec_from_file_location("exported_knowledge_secret_recovery", tmp_path / "knowledge/manage.py")
    manager = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(manager)

    def forbidden(*args, **kwargs):
        pytest.fail("No Docker, source checkout, provider, native service or real child may run.")

    for name in ("local_preflight", "source_checkout", "compose", "mcp_operation", "provision", "space_status"):
        monkeypatch.setattr(manager, name, forbidden)
    monkeypatch.setattr(manager.subprocess, "run", forbidden)
    monkeypatch.setattr(manager.subprocess, "Popen", forbidden)
    monkeypatch.setattr(manager.getpass, "getpass", lambda *args: pytest.fail("Saved provider keys need no prompt."))
    manager.write_env(tmp_path / ".env", PROVIDERS, compose=False)
    for name in ("data/history.bin", ".mobkit/state/history.bin"):
        path = tmp_path / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(b"Synthetic retained private history.\n")
        path.chmod(0o600)
    return manager, tmp_path


def terminal(manager, monkeypatch, command):
    monkeypatch.setattr(sys, "argv", [str(manager.ROOT / "manage.py"), command])
    return manager.main()


def snapshot(root):
    return {path: (path.read_bytes(), path.stat().st_ino) for path in root.rglob("*") if path.is_file()}


def unchanged(before):
    assert all(path.read_bytes() == content and path.stat().st_ino == inode
        for path, (content, inode) in before.items())


def forbid_credential_changes(manager, monkeypatch):
    monkeypatch.setattr(manager.secrets, "token_urlsafe", lambda *args: pytest.fail("Refusal must precede random generation."))
    monkeypatch.setattr(manager, "write_env", lambda *args, **kwargs: pytest.fail("Refusal must precede private writes."))


def test_setup_with_expired_retained_project_token_and_missing_private_file_refuses(exported, monkeypatch, capsys):
    manager, root = exported
    # Expiration is deliberately irrelevant: the selected retained value is
    # prior preparation evidence, not a token-authority assertion.
    expired = manager.jwt_token(SECRETS["JWT_SECRET"], "observing-team", now=0)
    manager.write_env(root / ".env", {**PROVIDERS, TOKEN: expired, SPACE: "observing-team"}, compose=False)
    before = snapshot(root)
    monkeypatch.setattr(sys, "argv", [str(manager.ROOT / "manage.py"), "setup"])
    with pytest.raises(SystemExit) as caught:
        runpy.run_path(str(manager.ROOT / "manage.py"), run_name="__main__")
    assert caught.value.code == 1
    assert not (manager.ROOT / ".env").exists()
    unchanged(before)
    output = capsys.readouterr()
    assert "Restore the original private knowledge/.env" in output.err
    assert expired not in output.out + output.err


@pytest.mark.parametrize("lost", ["JWT_SECRET", "SURREALDB_PASS", "empty-signing", "empty-database"])
def test_setup_retained_partial_credentials_refuses_before_random_prompt_or_write(exported, monkeypatch, lost):
    manager, root = exported
    values = {**PROVIDERS, **SECRETS, TOKEN: "retained-agent-token-canary", SPACE: "observing-team"}
    if lost.startswith("empty-"):
        values["JWT_SECRET" if lost == "empty-signing" else "SURREALDB_PASS"] = ""
    else:
        values.pop(lost)
    manager.write_env(manager.ROOT / ".env", values)
    before = snapshot(root)
    forbid_credential_changes(manager, monkeypatch)
    with pytest.raises(ValueError, match="Restore the original private knowledge/.env") as caught:
        terminal(manager, monkeypatch, "setup")
    unchanged(before)
    assert all(value not in str(caught.value) for value in [*PROVIDERS.values(), *SECRETS.values()])


@pytest.mark.parametrize("evidence", ["local-operator", "project-space", "source"])
def test_prior_preparation_evidence_preserves_missing_secrets(exported, monkeypatch, evidence):
    manager, root = exported
    if evidence == "local-operator":
        manager.write_env(manager.ROOT / ".env", {**PROVIDERS, "ELEPHANT_OPERATOR_TOKEN": "retained-operator-canary"})
    elif evidence == "project-space":
        manager.write_env(root / ".env", {**PROVIDERS, SPACE: "observing-team"}, compose=False)
    else:
        (manager.ROOT / ".source").mkdir()
    before = snapshot(root)
    forbid_credential_changes(manager, monkeypatch)
    with pytest.raises(ValueError, match="Restore the original private knowledge/.env"):
        terminal(manager, monkeypatch, "setup")
    unchanged(before)
    if evidence == "source":
        assert (manager.ROOT / ".source").is_dir()


@pytest.mark.parametrize("lost", ["JWT_SECRET", "SURREALDB_PASS", "both"])
def test_renew_never_recreates_missing_original_secret(exported, monkeypatch, lost):
    manager, root = exported
    values = {**PROVIDERS, **SECRETS}
    if lost == "both":
        values = dict(PROVIDERS)
    else:
        values.pop(lost)
    manager.write_env(manager.ROOT / ".env", values)
    before = snapshot(root)
    forbid_credential_changes(manager, monkeypatch)
    with pytest.raises(ValueError, match="Restore the original private knowledge/.env"):
        terminal(manager, monkeypatch, "renew")
    unchanged(before)


@pytest.mark.parametrize("local_file", [False, True])
def test_up_refuses_secret_loss_before_any_service_inspection(exported, monkeypatch, local_file):
    manager, root = exported
    if local_file:
        manager.write_env(manager.ROOT / ".env", {**PROVIDERS, "JWT_SECRET": SECRETS["JWT_SECRET"], TOKEN: "retained-agent-token"})
    else:
        manager.write_env(root / ".env", {**PROVIDERS, TOKEN: "retained-agent-token"}, compose=False)
    before = snapshot(root)
    forbid_credential_changes(manager, monkeypatch)
    with pytest.raises(ValueError, match="Restore the original private knowledge/.env"):
        terminal(manager, monkeypatch, "up")
    unchanged(before)


@pytest.mark.parametrize("command", ["setup", "renew", "up"])
def test_restored_original_secrets_allow_existing_terminal_action(exported, monkeypatch, command, capsys):
    manager, root = exported
    manager.write_env(manager.ROOT / ".env", {**PROVIDERS, **SECRETS, TOKEN: "retained-agent-token", SPACE: "observing-team"})
    (manager.ROOT / ".source").mkdir()
    state = snapshot(root / "data") | snapshot(root / ".mobkit")
    operations = []
    if command == "up":
        monkeypatch.setattr(manager, "local_preflight", lambda: operations.append("preflight"))
        monkeypatch.setattr(manager, "source_checkout", lambda selected: operations.append("source"))
        monkeypatch.setattr(manager, "compose", lambda *args: operations.append(args))
        monkeypatch.setattr(manager, "provision", lambda selected, values: operations.append("provision") or {})
        monkeypatch.setattr(manager, "space_status", lambda selected, wait=0: operations.append("status") or {})
        async def verify(selected, operation):
            operations.append(operation)
            return {"status": "inert-verified"}
        monkeypatch.setattr(manager, "mcp_operation", verify)
    terminal(manager, monkeypatch, command)
    local = manager.read_env(manager.ROOT / ".env")
    assert all(local[key] == value for key, value in SECRETS.items())
    unchanged(state)
    if command == "up":
        assert operations == ["preflight", "source", ("up", "--build", "--detach", "--wait", "--wait-timeout", "180"),
                              "provision", "status", "verify"]
    else:
        assert operations == []
        assert local[TOKEN] != local["ELEPHANT_OPERATOR_TOKEN"]
    output = capsys.readouterr().out
    assert all(value not in output for value in [*PROVIDERS.values(), *SECRETS.values()])


@pytest.mark.parametrize("blank_secrets", [False, True])
def test_provider_only_local_credentials_remain_genuine_first_setup(exported, monkeypatch, capsys, blank_secrets):
    manager, root = exported
    values = {**PROVIDERS, "LOCAL_PRIVATE_EXTRA": "retained-local-canary"}
    if blank_secrets:
        values.update(JWT_SECRET="", SURREALDB_PASS="")
    manager.write_env(manager.ROOT / ".env", values)
    manager.write_env(root / ".env", {TOKEN: "", SPACE: "", "HOST_EXTRA": "retained-host-canary"}, compose=False)
    terminal(manager, monkeypatch, "setup")
    local = manager.read_env(manager.ROOT / ".env")
    assert local["JWT_SECRET"] and local["SURREALDB_PASS"] and local["JWT_SECRET"] != local["SURREALDB_PASS"]
    assert local["LOCAL_PRIVATE_EXTRA"] == "retained-local-canary"
    assert manager.read_env(root / ".env")["HOST_EXTRA"] == "retained-host-canary"
    assert local[TOKEN] != local["ELEPHANT_OPERATOR_TOKEN"]
    output = capsys.readouterr().out
    assert all(value not in output for value in local.values())


@pytest.mark.parametrize("local_file", [False, True])
def test_first_up_initializes_missing_fresh_secrets_before_service_entry(exported, monkeypatch, local_file):
    manager, root = exported
    manager.write_env(root / ".env", {**PROVIDERS, TOKEN: "", SPACE: ""}, compose=False)
    if local_file:
        manager.write_env(manager.ROOT / ".env", {**PROVIDERS, "JWT_SECRET": "", "SURREALDB_PASS": ""})
    operations = []
    original_setup = manager.setup
    monkeypatch.setattr(manager, "local_preflight", lambda: operations.append("preflight"))
    def initialize(selected, renew=False):
        operations.append("setup")
        original_setup(selected, renew=renew)
    monkeypatch.setattr(manager, "setup", initialize)
    def source(selected):
        values = manager.read_env(manager.ROOT / ".env")
        assert values["JWT_SECRET"] and values["SURREALDB_PASS"]
        assert values[TOKEN] != values["ELEPHANT_OPERATOR_TOKEN"]
        operations.append("source")
    monkeypatch.setattr(manager, "source_checkout", source)
    monkeypatch.setattr(manager, "compose", lambda *args: operations.append("up"))
    monkeypatch.setattr(manager, "provision", lambda selected, values: operations.append("provision") or {})
    monkeypatch.setattr(manager, "space_status", lambda selected, wait=0: operations.append("status") or {})
    async def verify(selected, operation):
        assert operation == "verify"
        operations.append("verify")
        return {"status": "inert-verified"}
    monkeypatch.setattr(manager, "mcp_operation", verify)
    terminal(manager, monkeypatch, "up")
    assert operations == ["preflight", "setup", "source", "up", "provision", "status", "verify"]
    assert {key: manager.read_env(manager.ROOT / ".env")[key] for key in PROVIDERS} == PROVIDERS


def test_generated_terminal_first_setup_with_blank_exported_token_fields_creates_credentials(setup, monkeypatch, capsys):
    interview, root, _ = setup
    data = starter_blueprint().model_dump()
    data.update(provider="anthropic", model=DEFAULT_MODELS["anthropic"])
    data["knowledge"].update(mode="local", token_env=TOKEN, space_env=SPACE)
    for name, content in generate_files(data).items():
        path = root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content)
    console_metadata(root)
    spec = importlib.util.spec_from_file_location("terminal_first_knowledge_recovery", root / "knowledge/manage.py")
    manager = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(manager)
    assert TOKEN + "=" in (root / ".env.example").read_text()
    values = {**PROVIDERS, TOKEN: "", SPACE: "", "CONSOLE_USER_ID": "owner@example.test",
        "CONSOLE_LOGIN_PASSWORD": "retained-private-password", "UNRELATED_VALUE": "retained-host-value"}
    manager.write_env(root / ".env", values, compose=False)
    assert not (manager.ROOT / ".env").exists() and not (manager.ROOT / ".source").exists()
    monkeypatch.setattr(interview.getpass, "getpass", lambda *args: "")
    monkeypatch.setattr("builtins.input", lambda *args: "")
    handoffs = []
    def handoff(arguments, *, check):
        assert arguments == [sys.executable, str(manager.ROOT / "manage.py"), "setup"] and check is True
        handoffs.append(arguments)
        terminal(manager, monkeypatch, "setup")
        return SimpleNamespace(returncode=0)
    monkeypatch.setattr(interview.subprocess, "run", handoff)
    monkeypatch.setattr(manager.subprocess, "Popen", lambda *args, **kwargs: pytest.fail("No real child may run."))
    assert interview.main() == 0 and len(handoffs) == 1
    project = manager.read_env(root / ".env")
    local = manager.read_env(manager.ROOT / ".env")
    assert {key: project[key] for key in values if key not in {TOKEN, SPACE}} == {key: value for key, value in values.items() if key not in {TOKEN, SPACE}}
    assert project[TOKEN] == local[TOKEN] != local["ELEPHANT_OPERATOR_TOKEN"]
    assert project[SPACE] == local[SPACE] == data["project"]["slug"]
    assert local["JWT_SECRET"] and local["SURREALDB_PASS"] and local["JWT_SECRET"] != local["SURREALDB_PASS"]
    assert not {"JWT_SECRET", "SURREALDB_PASS", "ELEPHANT_OPERATOR_TOKEN"}.intersection(project)
    assert all(path.stat().st_mode & 0o777 == 0o600 for path in (root / ".env", manager.ROOT / ".env"))
    output = capsys.readouterr().out
    assert all(value not in output for value in [*PROVIDERS.values(), local["JWT_SECRET"], local["SURREALDB_PASS"], local[TOKEN], local["ELEPHANT_OPERATOR_TOKEN"]])
