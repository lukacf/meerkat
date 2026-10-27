"""A test session leaves its temporary directories removable."""
import os
from pathlib import Path
import subprocess
import sys

from _pytest.pathlib import rm_rf

from owner_access import lacking_owner_access, restore_owner_access

TESTS = Path(__file__).resolve().parent
# Rows that take write away from a directory with children to prove a refusal.
WITHHOLDING_ROWS = [
    ("test_application_context_parents.py", "test_unsafe_existing_parent_is_refused_without_repair[readonly-shared]"),
    ("test_application_context_parents.py", "test_unsafe_existing_parent_is_refused_without_repair[readonly-shared/data]"),
    ("test_application_context_parents.py", "test_unsafe_existing_parent_is_refused_without_repair[readonly-shared/control]"),
    ("test_managed_model_startup.py", "test_changed_model_selection_refuses_before_any_runtime[root-mode-fresh]"),
    ("test_managed_model_startup.py", "test_changed_model_selection_refuses_before_any_runtime[root-mode-restore]"),
    ("test_managed_model_startup.py", "test_original_bootstrap_model_observation_rejects_selected_custody_drift[root-mode]"),
]


def test_owner_access_reaches_every_directory_without_following_links(tmp_path):
    top, outside = tmp_path / "top", tmp_path / "outside"
    (top / "readonly/child").mkdir(parents=True)
    (top / "readonly/file").write_bytes(b"inert")
    (top / "no-search/inner").mkdir(parents=True)
    (top / "no-access").mkdir()
    outside.mkdir()
    (top / "link").symlink_to(outside, target_is_directory=True)
    (top / "readonly").chmod(0o500)
    (top / "no-search").chmod(0o600)
    (top / "no-access").chmod(0o000)
    outside.chmod(0o500)
    try:
        assert lacking_owner_access(top) == ["no-access", "no-search", "readonly"]
        restore_owner_access(top)
        assert lacking_owner_access(top) == []
        assert outside.stat().st_mode & 0o777 == 0o500
        rm_rf(top)
        assert not top.exists()
    finally:
        restore_owner_access(top)
        outside.chmod(0o700)


def test_a_session_of_the_withholding_rows_leaves_its_directories_removable(tmp_path):
    basetemp = tmp_path / "session"
    path = os.pathsep.join(filter(None, [str(TESTS.parent), os.environ.get("PYTHONPATH")]))
    try:
        run = subprocess.run(
            [sys.executable, "-m", "pytest", "-q", "-p", "no:cacheprovider", "--basetemp", str(basetemp),
             *(f"{TESTS / name}::{row}" for name, row in WITHHOLDING_ROWS)],
            cwd=TESTS.parent, env={**os.environ, "PYTHONPATH": path},
            capture_output=True, text=True, timeout=600)
        assert run.returncode == 0, run.stdout[-4000:] + run.stderr[-2000:]
        assert lacking_owner_access(basetemp) == []
        # pytest's own cleanup removes the whole session.
        rm_rf(basetemp)
        assert not basetemp.exists()
    finally:
        restore_owner_access(tmp_path)
