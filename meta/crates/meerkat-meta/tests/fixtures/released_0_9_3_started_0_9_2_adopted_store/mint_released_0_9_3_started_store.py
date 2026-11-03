#!/usr/bin/env python3
"""Mint the 0.8.26 -> 0.9.2 -> 0.9.3 lineage fixture as the supported 0.9.4 route
expects it:
a MobKit 0.8.26 store adopted and run by the PUBLISHED Meta 0.9.2, then
started ONCE by the PUBLISHED Meta 0.9.3, so the state directory under
``state/`` was last written by released 0.9.3, never by the build under test.
This script is how it was minted, so the capture can be reproduced; no test
runs it.

1. Mint a 0.8.26 store with the published MobKit 0.8.26 rpc_gateway, using
   ``../released_0_8_26_store/mint_released_0_8_26_store.py`` (demo_llm, one
   identity-first identity, one turn carrying FIXTURE-0826-TOKEN-ALPHA, two
   agent memories, one ledger assertion, clean shutdown).
2. Adopt it with the published Meta 0.9.2
   ``meta_gateway storage-adopt-predecessor --state-dir <state> --json``,
   dry run, then ``--apply`` (this rewrites the composition manifest's
   creator field from ``created_by_mobkit`` to ``created_by_meta``).
3. Boot the published Meta 0.9.2 ``rpc_gateway --persistent`` twice over the
   adopted store, under the HOME the mint used (members' session comms
   identity keys live in ``$XDG_STATE_HOME``, outside the state directory):
   ``meta/init`` with the released mob definition byte for byte, an
   identity-first roster and ``demo_llm``; ``meta/status_identity``; turns
   (2, then 1) through ``meta/send`` + ``meta/wait_for_turn``;
   ``meta/shutdown``, then EOF; the gateway must exit 0.
4. With ``--manifest predecessor``, put the creator field back to
   ``created_by_mobkit`` (the bytes 0.8.26 wrote, as an adoption without the
   manifest step leaves them); ``--manifest meta`` keeps the 0.9.2 spelling.
5. Start the published Meta 0.9.3 ``rpc_gateway --persistent`` ONCE, same
   HOME, same init and roster: ``meta/init`` (every identity active,
   none broken), ``meta/status_identity``, ``meta/shutdown``, EOF, exit 0.
   No turn. This first 0.9.3 open moves the mob ledger domain from 10 to 11.
6. With ``--keyless-check``, start published 0.9.3 once more on a throwaway
   copy under an EMPTY HOME (no comms keys) and record what meta/init and
   meta/status_identity report; the staged state is not touched by it.

The state is copied into ``state/`` byte for byte, except the gateway peer
key ``peer_key.ed25519`` (key material) and the empty ``blobs/`` directory
(git cannot track it). The members' comms identity keys are not part of the
state directory and are not committed either. ``checksums.sha256`` pins
every file; ``fixture-summary.json`` names the identity, member, session,
memories, ledger assertion, turns, mob config, roster, ledgers and binary
digests.

Usage (the output directory must not exist; the archives verified against
their releases' checksums.sha256 and extracted into their own empty
directories):

    python3 -I mint_released_0_9_3_started_store.py \
        --mobkit-gateway <dir>/rpc_gateway \
        --meta-gateway-092 <dir>/meta_gateway --rpc-gateway-092 <dir>/rpc_gateway \
        --rpc-gateway-093 <dir>/rpc_gateway --manifest predecessor \
        --keyless-check --work /tmp/meta-093-started-mint --out <fixture dir>
"""

import argparse
import hashlib
import json
import os
import queue
import shutil
import subprocess
import sys
import threading
import time

HERE = os.path.dirname(os.path.abspath(__file__))
MINT_0826 = os.path.join(HERE, "..", "released_0_8_26_store", "mint_released_0_8_26_store.py")


