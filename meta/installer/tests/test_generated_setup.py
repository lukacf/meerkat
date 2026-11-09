"""The generated terminal interview saves usable private settings without editing files."""
import builtins
import importlib.util
import json
import os
from pathlib import Path
import sys
from types import SimpleNamespace

import pytest

from launchkit.blueprint import starter_blueprint
from launchkit.compiler import generate_files
from launchkit.templates_v2.console_configuration import CONSOLE_FIELDS


@pytest.fixture
def setup(tmp_path, monkeypatch):
    for name, content in generate_files(starter_blueprint()).items():
        target = tmp_path / name
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(content)
    for name in ("mcp_connections", "application_configuration", "console_configuration", "connectors", "console_auth", "native_definition_continuity", "setup"):
        spec = importlib.util.spec_from_file_location(name, tmp_path / (name + ".py"))
        module = importlib.util.module_from_spec(spec)
        monkeypatch.setitem(sys.modules, name, module)
        spec.loader.exec_module(module)
    module = sys.modules["setup"]
    spec = json.loads((tmp_path / "deployment.json").read_text())
    spec["environment"] = []
    spec["knowledge"]["mode"] = "none"
    (tmp_path / "deployment.json").write_text(json.dumps(spec))
    monkeypatch.setattr(module.sys, "stdin", SimpleNamespace(isatty=lambda: True))
    return module, tmp_path, spec


def fields(root, spec, rows):
    spec["environment"] = rows
    (root / "deployment.json").write_text(json.dumps(spec))


def private(root, text):
    path = root / ".env"
    path.write_text(text)
    path.chmod(0o600)
    return path


def test_enter_keeps_stored_value_instead_of_source_default_or_different_inherited_value(setup, monkeypatch, capsys):
    module, root, spec = setup
    fields(root, spec, [{"key": "PROJECT_NAME", "label": "Project name", "secret": False,
                         "required": True, "default": "source default"}])
    private(root, 'PROJECT_NAME="chosen project"\nUNRELATED_VALUE="keep me"\n')
    monkeypatch.setenv("PROJECT_NAME", "different inherited value")
    prompts = []
    monkeypatch.setattr(builtins, "input", lambda prompt: prompts.append(prompt) or "")
    assert module.main() == 0
    saved, _ = module.private_values(root / ".env")
    assert saved == {"PROJECT_NAME": "chosen project", "UNRELATED_VALUE": "keep me"}
    assert "Enter keeps value" in prompts[0]
    assert "source default" not in prompts[0] and "different inherited value" not in prompts[0]
    assert "Saved." in capsys.readouterr().out


def test_required_provider_from_destination_environment_is_saved_without_echo(setup, monkeypatch, capsys):
    module, root, spec = setup
    fields(root, spec, [{"key": "OPENAI_API_KEY", "label": "Provider key", "secret": True, "required": True}])
    monkeypatch.setenv("OPENAI_API_KEY", "destination-provider-canary")
    prompts = []
    monkeypatch.setattr(module.getpass, "getpass", lambda prompt: prompts.append(prompt) or "")
    assert module.main() == 0
    saved, _ = module.private_values(root / ".env")
    assert saved["OPENAI_API_KEY"] == "destination-provider-canary"
    assert "Enter keeps value" in prompts[0]
    assert "destination-provider-canary" not in capsys.readouterr().out + "".join(prompts)
    assert (root / ".env").stat().st_mode & 0o777 == 0o600


