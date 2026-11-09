#!/usr/bin/env python3
"""Mint the released MobKit 0.8.26 persistent store fixture.

The store under ``state/`` beside this script was written by the PUBLIC
RELEASE 0.8.26 ``rpc_gateway`` (release v0.8.26 of lukacf/meerkat-meta,
archive ``mobkit-rpc-gateway-0.8.26-x86_64-unknown-linux-gnu.tar.gz``,
sha256 fdac071b0a1b3983c67e6a75fc279135aa9bff9dcae56d8caaaaa8bb9dfbafb1),
never by the build under test. This script is how it was minted, so the
capture can be reproduced; it is not run by any test.

It drives the released gateway the way the 0.8.26 SDKs do: one cold
``rpc_gateway --persistent`` process speaking JSON-RPC over stdin/stdout,
``mobkit/init`` with ``persistent_state`` and an identity-first roster whose
one durable identity this script serves through
``callback/roster_provider/roster``. No provider key is used or passed:
``runtime_options.demo_llm`` selects the gateway's deterministic offline
test client, and the environment is replaced by a minimal one (PATH, LANG,
and a scratch HOME inside the output directory, so nothing ambient is read
or written).

One boot, then a clean shutdown:

1. ``mobkit/init`` creates the mob (persistent SQLite mob storage, so the
   event log ``event_log.sqlite3`` holds the mob's events and MobKit writes
   ``event_log.sqlite3.composition.json`` beside it with
   ``created_by_mobkit``), materializes the durable identity and its member
   session, and opens agent memory and the local JSON memory ledger.
2. ``mobkit/send`` runs one turn on the identity's session.
3. ``mobkit/agent_memory/remember`` stores two agent memories for the
   identity; ``mobkit/agent_memory/recall`` reads them back.
4. ``mobkit/memory/index`` writes one assertion into the memory ledger.
5. ``mobkit/shutdown``, then EOF on stdin; the gateway must exit 0.

Usage (the output directory must not exist):

    gh release download v0.8.26 -R lukacf/meerkat-meta \\
        -p 'mobkit-rpc-gateway-0.8.26-x86_64-unknown-linux-gnu.tar.gz' \\
        -p checksums.sha256
    # verify the archive against checksums.sha256, extract it into its own
    # empty directory, then:
    python3 -I mint_released_0_8_26_store.py \\
        --gateway <dir>/rpc_gateway --out /tmp/meta-adopt-0826-mint

The minted store is ``<out>/state``; it is copied into ``state/`` here
byte for byte (``cp -a``), except the gateway peer key ``peer_key.ed25519``
(key material) and the empty ``blobs/`` directory (git cannot track it), and
``checksums.sha256`` pins every file (``sha256sum`` from this directory). The
summary printed at the end names the identity's session id and the agent
memory ids the acceptance test pins.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import queue
import sqlite3
import subprocess
import sys
import threading
import time
from pathlib import Path

MOB_ID = "adoption-fixture-planners"
IDENTITY = "learner:fixture-0001"
PROFILE = "planner"

# The mob definition the store is created for. The acceptance test boots
# the build under test with these exact bytes ("the same definition"), and
# again with the `mob_tasks` line removed. 0.8.26 accepted `tools.mob_tasks`
# (its profile tool config ignores unknown keys, so the knob never reached
# the stored definition); 0.9 warns that it ignores it. The profile carries
# no `system_prompt`: 0.8.26 ignored that key too, but 0.9 refuses it, so a
# definition with it could not be supplied unchanged.
MOB_CONFIG = """\
[mob]
id = "adoption-fixture-planners"

[profiles.planner]
model = "gpt-5.5"
external_addressable = true
runtime_mode = "turn_driven"

