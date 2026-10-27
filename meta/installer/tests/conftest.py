"""Session-wide support for the installer tests."""
import pytest

from owner_access import restore_owner_access


@pytest.fixture(scope="session", autouse=True)
def _host_isolation_probed_before_any_row():
    # The host's isolation probe runs a process once per test process and
    # caches its answer. Rows that forbid processes may reach it through the
    # guide, so probe before any row runs: no row depends on an earlier row,
    # whatever order or shard runs it.
    from launchkit.implementation_workspace import _sandbox_executable
    _sandbox_executable()


@pytest.fixture(scope="session", autouse=True)
def _session_directories_stay_removable(tmp_path_factory):
    # Rows that withhold write or search on a directory leave it that way, and
    # pytest could not remove such a session later.
    yield
    restore_owner_access(tmp_path_factory.getbasetemp())