def test_explicit_optional_removal_stays_absent_on_later_enter_despite_default_and_environment(setup, monkeypatch):
    module, root, spec = setup
    fields(root, spec, [
        {"key": "OPTIONAL_TOKEN", "label": "Optional token", "secret": True, "required": False},
        {"key": "DISPLAY_THEME", "label": "Theme", "secret": False, "required": False, "default": "source-theme"},
    ])
    private(root, 'OPTIONAL_TOKEN="saved-canary"\nDISPLAY_THEME="chosen-theme"\nOTHER="preserve"\n')
    monkeypatch.setenv("OPTIONAL_TOKEN", "inherited-canary")
    monkeypatch.setenv("DISPLAY_THEME", "inherited-theme")
    monkeypatch.setattr(module.getpass, "getpass", lambda prompt: ":remove")
    monkeypatch.setattr(builtins, "input", lambda prompt: ":remove")
    assert module.main() == 0
    assert module.private_values(root / ".env")[0] == {"OTHER": "preserve"}
    prompts = []
    monkeypatch.setattr(module.getpass, "getpass", lambda prompt: prompts.append(prompt) or "")
    monkeypatch.setattr(builtins, "input", lambda prompt: prompts.append(prompt) or "")
    assert module.main() == 0
    assert module.private_values(root / ".env")[0] == {"OTHER": "preserve"}
    assert all("Enter leaves unset" in prompt for prompt in prompts)


def test_required_removal_is_rejected_even_when_default_or_inherited_value_exists(setup, monkeypatch, capsys):
    module, root, spec = setup
    fields(root, spec, [{"key": "PROJECT_NAME", "label": "Project", "secret": False,
                         "required": True, "default": "source-default"}])
    private(root, 'PROJECT_NAME="chosen"\nOTHER="preserve"\n')
    monkeypatch.setenv("PROJECT_NAME", "inherited")
    answers = iter([":remove", ""])
    monkeypatch.setattr(builtins, "input", lambda prompt: next(answers))
    assert module.main() == 0
    assert module.private_values(root / ".env")[0] == {"PROJECT_NAME": "chosen", "OTHER": "preserve"}
    assert "cannot be removed" in capsys.readouterr().out


def test_launch_uses_interview_choices_over_old_exported_values_and_keeps_absent_environment(setup, monkeypatch, capsys):
    module, root, spec = setup
    fields(root, spec, [
        {"key": "PROJECT_NAME", "label": "Project", "secret": False, "required": True},
        {"key": "OPENAI_API_KEY", "label": "Provider key", "secret": True, "required": True},
    ])
    private(root, 'PROJECT_NAME="stored project"\nOPENAI_API_KEY="stored-private-canary"\n')
    monkeypatch.setenv("PROJECT_NAME", "old terminal project")
    monkeypatch.setenv("OPENAI_API_KEY", "old-terminal-private-canary")
    monkeypatch.setenv("SETUP_TEST_INHERITED", "destination-only-value")
    prompts = []
    monkeypatch.setattr(builtins, "input", lambda prompt: prompts.append(prompt) or "")
    monkeypatch.setattr(module.getpass, "getpass", lambda prompt: prompts.append(prompt) or "new-private-canary")
    assert module.main() == 0
    definition = importlib.util.spec_from_file_location("generated_setup_host", root / "app.py")
    host = importlib.util.module_from_spec(definition)
    definition.loader.exec_module(host)
    monkeypatch.chdir(root)
    monkeypatch.setattr(sys, "argv", ["app.py", "--doctor"])
    def check_destination(configuration):
        assert os.environ["PROJECT_NAME"] == "stored project"
        assert os.environ["OPENAI_API_KEY"] == "new-private-canary"
        assert os.environ["SETUP_TEST_INHERITED"] == "destination-only-value"
        return True
    monkeypatch.setattr(host, "doctor", check_destination)
    with pytest.raises(SystemExit) as exit_status:
        host.main()
    assert exit_status.value.code == 0
    assert os.environ["PROJECT_NAME"] == "stored project"
    assert os.environ["OPENAI_API_KEY"] == "new-private-canary"
    assert os.environ["SETUP_TEST_INHERITED"] == "destination-only-value"
    output = capsys.readouterr().out + "".join(prompts)
    assert all(value not in output for value in (
        "stored-private-canary", "old-terminal-private-canary", "new-private-canary"))


def test_default_legacy_loader_preserves_explicit_environment_values(setup, monkeypatch):
    module, root, spec = setup
    private(root, 'SETUP_TEST_EXISTING="saved value"\nSETUP_TEST_MISSING="saved fallback"\n')
    monkeypatch.setenv("SETUP_TEST_EXISTING", "explicit destination value")
    monkeypatch.delenv("SETUP_TEST_MISSING", raising=False)
    sys.modules["connectors"].load_environment(root / ".env")
    assert os.environ["SETUP_TEST_EXISTING"] == "explicit destination value"
    assert os.environ["SETUP_TEST_MISSING"] == "saved fallback"


