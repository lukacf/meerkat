from launchkit.guide_models import starter_state
from launchkit.guide_store import GuideStore


def test_complete_workspace_listing_includes_older_setups_without_changing_recent_menu(tmp_path):
    store = GuideStore(tmp_path / "first-user")
    oldest = store.create(starter_state({}))
    for _ in range(100):
        store.create(starter_state({}))
    assert len(store.list()) == 100
    complete = store.list(include_all=True)
    assert len(complete) == 101
    assert oldest["id"] in {setup["id"] for setup in complete}
    assert GuideStore(tmp_path / "second-user").list(include_all=True) == []
    assert all(set(setup) == {"id", "name", "stage", "status", "updated_at"} for setup in complete)
