import pytest
from launchkit.projects import DraftConflict, ProjectStore, fingerprint


def test_revision_cas_and_history_preserve_previous_design(tmp_path):
    store = ProjectStore(tmp_path / "studio")
    first = {"project": {"slug": "my-team", "name": "My team"}, "agent_classes": []}
    saved = store.save(first, expected_revision=0)
    assert saved["revision"] == 1
    assert store.save(first, expected_revision=1)["revision"] == 1
    changed = {**first, "agent_classes": [{"id": "researcher"}]}
    assert store.save(changed, expected_revision=1)["revision"] == 2
    with pytest.raises(DraftConflict):
        store.save(first, expected_revision=1)
    assert store.get("my-team")["blueprint"] == changed
    assert store.get("my-team", 1)["blueprint"] == first
    assert [entry["revision"] for entry in store.history("my-team")] == [2, 1]
    assert store.list()[0]["slug"] == "my-team"
    assert (tmp_path / "studio/blueprints.sqlite3").stat().st_mode & 0o077 == 0


def test_project_identifier_cannot_be_a_path(tmp_path):
    store = ProjectStore(tmp_path)
    with pytest.raises(ValueError):
        store.get("../../other")
    assert fingerprint({"b": 2, "a": 1}) == fingerprint({"a": 1, "b": 2})