def test_missing_required_input_reprompts_and_invalid_values_are_not_saved(setup, monkeypatch, capsys):
    module, root, spec = setup
    fields(root, spec, [{"key": "SETUP_TEST_TOKEN", "label": "Required private token", "secret": True, "required": True}])
    monkeypatch.delenv("SETUP_TEST_TOKEN", raising=False)
    answers = iter(["", "invalid\x00private", "x" * 32769, "accepted-private-canary"])
    seen = []
    def answer(prompt):
        assert not (root / ".env").exists()
        seen.append(prompt)
        return next(answers)
    monkeypatch.setattr(module.getpass, "getpass", answer)
    assert module.main() == 0
    assert len(seen) == 4
    saved, _ = module.private_values(root / ".env")
    assert saved["SETUP_TEST_TOKEN"] == "accepted-private-canary"
    output = capsys.readouterr().out
    assert "Required private token (SETUP_TEST_TOKEN): Enter a required value" in output
    assert "accepted-private-canary" not in output and "invalid\x00private" not in output


def test_application_constraints_reprompt_without_writes_and_preserve_secret_exactly(setup, monkeypatch, capsys):
    module, root, spec = setup
    application = root / "application"
    application.mkdir()
    (application / "__init__.py").write_text("def build(context):\n    return object()\n")
    (application / "configuration.json").write_text(json.dumps({"fields": [
        {"key": "SETUP_PASSWORD", "label": "Staff password", "secret": True, "required": True,
         "min_length": 12, "max_length": 80},
        {"key": "SETUP_PORT", "label": "Staff listener", "secret": False, "required": True,
         "type": "integer", "minimum": 1024, "maximum": 65535},
    ]}))
    monkeypatch.delenv("SETUP_PASSWORD", raising=False)
    monkeypatch.delenv("SETUP_PORT", raising=False)
    exact = " \taccepted private password\t "
    passwords = iter(["   ", "tiny-marker", exact])
    ports = iter(["65536", "4111"])
    def password(prompt):
        assert not (root / ".env").exists()
        return next(passwords)
    def port(prompt):
        assert not (root / ".env").exists()
        return next(ports)
    monkeypatch.setattr(module.getpass, "getpass", password)
    monkeypatch.setattr(builtins, "input", port)
    assert module.main() == 0
    saved, _ = module.private_values(root / ".env")
    assert saved == {"SETUP_PASSWORD": exact, "SETUP_PORT": "4111"}
    output = capsys.readouterr().out
    assert "Use at least 12 characters" in output and "between 1024 and 65535" in output
    assert exact not in output and "tiny-marker" not in output

    definition = importlib.util.spec_from_file_location("constrained_setup_host", root / "app.py")
    host = importlib.util.module_from_spec(definition)
    definition.loader.exec_module(host)
    (root / ".bin").mkdir()
    for name in ("rpc_gateway", "mobkit_gateway"):
        (root / ".bin" / name).touch()
    monkeypatch.setenv("SETUP_PASSWORD", "tiny-marker")
    monkeypatch.setenv("SETUP_PORT", "65536")
    assert host.doctor(spec) is False
    output = capsys.readouterr().out
    assert "Staff password (SETUP_PASSWORD): Use at least 12 characters" in output
    assert "Staff listener (SETUP_PORT): Enter a whole number" in output
    assert "tiny-marker" not in output
    monkeypatch.setenv("SETUP_PASSWORD", exact)
    monkeypatch.setenv("SETUP_PORT", "4111")
    assert host.doctor(spec) is True
    assert exact not in capsys.readouterr().out