def child_env(work):
    home = os.path.join(work, "home")
    env = {
        "PATH": "/usr/bin:/bin",
        "HOME": home,
        "XDG_CONFIG_HOME": os.path.join(home, ".config"),
        "XDG_DATA_HOME": os.path.join(home, ".local", "share"),
        "XDG_STATE_HOME": os.path.join(home, ".local", "state"),
        "XDG_CACHE_HOME": os.path.join(home, ".cache"),
        "LANG": "C.UTF-8",
        "RUST_LOG": "info",
    }
    for key in ("XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME", "XDG_CACHE_HOME"):
        os.makedirs(env[key], exist_ok=True)
    return env


class Gateway:
    def __init__(self, binary, work, label, log_path, roster):
        self.label = label
        self.roster = roster
        self.next_id = 0
        cwd = os.path.join(work, "cwd")
        os.makedirs(cwd, exist_ok=True)
        self.stderr = open(log_path, "x", encoding="utf-8")
        self.proc = subprocess.Popen(
            [binary, "--persistent"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=self.stderr,
            cwd=cwd,
            env=child_env(work),
            text=True,
            bufsize=1,
        )
        print(f"[{label}] gateway pid {self.proc.pid}", flush=True)
        self.lines = queue.Queue()
        threading.Thread(target=self._read, daemon=True).start()

    def _read(self):
        for line in self.proc.stdout:
            self.lines.put(line)
        self.lines.put(None)

    def send(self, value):
        self.proc.stdin.write(json.dumps(value) + "\n")
        self.proc.stdin.flush()

    def answer_callback(self, message):
        method = message.get("method")
        if method is None or "id" not in message:
            return
        if method == "callback/roster_provider/roster" and self.roster is not None:
            self.send({"jsonrpc": "2.0", "id": message["id"], "result": self.roster})
            return
        print(f"[{self.label}] unexpected callback {method}", flush=True)
        self.send(
            {
                "jsonrpc": "2.0",
                "id": message["id"],
                "error": {"code": -32601, "message": f"fixture host does not provide {method}"},
            }
        )

    def request(self, method, params, timeout=120.0):
        self.next_id += 1
        rid = f"{self.label}-{self.next_id}"
        self.send({"jsonrpc": "2.0", "id": rid, "method": method, "params": params})
        deadline = time.monotonic() + timeout
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise RuntimeError(f"[{self.label}] no response to {method} within {timeout}s")
            try:
                line = self.lines.get(timeout=remaining)
            except queue.Empty:
                continue
            if line is None:
                raise RuntimeError(f"[{self.label}] gateway stdout closed during {method}")
            try:
                message = json.loads(line)
            except ValueError:
                continue
            if "method" in message:
                self.answer_callback(message)
                continue
            if message.get("id") == rid:
                return message

    def call(self, method, params, timeout=120.0):
        response = self.request(method, params, timeout)
        if response.get("error") is not None:
            raise RuntimeError(f"[{self.label}] {method} failed: {json.dumps(response)}")
        return response.get("result")

    def pump(self, seconds):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            try:
                line = self.lines.get(timeout=0.1)
            except queue.Empty:
                continue
            if line is None:
                return
            try:
                message = json.loads(line)
            except ValueError:
                continue
            if "method" in message:
                self.answer_callback(message)

    def shutdown(self):
        result = self.call("meta/shutdown", {}, timeout=180.0)
        print(f"[{self.label}] shutdown: {json.dumps(result)}", flush=True)
        self.proc.stdin.close()
        deadline = time.monotonic() + 120
        while self.proc.poll() is None:
            if time.monotonic() > deadline:
                raise RuntimeError(f"[{self.label}] gateway did not exit")
            self.pump(0.2)
        rc = self.proc.returncode
        self.stderr.close()
        print(f"[{self.label}] exit status {rc}", flush=True)
        if rc != 0:
            raise RuntimeError(f"[{self.label}] gateway exited {rc}")
        return result


def tree_digest(root):
    rows = []
    for base, dirs, files in os.walk(root):
        dirs.sort()
        for name in sorted(files):
            path = os.path.join(base, name)
            if os.path.islink(path):
                rows.append((os.path.relpath(path, root), "symlink", 0))
                continue
            with open(path, "rb") as handle:
                data = handle.read()
            rows.append((os.path.relpath(path, root), hashlib.sha256(data).hexdigest(), len(data)))
    return rows


MOB_CONFIG = """[mob]
id = "adoption-fixture-planners"

[profiles.planner]
model = "gpt-5.5"
external_addressable = true
runtime_mode = "turn_driven"

[profiles.planner.tools]
comms = true
mob_tasks = true
"""
IDENTITY = "learner:fixture-0001"
ROSTER = [
    {
        "identity": IDENTITY,
        "profile": "planner",
        "addressability": "addressable",
        "labels": {"cohort": "fixture"},
        "additional_instructions": ["Answer in one sentence."],
    }
]


def init_params(state):
    return {
        "persistent_state": state,
        "mob_config": MOB_CONFIG,
        "has_roster_provider": True,
        "runtime_options": {
            "demo_llm": True,
            "identity_bootstrap_mode": {"mode": "eager_materialize"},
            "agent_memory": {"selection": "contextual", "max_entries": 4},
            "memory_config": {"backend": "local_json"},
        },
    }


def sha256_file(path):
    with open(path, "rb") as handle:
        return hashlib.sha256(handle.read()).hexdigest()


def ledgers(state):
    """meerkat_schema rows (domain, version) of every SQLite file under state,
    read from a throwaway copy (database + -wal + -shm)."""
    import sqlite3
    import tempfile

    found = {}
    for base, dirs, files in os.walk(state):
        dirs.sort()
        for name in sorted(files):
            path = os.path.join(base, name)
            if name.endswith(("-wal", "-shm")):
                continue
            with open(path, "rb") as handle:
                if handle.read(16) != b"SQLite format 3\x00":
                    continue
            with tempfile.TemporaryDirectory() as scratch:
                copy = os.path.join(scratch, name)
                for suffix in ("", "-wal", "-shm"):
                    if os.path.exists(path + suffix):
                        shutil.copy2(path + suffix, copy + suffix)
                con = sqlite3.connect(copy)
                try:
                    rows = con.execute("SELECT domain, version FROM meerkat_schema ORDER BY domain").fetchall()
                except sqlite3.Error:
                    rows = []
                con.close()
            found[os.path.relpath(path, state)] = [list(row) for row in rows]
    return found


MANIFEST_FILE = "event_log.sqlite3.composition.json"
META_FIELD = b'"created_by_meta":'
PREDECESSOR_FIELD = b'"created_by_mobkit":'


def start_once(binary, work, home_root, state, label, log_path):
    """One rpc_gateway start: meta/init, meta/status_identity, meta/shutdown,
    EOF; returns (init result, status result, shutdown result)."""
    gateway = Gateway(binary, home_root, label, log_path, ROSTER)
    try:
        init = gateway.call("meta/init", init_params(state), timeout=240.0)
        status = gateway.call("meta/status_identity", {"identity": IDENTITY})
        shutdown = gateway.shutdown()
    except BaseException:
        if gateway.proc.poll() is None:
            gateway.proc.kill()
            gateway.proc.wait()
        raise
    return init, status, shutdown


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--mobkit-gateway", required=True, help="published MobKit 0.8.26 rpc_gateway")
    parser.add_argument("--meta-gateway-092", required=True, help="published Meta 0.9.2 meta_gateway")
    parser.add_argument("--rpc-gateway-092", required=True, help="published Meta 0.9.2 rpc_gateway")
    parser.add_argument("--rpc-gateway-093", required=True, help="published Meta 0.9.3 rpc_gateway")
    parser.add_argument("--manifest", required=True, choices=["meta", "predecessor"])
    parser.add_argument("--keyless-check", action="store_true")
    parser.add_argument("--mint-0826", default=MINT_0826, help="the 0.8.26 mint script")
    parser.add_argument("--work", required=True, help="new directory")
    parser.add_argument("--out", required=True, help="new fixture directory (state/, checksums, summary)")
    args = parser.parse_args()
    os.makedirs(args.work)
    mint_out = os.path.join(args.work, "mint-0826")
    minted = subprocess.run(
        [sys.executable, "-I", args.mint_0826, "--gateway", args.mobkit_gateway, "--out", mint_out],
        check=True,
        capture_output=True,
        text=True,
    )
    mint_summary = json.loads(minted.stdout)
    state = os.path.join(mint_out, "state")
    env = child_env(mint_out)
    for apply in (False, True):
        cmd = [args.meta_gateway_092, "storage-adopt-predecessor", "--state-dir", state, "--json"]
        if apply:
            cmd.append("--apply")
        run = subprocess.run(cmd, cwd=mint_out, env=env, capture_output=True, text=True, timeout=600)
        print(f"adopt apply={apply}: exit {run.returncode}", flush=True)
        if run.returncode != 0:
            raise SystemExit(run.stderr)
    boots = []
    for boot, turns in ((1, 2), (2, 1)):
        label = f"boot{boot}-092"
        gateway = Gateway(args.rpc_gateway_092, mint_out, label, os.path.join(args.work, f"{label}-stderr.log"), ROSTER)
        texts = []
        try:
            gateway.call("meta/init", init_params(state), timeout=240.0)
            gateway.call("meta/status_identity", {"identity": IDENTITY})
            for n in range(turns):
                text = f"boot {boot} turn {n} for {IDENTITY}"
                sent = gateway.call("meta/send", {"identity": IDENTITY, "content": text, "track_turn": True})
                gateway.call(
                    "meta/wait_for_turn",
                    {"identity": IDENTITY, "ticket": sent["turn"]["ticket"], "timeout_ms": 90000},
                    timeout=120.0,
                )
                texts.append(text)
            gateway.shutdown()
        except BaseException:
            if gateway.proc.poll() is None:
                gateway.proc.kill()
                gateway.proc.wait()
            raise
        boots.append({"boot": boot, "turns": texts})
    manifest = os.path.join(state, MANIFEST_FILE)
    with open(manifest, "rb") as handle:
        manifest_bytes = handle.read()
    if manifest_bytes.count(META_FIELD) != 1:
        raise SystemExit(f"{MANIFEST_FILE}: expected one {META_FIELD!r} after the 0.9.2 adoption")
    if args.manifest == "predecessor":
        with open(manifest, "wb") as handle:
            handle.write(manifest_bytes.replace(META_FIELD, PREDECESSOR_FIELD))
    ledgers_before = ledgers(state)
    init, status, shutdown = start_once(
        args.rpc_gateway_093, args.work, mint_out, state, "start-093", os.path.join(args.work, "start-093-stderr.log")
    )
    counts = (init.get("identity_bootstrap") or {}).get("counts") or {}
    if counts.get("broken") != 0 or counts.get("active") != 1 or status.get("state") != "active":
        raise SystemExit(f"0.9.3 start did not resume the identity: {json.dumps(init)} {json.dumps(status)}")
    ledgers_after = ledgers(state)
    with open(manifest, "rb") as handle:
        manifest_after = handle.read()
    keyless = None
    if args.keyless_check:
        scratch = os.path.join(args.work, "keyless")
        os.makedirs(scratch)
        copy = os.path.join(scratch, "state")
        shutil.copytree(state, copy, symlinks=True)
        k_init, k_status, k_shutdown = start_once(
            args.rpc_gateway_093, args.work, scratch, copy, "keyless-093", os.path.join(args.work, "keyless-093-stderr.log")
        )
        keyless = {
            "home": "empty (no session comms identity keys)",
            "identity_bootstrap": k_init.get("identity_bootstrap"),
            "status_identity": {k: k_status.get(k) for k in ("state", "session_id", "addressability", "lease_healthy")},
            "shutdown": k_shutdown,
        }
    os.makedirs(args.out)
    staged = os.path.join(args.out, "state")
    shutil.copytree(
        state, staged, symlinks=True, ignore=lambda d, names: [n for n in names if n in ("peer_key.ed25519", "blobs")]
    )
    rows = sorted(tree_digest(staged))
    with open(os.path.join(args.out, "checksums.sha256"), "x", encoding="utf-8") as handle:
        for path, digest, _size in rows:
            handle.write(f"{digest}  state/{path}\n")
    summary = {
        "fixture_id": f"meta-0.9.3-started-0.9.2-adopted-mobkit-0.8.26-store-{args.manifest}-manifest-synthetic",
        "data_classification": "synthetic_non_production",
        "lineage": [
            {"step": "mint", "product": "MobKit rpc_gateway 0.8.26 (published)",
             "binary_sha256": sha256_file(args.mobkit_gateway), "driver": os.path.basename(args.mint_0826)},
            {"step": "adopt", "product": "Meta meta_gateway 0.9.2 (published)",
             "command": "meta_gateway storage-adopt-predecessor --state-dir <state> --json --apply",
             "binary_sha256": sha256_file(args.meta_gateway_092)},
            {"step": "run", "product": "Meta rpc_gateway 0.9.2 (published)", "boots": boots,
             "clean_shutdown": True, "binary_sha256": sha256_file(args.rpc_gateway_092)},
            {"step": "manifest", "creator_field": "created_by_mobkit" if args.manifest == "predecessor" else "created_by_meta"},
            {"step": "start", "product": "Meta rpc_gateway 0.9.3 (published)", "turns": 0,
             "clean_shutdown": True, "shutdown": shutdown, "binary_sha256": sha256_file(args.rpc_gateway_093)},
        ],
        "mob_id": "adoption-fixture-planners",
        "mob_config": MOB_CONFIG,
        "identity": IDENTITY,
        "member_id": (status.get("delivery_scope") or {}).get("member", {}).get("runtime_id", {}).get("identity"),
        "session_id": status.get("session_id"),
        "session_id_at_0_8_26_mint": mint_summary.get("session_id"),
        "memory_ids": mint_summary.get("memory_ids"),
        "ledger_assertion": mint_summary.get("ledger_assertion"),
        "turn_token": "FIXTURE-0826-TOKEN-ALPHA (the 0.8.26 mint's one turn)",
        "roster": ROSTER,
        "init_params": {k: v for k, v in init_params("<state>").items() if k != "persistent_state"},
        "init_0_9_3": {"identity_bootstrap": init.get("identity_bootstrap"), "contract_version": init.get("contract_version")},
        "status_identity_0_9_3": status,
        "ledgers_before_0_9_3_start": ledgers_before,
        "ledgers_after_0_9_3_start": ledgers_after,
        "manifest_rewritten_by_0_9_3_start": manifest_after != (manifest_bytes if args.manifest == "meta"
                                                                else manifest_bytes.replace(META_FIELD, PREDECESSOR_FIELD)),
        "keyless_0_9_3_start": keyless,
        "not_committed": [
            "state/peer_key.ed25519 (gateway peer key, key material)",
            "state/blobs/ (empty directory; recreate it when staging)",
            "$XDG_STATE_HOME/meerkat/session_comms_identity/<session>/identity.{key,pub} (outside the state dir)",
        ],
    }
    with open(os.path.join(args.out, "fixture-summary.json"), "x", encoding="utf-8") as handle:
        json.dump(summary, handle, indent=1, sort_keys=False)
        handle.write("\n")
    for path, digest, size in rows:
        print("file", path, digest, size, flush=True)


if __name__ == "__main__":
    sys.exit(main())
