"""Real FIFO refusal in a bounded child, without SDK or native execution."""
import hashlib
import os
from pathlib import Path
import subprocess
import sys

import pytest


CHILD = r'''
import os, pathlib, sys
sys.path.insert(0, sys.argv[1])
from launchkit.connect_installer_caller import CustodyError, ExactFile
path = pathlib.Path(sys.argv[2])
pin = ExactFile(path, sys.argv[3])
method, executable, replace = sys.argv[4], sys.argv[5] == "yes", sys.argv[6] == "yes"
if replace:
    original_open = os.open
    changed = False
    def swap_before_open(selected, flags, *args, **kwargs):
        global changed
        if not changed and pathlib.Path(selected) == path:
            changed = True
            path.unlink()
            os.mkfifo(path, 0o600)
        return original_open(selected, flags, *args, **kwargs)
    os.open = swap_before_open
print("entered", flush=True)
try:
    getattr(pin, method)(executable=executable)
except CustodyError:
    print("refused", flush=True)
else:
    raise AssertionError("A FIFO became a retained input")
'''


@pytest.mark.parametrize("method,executable", [
    ("read", False), ("read", True), ("verify", False), ("verify", True),
])
@pytest.mark.parametrize("replace", [False, True], ids=["existing-fifo", "replaced-before-open"])
def test_fifo_refused_without_waiting_for_a_writer(tmp_path, method, executable, replace):
    tmp_path.chmod(0o700)
    path = tmp_path / "retained"
    content = b"original retained bytes\n"
    if replace:
        path.write_bytes(content)
        path.chmod(0o700 if executable else 0o600)
    else:
        os.mkfifo(path, 0o600)
    argv = [sys.executable, "-I", "-S", "-c", CHILD,
        str(Path(__file__).parents[1]), str(path), hashlib.sha256(content).hexdigest(),
        method, "yes" if executable else "no", "yes" if replace else "no"]
    try:
        result = subprocess.run(argv, cwd=tmp_path, capture_output=True, timeout=2)
    except subprocess.TimeoutExpired as error:
        pytest.fail(f"Retained {method} waited for a FIFO writer: {error.stdout!r}")
    assert result.returncode == 0, result.stderr.decode()
    assert result.stdout == b"entered\nrefused\n"