def test_invalid_application_default_cannot_publish_terminal_configuration(setup, monkeypatch):
    module, root, spec = setup
    application = root / "application"
    application.mkdir()
    (application / "configuration.json").write_text(json.dumps({"fields": [
        {"key": "SETUP_PORT", "label": "Staff listener", "secret": False, "required": False,
         "type": "integer", "minimum": 1024, "maximum": 65535, "default": "70000"},
    ]}))
    monkeypatch.setattr(builtins, "input", lambda prompt: pytest.fail("Invalid source should fail before questions."))
    with pytest.raises(ValueError, match="whole number"):
        module.main()
    assert not (root / ".env").exists()


def console_metadata(root):
    application = root / "application"
    application.mkdir(exist_ok=True)
    (application / "__init__.py").write_text("def build(context):\n    return object()\n")
    (application / "console.json").write_text('{"authentication":"local-password"}')
    (application / "configuration.json").write_text(json.dumps({"fields": list(CONSOLE_FIELDS)}))


def test_declared_console_setup_reprompts_without_scalar_limits_and_preserves_exact_password(setup, monkeypatch, capsys):
    module, root, spec = setup
    console_metadata(root)
    identities = iter(["invalid identity-marker", "owner@example.test"])
    passwords = iter(["tiny-marker", " \tvalid private password\t "])
    def answer(prompt, source):
        assert not (root / ".env").exists()
        return next(source)
    monkeypatch.setattr(builtins, "input", lambda prompt: answer(prompt, identities))
    monkeypatch.setattr(module.getpass, "getpass", lambda prompt: answer(prompt, passwords))
    assert module.main() == 0
    saved, _ = module.private_values(root / ".env")
    assert saved == {"CONSOLE_USER_ID": "owner@example.test", "CONSOLE_LOGIN_PASSWORD": " \tvalid private password\t "}
    output = capsys.readouterr().out
    assert "CONSOLE_USER_ID" in output and "CONSOLE_LOGIN_PASSWORD" in output
    assert "1-200 ASCII" in output and "12-4096 characters" in output
    assert "tiny-marker" not in output and "identity-marker" not in output and "valid private password" not in output


@pytest.mark.parametrize("password", ["private apostrophe's passphrase", r"private\backslash\passphrase", "private\nmultiline passphrase"])
def test_local_knowledge_setup_preserves_host_private_values_outside_compose_format(setup, monkeypatch, capsys, password):
    from launchkit.elephant_setup import generate_elephant_files

    module, root, spec = setup
    spec["knowledge"]["mode"] = "local"
    fields(root, spec, [{"key": "OPENAI_API_KEY", "label": "Provider key", "secret": True, "required": True}])
    console_metadata(root)
    application = json.loads((root / "application/configuration.json").read_text())
    application["fields"].append({"key": "APP_PASSWORD", "label": "Application password", "secret": True, "required": True})
    (root / "application/configuration.json").write_text(json.dumps(application))
    for name, content in generate_elephant_files(spec["knowledge"], spec["slug"]).items():
        path = root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content)
    definition = importlib.util.spec_from_file_location("generated_private_elephant_setup", root / "knowledge/manage.py")
    manager = importlib.util.module_from_spec(definition)
    definition.loader.exec_module(manager)
    preserved = {"OPENAI_API_KEY": "synthetic-${NO_INTERPOLATION}-key", "CONSOLE_USER_ID": "owner@example.test",
                 "CONSOLE_LOGIN_PASSWORD": password, "APP_PASSWORD": "app:" + password,
                 "UNRELATED_VALUE": "preserve 'quotes' \\ and\nnewlines exactly"}
    private(root, "".join(f"{key}={json.dumps(value)}\n" for key, value in preserved.items()))
    monkeypatch.setattr(builtins, "input", lambda _: "")
    monkeypatch.setattr(module.getpass, "getpass", lambda _: "")
    entered = []

    def local_setup(argv, *, check):
        assert argv == [sys.executable, str(root / "knowledge/manage.py"), "setup"] and check is True
        entered.append(argv)
        # Enter the actual exported serialization helper in process, without
        # a child process, Docker, source checkout, provider or runtime work.
        manager.setup(manager.load_settings())
        return SimpleNamespace(returncode=0)

    monkeypatch.setattr(module.subprocess, "run", local_setup)
    assert module.main() == 0 and len(entered) == 1
    project, _ = module.private_values(root / ".env")
    local = manager.read_env(root / "knowledge/.env")
    assert {key: project[key] for key in preserved} == preserved
    assert project[spec["knowledge"]["token_env"]] == local[spec["knowledge"]["token_env"]]
    assert project[spec["knowledge"]["space_env"]] == spec["slug"]
    assert not {"ELEPHANT_OPERATOR_TOKEN", "JWT_SECRET", "SURREALDB_PASS"} & project.keys()
    assert not {"CONSOLE_LOGIN_PASSWORD", "APP_PASSWORD", "UNRELATED_VALUE"} & local.keys()
    local_path = root / "knowledge/.env"
    original = local_path.read_bytes()
    assert "OPENAI_API_KEY='synthetic-${NO_INTERPOLATION}-key'" in original.decode()
    with pytest.raises(ValueError, match="unsupported"):
        manager.write_env(local_path, {"VALUE": password})
    assert local_path.read_bytes() == original, "Compose quoting and refusal preserve the knowledge service's file."
    for key in project:
        monkeypatch.setenv(key, "host-loader-sentinel")
    sys.modules["connectors"].load_environment(root / ".env", override=True)
    assert {key: os.environ[key] for key in preserved} == preserved
    manager.setup(manager.load_settings(), renew=True)
    renewed, _ = module.private_values(root / ".env")
    assert {key: renewed[key] for key in preserved} == preserved
    assert manager.read_env(local_path)["JWT_SECRET"] == local["JWT_SECRET"]
    assert all(path.stat().st_mode & 0o777 == 0o600 for path in [root / ".env", local_path])
    output = capsys.readouterr().out
    assert all(value not in output for value in preserved.values())