[profiles.planner.tools]
comms = true
mob_tasks = true
"""

TURN_PROMPT = "FIXTURE-0826-TOKEN-ALPHA: plan the first synthetic lesson."

MEMORIES = [
    {
        "title": "Fixture pacing preference",
        "body": "FIXTURE-0826-MEMORY-ONE: the synthetic learner prefers short lessons.",
        "tags": ["fixture", "pacing"],
    },
    {
        "title": "Fixture topic progress",
        "body": "FIXTURE-0826-MEMORY-TWO: the synthetic learner finished unit one.",
        "tags": ["fixture", "progress"],
    },
]

LEDGER_ASSERTION = {
    "entity": "fixture-curriculum",
    "topic": "unit-one",
    "store": "knowledge_graph",
    "fact": "FIXTURE-0826-LEDGER-FACT: unit one is the synthetic starting unit.",
}

INIT_TIMEOUT = 180.0
RPC_TIMEOUT = 60.0
TURN_TIMEOUT = 180.0
EXIT_TIMEOUT = 120.0


def roster_spec() -> dict:
    return {
        "identity": IDENTITY,
        "profile": PROFILE,
        "addressability": "addressable",
        "labels": {"cohort": "fixture"},
        "additional_instructions": ["Answer in one sentence."],
    }


class Gateway:
    def __init__(self, binary: Path, cwd: Path, home: Path, stderr_log: Path) -> None:
        env = {
            "PATH": os.environ.get("PATH", "/usr/bin:/bin"),
            "LANG": "C.UTF-8",
            "HOME": str(home),
            "RUST_LOG": "warn,meerkat_mobkit=info,rpc_gateway=info",
        }
        self._stderr = stderr_log.open("wb")
        self.proc = subprocess.Popen(
            [str(binary), "--persistent"],
            cwd=str(cwd),
            env=env,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=self._stderr,
        )
        self._lines: "queue.Queue[str | None]" = queue.Queue()
        self._reader = threading.Thread(target=self._read, daemon=True)
        self._reader.start()
        self._next_id = 0

    def _read(self) -> None:
        assert self.proc.stdout is not None
        for raw in self.proc.stdout:
            self._lines.put(raw.decode("utf-8", errors="replace"))
        self._lines.put(None)

    def _send(self, message: dict) -> None:
        assert self.proc.stdin is not None
        self.proc.stdin.write((json.dumps(message) + "\n").encode("utf-8"))
        self.proc.stdin.flush()

    def _answer_callback(self, message: dict) -> None:
        method = message.get("method", "")
        if "id" not in message:
            return
        if method == "callback/roster_provider/roster":
            self._send({"jsonrpc": "2.0", "id": message["id"], "result": [roster_spec()]})
            return
        if method.startswith("callback/"):
            raise RuntimeError(
                f"unexpected host callback {method}: this driver serves only the roster"
            )

    def call(self, method: str, params: dict, timeout: float = RPC_TIMEOUT) -> dict:
        self._next_id += 1
        request_id = f"mint-{self._next_id}"
        self._send({"jsonrpc": "2.0", "id": request_id, "method": method, "params": params})
        deadline = time.monotonic() + timeout
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise TimeoutError(f"no response to {method} within {timeout}s")
            try:
                line = self._lines.get(timeout=remaining)
            except queue.Empty as error:
                raise TimeoutError(f"no response to {method} within {timeout}s") from error
            if line is None:
                raise RuntimeError(f"gateway closed stdout before answering {method}")
            try:
                message = json.loads(line)
            except json.JSONDecodeError:
                continue
            if "method" in message:
                self._answer_callback(message)
                continue
            if message.get("id") != request_id:
                continue
            if "error" in message:
                raise RuntimeError(f"{method} failed: {json.dumps(message['error'])}")
            return message.get("result")

    def close_and_wait(self, timeout: float) -> int:
        assert self.proc.stdin is not None
        self.proc.stdin.close()
        try:
            return self.proc.wait(timeout=timeout)
        finally:
            self._stderr.close()


def head_message_count(continuity: Path) -> int:
    if not continuity.exists():
        return 0
    conn = sqlite3.connect(f"file:{continuity}?mode=ro", uri=True, timeout=10)
    try:
        tables = {
            row[0]
            for row in conn.execute("SELECT name FROM sqlite_master WHERE type = 'table'")
        }
        if "continuity_session_heads" not in tables:
            return 0
        row = conn.execute("SELECT MAX(message_count) FROM continuity_session_heads").fetchone()
        return int(row[0] or 0)
    finally:
        conn.close()


def wait_for_messages(continuity: Path, floor: int, timeout: float) -> int:
    deadline = time.monotonic() + timeout
    while True:
        count = head_message_count(continuity)
        if count >= floor:
            return count
        if time.monotonic() > deadline:
            raise TimeoutError(f"durable head stayed at {count} messages, wanted {floor}")
        time.sleep(0.2)


def file_listing(root: Path) -> list[dict]:
    listing = []
    for path in sorted(root.rglob("*")):
        if path.is_file():
            data = path.read_bytes()
            listing.append(
                {
                    "path": str(path.relative_to(root)),
                    "bytes": len(data),
                    "sha256": hashlib.sha256(data).hexdigest(),
                }
            )
        elif path.is_dir() and not any(path.iterdir()):
            listing.append({"path": str(path.relative_to(root)) + "/", "bytes": 0})
    return listing


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--gateway", required=True, type=Path)
    parser.add_argument("--out", required=True, type=Path)
    args = parser.parse_args()

    out: Path = args.out
    if out.exists():
        parser.error(f"{out} exists; the output directory must be new")
    state = out / "state"
    home = out / "home"
    work = out / "work"
    for directory in (state, home, work):
        directory.mkdir(parents=True)

    gateway = Gateway(args.gateway, work, home, out / "gateway.stderr.log")
    summary: dict = {"mob_id": MOB_ID, "identity": IDENTITY}
    try:
        init = gateway.call(
            "mobkit/init",
            {
                "mob_config": MOB_CONFIG,
                "persistent_state": str(state),
                "has_roster_provider": True,
                "runtime_options": {
                    "demo_llm": True,
                    "identity_bootstrap_mode": {"mode": "eager_materialize"},
                    "agent_memory": {"selection": "contextual", "max_entries": 4},
                    "memory_config": {"backend": "local_json"},
                },
            },
            INIT_TIMEOUT,
        )
        summary["contract_version"] = init.get("contract_version")

        status = gateway.call("mobkit/status_identity", {"identity": IDENTITY})
        if status.get("state") != "active":
            raise RuntimeError(f"identity did not come up active: {json.dumps(status)}")
        summary["session_id"] = status["session_id"]

        continuity = state / "continuity.sqlite3"
        before = head_message_count(continuity)
        gateway.call("mobkit/send", {"identity": IDENTITY, "content": TURN_PROMPT})
        summary["messages_after_turn"] = wait_for_messages(continuity, before + 2, TURN_TIMEOUT)

        memory_ids = []
        for memory in MEMORIES:
            record = gateway.call(
                "mobkit/agent_memory/remember", {"identity": IDENTITY, **memory}
            )
            memory_ids.append(record["memory_id"])
        summary["memory_ids"] = memory_ids
        recalled = gateway.call(
            "mobkit/agent_memory/recall",
            {"identity": IDENTITY, "selection": "always", "max_entries": 8},
        )
        recalled_ids = sorted(record["memory_id"] for record in recalled["records"])
        if recalled_ids != sorted(memory_ids):
            raise RuntimeError(f"recall returned {recalled_ids}, wanted {memory_ids}")

        indexed = gateway.call("mobkit/memory/index", LEDGER_ASSERTION)
        summary["ledger_assertion"] = indexed

        events = gateway.call("mobkit/mob_events/query", {"limit": 100})
        summary["mob_events"] = len(events.get("events", events) or [])

        summary["final_status"] = gateway.call("mobkit/status_identity", {"identity": IDENTITY})
        summary["shutdown"] = gateway.call("mobkit/shutdown", {}, EXIT_TIMEOUT)
    finally:
        exit_code = gateway.close_and_wait(EXIT_TIMEOUT)
    summary["exit_code"] = exit_code
    if exit_code != 0:
        raise RuntimeError(f"rpc_gateway exited with {exit_code}")
    summary["files"] = file_listing(state)
    json.dump(summary, sys.stdout, indent=2, sort_keys=True)
    sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
