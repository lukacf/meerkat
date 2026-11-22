"""Native occurrence completion while the public builder is still pending.

Opt in with META_GATEWAY_BIN. The test uses the real scheduler, bridge,
dispatcher and durable receipts. A public transport factory holds the real
init answer until the occurrence settles; native init may already be ready.
No provider or model is used. This is distinct from a natural-startup race.
"""
from __future__ import annotations

import datetime
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time

import pytest

GATEWAY = os.environ.get("META_GATEWAY_BIN", "")
FIXTURE = Path(__file__).parent / "fixtures" / "schedule_fire_bootstrap_native_host.py"


@pytest.mark.skipif(not GATEWAY, reason="META_GATEWAY_BIN is not set to a real rpc_gateway")
# Above the sum of the row's own bounds (two phases of 100 s and a wait of at
# most 60 s for the stored occurrence), so that a slow host is stopped by a
# phase's bound and its message, not by this one.
@pytest.mark.timeout(280)
def test_stored_schedule_completes_before_public_build_returns(tmp_path, record_property):
    evidence_root = os.environ.get("META_SCHEDULE_TEST_EVIDENCE_DIR")
    if evidence_root:
        Path(evidence_root).mkdir(parents=True, exist_ok=True)
        directory = Path(tempfile.mkdtemp(prefix="python-schedule-bootstrap-", dir=evidence_root))
    else:
        directory = tmp_path
    gateway = str(Path(GATEWAY).resolve())
    assert Path(gateway).is_file(), "the configured gateway must exist"
    (directory / "home").mkdir()
    (directory / "mob.toml").write_text(
        '[mob]\nid = "schedule-bootstrap"\n[profiles.reader]\n'
        'model = "claude-haiku-5-5"\nruntime_mode = "turn_driven"\n'
        'external_addressable = true\n'
    )
    # A fresh HOME and an explicit environment keep provider credentials and
    # repository convention files out of both child processes.
    environment = {"HOME": str(directory / "home"), "PATH": os.defpath,
                   "LANG": "C.UTF-8", "PYTHONPATH": str(FIXTURE.parents[2]),
                   "META_GATEWAY_BIN": gateway, "RUST_MIN_STACK": "16777216"}
    record_property("evidence_directory", str(directory))

    def phase(mode):
        command = [sys.executable, str(FIXTURE), mode, str(directory)]
        receipt = {"command": command, "cwd": str(directory), "environment": environment,
                   "started_ns": time.time_ns(), "retries": 0}
        try:
            with (directory / f"{mode}.stdout.log").open("w") as stdout, \
                 (directory / f"{mode}.stderr.log").open("w") as stderr:
                result = subprocess.run(command, cwd=directory, env=environment,
                                        stdout=stdout, stderr=stderr, timeout=100, check=False)
            receipt["rc"] = result.returncode
        except subprocess.TimeoutExpired:
            receipt["timed_out"] = True
            raise
        finally:
            receipt["finished_ns"] = time.time_ns()
            (directory / f"{mode}.receipt.json").write_text(json.dumps(receipt, indent=2) + "\n")
        assert result.returncode == 0, f"{mode} fixture failed; inspect {directory}"
        return json.loads((directory / f"{mode}.json").read_text())

    seed = phase("seed")
    target = seed["next_occurrence"]
    assert target["phase"] == "pending"
    assert target["occurrence"]["attempt_count"] == 0
    due = datetime.datetime.fromisoformat(target["occurrence"]["due_at_utc"].replace("Z", "+00:00"))
    # Wait for the persisted deadline while no gateway is running. This is
    # schedule downtime, not a sleep inserted into startup or registration.
    while time.time() < due.timestamp():
        time.sleep(max(0.0, min(1.0, due.timestamp() - time.time())))
    restart = phase("restart")
    stored = restart["stored"]
    observed = restart["stored_while_build_pending"]
    assert seed["pid"] != restart["pid"], "restart uses a fresh Python process"
    assert restart["build_pending_at_receipt"] is True
    assert restart["receipt_observed_ns"] < restart["init_answer_released_ns"] < restart["build_returned_ns"]
    assert len(restart["fires"]) == 1
    fire = restart["fires"][0]
    assert fire["init_answer_released"] is False
    assert fire["params"]["occurrence"]["occurrence_id"] == target["occurrence"]["occurrence_id"]
    assert stored["occurrence"]["occurrence_id"] == target["occurrence"]["occurrence_id"]
    assert observed["occurrence"]["occurrence_id"] == stored["occurrence"]["occurrence_id"]
    assert observed["phase"] == stored["phase"]
    assert stored["occurrence"]["attempt_count"] == 1
    assert stored["action_at_ms"] is None
    for snapshot in (observed, stored):
        # Receipt timestamps can tie, and UUID receipt IDs do not order the
        # transitions. Follow the occurrence's authoritative last receipt.
        last_id = snapshot["occurrence"]["last_receipt"]["receipt_id"]
        matching = [receipt for receipt in snapshot["receipts"]
                    if receipt["receipt_id"] == last_id]
        assert len(matching) == 1
        assert matching[0]["stage"] == snapshot["phase"]
    # Same intended outcome on released and fixed SDKs. The fallback reaches
    # a real delivery_failed/runtime_rejected receipt on released behavior.
    # Missing builder API, import failure or fixture timeout is not this red.
    assert stored["phase"] == "completed", json.dumps({
        "strategy": restart["registration_strategy"], "fire": fire,
        "stored": stored, "evidence_directory": str(directory),
    }, indent=2)
    assert len(restart["callbacks_while_build_pending"]) == 1
    assert restart["callbacks_while_build_pending"][0]["occurrence"]["occurrence_id"] == target["occurrence"]["occurrence_id"]