def test_console_names_without_declared_authentication_keep_unconstrained_terminal_behavior(setup, monkeypatch):
    module, root, spec = setup
    fields(root, spec, list(CONSOLE_FIELDS))
    monkeypatch.setattr(builtins, "input", lambda prompt: "custom identity with spaces")
    monkeypatch.setattr(module.getpass, "getpass", lambda prompt: "short")
    assert module.main() == 0
    assert module.private_values(root / ".env")[0] == {
        "CONSOLE_USER_ID": "custom identity with spaces", "CONSOLE_LOGIN_PASSWORD": "short",
    }


async def test_console_doctor_and_launch_validate_intrinsic_rules_before_application_work(setup, monkeypatch, capsys):
    module, root, spec = setup
    console_metadata(root)
    definition = importlib.util.spec_from_file_location("console_setup_host", root / "app.py")
    host = importlib.util.module_from_spec(definition)
    definition.loader.exec_module(host)
    (root / ".bin").mkdir()
    for name in ("rpc_gateway", "mobkit_gateway"):
        (root / ".bin" / name).touch()
    monkeypatch.setenv("CONSOLE_USER_ID", "invalid identity-marker")
    monkeypatch.setenv("CONSOLE_LOGIN_PASSWORD", "tiny-marker")
    assert host.doctor(spec) is False
    output = capsys.readouterr().out
    assert "CONSOLE_USER_ID" in output and "1-200 ASCII" in output
    assert "CONSOLE_LOGIN_PASSWORD" in output and "12-4096 characters" in output
    assert "tiny-marker" not in output and "identity-marker" not in output
    monkeypatch.setenv("CONSOLE_USER_ID", "owner@example.test")
    exact = " \tvalid private password\t "
    monkeypatch.setenv("CONSOLE_LOGIN_PASSWORD", exact)
    assert host.doctor(spec) is True
    assert exact not in capsys.readouterr().out
    monkeypatch.setenv("CONSOLE_LOGIN_PASSWORD", "tiny-marker")
    monkeypatch.setitem(sys.modules, "storage_setup", SimpleNamespace(prepare_storage=lambda *args: pytest.fail("No storage setup.")))
    monkeypatch.setattr(host, "Application", lambda *args: pytest.fail("No application construction."))
    with pytest.raises(ValueError, match="CONSOLE_LOGIN_PASSWORD") as failure:
        await host.run(spec, SimpleNamespace())
    assert "tiny-marker" not in str(failure.value)
    assert not (root / ".env").exists() and not (root / ".mobkit").exists()


