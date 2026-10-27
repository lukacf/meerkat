"""Selected inert gateway and fail-closed native seams for offline trial tests."""
import pytest
import meerkat_meta

from test_meta_guide_runtime import selected


@pytest.fixture(autouse=True)
def meta_trial_runtime(selected, monkeypatch):
    def forbidden(*args, **kwargs):
        pytest.fail("This offline test must replace native startup explicitly.")

    monkeypatch.setattr(meerkat_meta.Meta, "builder", forbidden)
    monkeypatch.setattr("launchkit.bootstrap.download_gateway", forbidden)
    return selected
