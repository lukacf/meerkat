"""The product rename keeps saved setup state and honors both home settings."""

from launchkit import server
from launchkit.guide_store import GuideStore


def saved_setup(root):
    return GuideStore(root).create({
        "stage": "context", "status": "active", "messages": [],
        "proposal": {"project": {"name": "Existing observing club"}},
    })


def test_old_and_new_home_names_reopen_the_same_saved_setup(tmp_path, monkeypatch):
    home = tmp_path / "retained-home"
    original = saved_setup(home)
    original_bytes = (home / "guided-setups.sqlite3").read_bytes()
    monkeypatch.delenv("META_FORGE_HOME", raising=False)
    monkeypatch.setenv("MOBKIT_LAUNCH_HOME", str(home))
    assert GuideStore(server.setup_home()).get(original["id"]) == original

    monkeypatch.setenv("META_FORGE_HOME", str(home))
    monkeypatch.delenv("MOBKIT_LAUNCH_HOME")
    assert GuideStore(server.setup_home()).get(original["id"]) == original
    assert (home / "guided-setups.sqlite3").read_bytes() == original_bytes


def test_forge_home_takes_precedence_without_changing_the_old_home(tmp_path, monkeypatch):
    old = tmp_path / "old-home"
    new = tmp_path / "selected-home"
    old_setup = saved_setup(old)
    new_setup = saved_setup(new)
    old_bytes = (old / "guided-setups.sqlite3").read_bytes()
    monkeypatch.setenv("MOBKIT_LAUNCH_HOME", str(old))
    monkeypatch.setenv("META_FORGE_HOME", str(new))
    selected = GuideStore(server.setup_home())
    assert selected.get(new_setup["id"]) == new_setup
    assert selected.get(old_setup["id"]) is None
    assert (old / "guided-setups.sqlite3").read_bytes() == old_bytes


def test_empty_forge_setting_keeps_the_old_setting_and_default(tmp_path, monkeypatch):
    monkeypatch.setattr(server, "APP_ROOT", tmp_path)
    monkeypatch.setenv("META_FORGE_HOME", "")
    monkeypatch.setenv("MOBKIT_LAUNCH_HOME", str(tmp_path / "existing"))
    assert server.setup_home() == tmp_path / "existing"
    monkeypatch.delenv("MOBKIT_LAUNCH_HOME")
    assert server.setup_home() == tmp_path / ".studio"