@pytest.mark.parametrize("failure", [KeyboardInterrupt, EOFError])
def test_cancelled_interview_preserves_existing_configuration_exactly(setup, monkeypatch, failure):
    module, root, spec = setup
    fields(root, spec, [{"key": "PROJECT_NAME", "label": "Project", "secret": False, "required": True}])
    path = private(root, 'PROJECT_NAME="prior project"\n')
    before = path.read_bytes()
    def cancel(prompt):
        raise failure
    monkeypatch.setattr(builtins, "input", cancel)
    with pytest.raises(failure):
        module.main()
    assert path.read_bytes() == before
    assert not list(root.glob(".env.*.pending"))


def test_failed_atomic_publication_preserves_prior_values_and_removes_only_pending_file(setup, monkeypatch):
    module, root, spec = setup
    path = private(root, 'PROJECT_NAME="prior project"\n')
    before = path.read_bytes()
    def fail(source, destination):
        assert Path(source).stat().st_mode & 0o777 == 0o600
        assert path.read_bytes() == before
        raise OSError("proof publication fault")
    monkeypatch.setattr(module.os, "replace", fail)
    with pytest.raises(OSError, match="publication fault"):
        module.save_values(path, {"PROJECT_NAME": "next project"}, before)
    assert path.read_bytes() == before
    assert not list(root.glob(".env.*.pending"))


def test_concurrent_configuration_change_is_preserved_instead_of_overwritten(setup, monkeypatch):
    module, root, spec = setup
    fields(root, spec, [{"key": "PROJECT_NAME", "label": "Project", "secret": False, "required": True}])
    path = private(root, 'PROJECT_NAME="prior project"\n')
    newer = b'PROJECT_NAME="newer project"\n'
    def answer(prompt):
        path.write_bytes(newer)
        return "interview project"
    monkeypatch.setattr(builtins, "input", answer)
    with pytest.raises(ValueError, match="newer values were preserved"):
        module.main()
    assert path.read_bytes() == newer
    assert not list(root.glob(".env.*.pending"))


def test_linked_configuration_is_refused_before_interview_and_target_is_unchanged(setup, tmp_path, monkeypatch):
    module, root, spec = setup
    target = root / "other-private-settings"
    target.write_text('OTHER="preserve"\n')
    (root / ".env").symlink_to(target)
    def unexpected(prompt):
        pytest.fail("Linked configuration must be refused before input")
    monkeypatch.setattr(builtins, "input", unexpected)
    with pytest.raises(ValueError, match="link"):
        module.main()
    assert target.read_text() == 'OTHER="preserve"\n'
    assert (root / ".env").is_symlink()


def test_noninteractive_setup_does_not_report_success_or_require_file_editing(setup, monkeypatch, capsys):
    module, root, spec = setup
    monkeypatch.setattr(module.sys, "stdin", SimpleNamespace(isatty=lambda: False))
    assert module.main() == 1
    assert not (root / ".env").exists()
    output = capsys.readouterr().out
    assert ".venv/bin/python setup.py" in output
    assert "edit .env" not in output and "Saved." not in output


@pytest.mark.parametrize("payload", [b'VALUE=123\n', b'VALUE="private\x00canary"\n', b'\xffprivate-canary'])
def test_unreadable_existing_values_fail_without_echoing_their_content(setup, capsys, payload):
    module, root, spec = setup
    path = root / ".env"
    path.write_bytes(payload)
    # Unquoted dotenv numbers are strings and remain supported.
    if payload == b'VALUE=123\n':
        assert module.private_values(path)[0] == {"VALUE": "123"}
        return
    with pytest.raises(ValueError, match="could not be read|invalid value") as error:
        module.private_values(path)
    assert "private-canary" not in str(error.value)
    assert path.read_bytes() == payload
