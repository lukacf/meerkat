#!/usr/bin/env python3
"""Mint the released Meta 0.9.3 H-app state directory fixtures.

H-app is the #306 plan's name for an application that embeds the Meta runtime
through `UnifiedRuntimeBuilder` with `persistent_state` (shape S2). The
variants beside this script were written by the PUBLISHED 0.9.3 crates from
crates.io, never by the code under test: driver/ is a standalone crate whose
lockfile resolves every meerkat crate to 0.9.3 (meerkat-meta =0.9.3), and this
script runs its binary once per variant and records what 0.9.3 wrote. No test
runs it.

driver/ keeps its manifest and lockfile as Cargo.toml.fixture and
Cargo.lock.fixture, so that nothing in this repository reads it as a crate or
a lockfile (the workspace, dependency scanners). The build steps below restore
the real names in a copy outside the repository.

The driver builds the runtime with persistent_state, comms, pinned compaction
(auto_compact_threshold 50000, recent_turn_budget 1) and a scripted
in-process model ("app reply N"; a user message carrying [compact-now] makes
the answering request report 60000 input tokens once), so nothing leaves the
machine and no provider key is read. Each run gets a scratch HOME and XDG
roots inside --work, so 0.9.3 writes nothing outside --work.

Variants (--variants, default all):
  roster          a RosterProvider (eager bootstrap) over app:app-01..06, the
                  builder's default in-memory mob storage. Boot 1: a status
                  read and a turn each; app-01 compacts; app-03 is retired and
                  woken by a turn; app-04 is reset and takes a turn;
                  app-05 is deleted (its next turn is refused). Boot 2: the
                  same roster re-adds app-05 on a fresh session, which takes no
                  turn; app-01..03 take a turn. Boot 3: the roster drops
                  app-06; app-01 takes a turn. Clean shutdown after each boot.
  roster-durable  the roster plan with persistent mob storage supplied through
                  the builder's documented option
                  (mob_storage(persistent_mob_storage(<state>/event_log.sqlite3))).
  roster-durable-mob-sqlite
                  the same, with the mob store at <state>/mob.sqlite3 (the file
                  name 0.9.3's own non-gateway call sites use).
  plain           no roster provider: over two boots the app spawns worker-1
                  and worker-2 and runs a turn on each. The mob is in-memory,
                  so boot 2 spawns them again on fresh sessions.
After the last boot the driver builds the 0.9.3 runtime once more on a copy of
the final state (the restart probe) and records the outcome. The script fails
unless the stores show each variant's shape (see check_variant).

Usage (--work and --out must not exist; build the driver first, outside the
repository):

    cp -r driver /tmp/h-app-driver && cd /tmp/h-app-driver
    mv Cargo.toml.fixture Cargo.toml && mv Cargo.lock.fixture Cargo.lock
    cargo +1.97.0 build --locked
    python3 -I mint_released_0_9_3_h_app.py \\
        --binary /tmp/h-app-driver/target/debug/s2-mint-093 \\
        --driver-dir driver --work /tmp/h-app-work --out /tmp/h-app

Outputs under --out/<variant>/:
  state/         the final state directory; a file above 900 KiB is stored as
                 <name>.gz (see PACK_ABOVE_BYTES)
  events.jsonl   every operation with its result or error
  probe.json     the restart probe's outcome
  MANIFEST.json  producer (driver file digests, the meerkat crate versions
                 from Cargo.lock.fixture, the binary digest, toolchain, compaction and
                 mob storage), statuses per boot, refusals, the probe,
                 committed sessions read from runtime.sqlite (WAL included)
                 with their sessions.sqlite3 projection and continuity record,
                 the continuity store's rows, the session comms identity files
                 0.9.3 wrote outside the state directory (listed, not stored),
                 file digests, packed files, empty directories
  MANIFEST.txt   the human-readable twin
  checksums.sha256  sha256sum lines for every stored file above
To use a variant, copy its state/ somewhere else, inflate each .gz it lists
under "packed", recreate the empty directories it lists, and never open the
files in place.
"""

import argparse
import datetime
import gzip
import hashlib
import json
import os
import re
import shutil
import sqlite3
import subprocess
import sys
import tempfile

VARIANTS = ("roster", "roster-durable", "roster-durable-mob-sqlite", "plain")
# The persistent mob store of each durable variant, inside the state directory.
MOB_STORES = {"roster-durable": "event_log.sqlite3", "roster-durable-mob-sqlite": "mob.sqlite3"}
COMPOSITION_SUFFIX = ".composition.json"
TOOLCHAIN = "1.97.0"
COMPACT_THRESHOLD = 50000
REPORTED_PROMPT_TOKENS = COMPACT_THRESHOLD + 10000
# Files above this size are stored gzip-compressed (the repository refuses
# files above 1000 KiB).
PACK_ABOVE_BYTES = 900 * 1024
ROSTER_IDENTITIES = [f"app:app-{n:02d}" for n in range(1, 7)]
DRIVER_FILES = ("Cargo.toml.fixture", "Cargo.lock.fixture", "src/main.rs", "src/llm_usage.rs")


def sha256_bytes(data):
    return hashlib.sha256(data).hexdigest()


def sha256_file(path):
    with open(path, "rb") as handle:
        return sha256_bytes(handle.read())


def messages_digest(messages):
    return sha256_bytes(json.dumps(messages, sort_keys=True).encode())


def meerkat_crates(lock_path):
    text = open(lock_path, encoding="utf-8").read()
    found = {}
    for name, version, source in re.findall(
        r'\[\[package\]\]\nname = "([^"]+)"\nversion = "([^"]+)"\n(?:source = "([^"]+)")?', text
    ):
        if name.startswith("meerkat"):
            found[name] = {"version": version, "source": source.split("+")[0] if source else "local"}
    return found


def run_variant(binary, work, variant):
    home = os.path.join(work, "home")
    for sub in (".config", ".local/share", ".local/state", ".cache", "tmp"):
        os.makedirs(os.path.join(home, sub))
    env = {
        "PATH": "/usr/bin:/bin",
        "HOME": home,
        "XDG_CONFIG_HOME": os.path.join(home, ".config"),
        "XDG_DATA_HOME": os.path.join(home, ".local/share"),
        "XDG_STATE_HOME": os.path.join(home, ".local/state"),
        "XDG_CACHE_HOME": os.path.join(home, ".cache"),
        "TMPDIR": os.path.join(home, "tmp"),
        "LANG": "C.UTF-8",
    }
    run_dir = os.path.join(work, "run")
    with open(os.path.join(work, "driver.log"), "x", encoding="utf-8") as log:
        proc = subprocess.run(
            [binary, variant, run_dir], cwd=work, env=env, stdout=log, stderr=subprocess.STDOUT, timeout=900
        )
    if proc.returncode != 0:
        raise SystemExit(f"{variant}: the driver exited {proc.returncode}; see {work}/driver.log")
    return run_dir, os.path.join(home, ".local/state")


def copy_db(state, name, scratch):
    for suffix in ("", "-wal", "-shm"):
        src = os.path.join(state, name + suffix)
        if os.path.exists(src):
            shutil.copyfile(src, os.path.join(scratch, name + suffix))
    path = os.path.join(scratch, name)
    return sqlite3.connect(path) if os.path.exists(path) else None


def transcript_history(metadata):
    history = metadata.get("session_transcript_history_state_v1")
    if history is None:
        return None
    retired = history.get("retired")
    return {
        "retained_generations": [
            (edge.get("commit") or {}).get("rewrite_generation") for edge in history.get("edges") or []
        ],
        "retired_generations": [c.get("rewrite_generation") for c in (retired or {}).get("commits") or []],
        "reanchored": retired is not None,
    }


def read_stores(state):
    """The committed runtime bodies, the sessions.sqlite3 projection and the
    continuity store, read from copies with their -wal and -shm."""
    with tempfile.TemporaryDirectory() as scratch:
        runtime = copy_db(state, "runtime.sqlite", scratch)
        sessions = {}
        for sid, revision, blob in runtime.execute(
            "SELECT session_id, store_revision, blob_sha256 FROM runtime_whole_blob_authority"
        ):
            (body,) = runtime.execute(
                "SELECT session_snapshot FROM runtime_whole_blob_bodies WHERE blob_sha256=?", (blob,)
            ).fetchone()
            snapshot = json.loads(body)
            messages = snapshot.get("messages") or []
            sessions[sid] = {
                "store_revision": revision,
                "messages": len(messages),
                "messages_sha256": messages_digest(messages),
                "transcript_history": transcript_history(snapshot.get("metadata") or {}),
            }
        runtime.close()
        projection = {}
        conn = copy_db(state, "sessions.sqlite3", scratch)
        if conn is not None:
            for sid, data in conn.execute("SELECT session_id, session_json FROM sessions"):
                messages = json.loads(data).get("messages") or []
                projection[sid] = {"messages": len(messages), "messages_sha256": messages_digest(messages)}
            conn.close()
        continuity = None
        conn = copy_db(state, "continuity.sqlite3", scratch)
        if conn is not None:
            tables = sorted(r[0] for r in conn.execute("SELECT name FROM sqlite_master WHERE type='table'"))
            continuity = {
                "schema": [list(r) for r in conn.execute("SELECT * FROM meerkat_schema")],
                "tables": tables,
                "records": {
                    identity: {"session_id": sid, "generation": generation}
                    for identity, sid, generation in conn.execute(
                        "SELECT identity, session_id, generation FROM continuity_records"
                    )
                },
                "snapshot_rows": conn.execute("SELECT count(*) FROM session_snapshots").fetchone()[0],
            }
            conn.close()
    return sessions, projection, continuity


def read_events(path):
    statuses, refusals, ops = {}, [], []
    with open(path, encoding="utf-8") as handle:
        for line in handle:
            row = json.loads(line)
            ops.append([row["boot"], row["op"], row.get("identity")])
            if "error" in row:
                refusals.append(
                    {"boot": row["boot"], "op": row["op"], "identity": row.get("identity"), "message": row["error"][:400]}
                )
            elif row["op"] == "status":
                result = row["result"]
                statuses.setdefault(str(row["boot"]), {})[row["identity"]] = {
                    "state": result.get("state"),
                    "session_id": result.get("session_id"),
                    "generation": result.get("generation"),
                    "agent_runtime_id": result.get("agent_runtime_id"),
                }
    return statuses, refusals, ops


def check_variant(variant, state, sessions, projection, continuity, statuses, refusals, probe):
    """Raise unless the stores show the variant's shape."""

    def fail(what):
        raise SystemExit(f"{variant}: {what}")

    if not probe.get("started"):
        fail(f"0.9.3 cannot build on its own final state: {probe}")
    if set(projection) != set(sessions):
        fail(f"sessions.sqlite3 and runtime.sqlite hold different sessions: {sorted(projection)} {sorted(sessions)}")
    for sid, facts in sessions.items():
        if projection[sid]["messages_sha256"] != facts["messages_sha256"]:
            fail(f"projection and runtime body of {sid} differ")
    store = MOB_STORES.get(variant)
    for name in sorted(set(MOB_STORES.values())):
        present = os.path.exists(os.path.join(state, name))
        if present != (name == store):
            fail(f"{name} present: {present}")
    manifests = sorted(name for name in os.listdir(state) if name.endswith(COMPOSITION_SUFFIX))
    if manifests != ([store + COMPOSITION_SUFFIX] if store else []):
        fail(f"mob composition manifests: {manifests}")
    if variant == "plain":
        if continuity is not None or len(sessions) != 4 or refusals:
            fail(f"expected 4 sessions, no continuity store, no refusal: {len(sessions)} {continuity} {refusals}")
        return
    final = statuses.get("3") or {}
    if continuity is None or continuity["snapshot_rows"] != 0:
        fail(f"expected continuity records without snapshot rows: {continuity}")
    if sorted(continuity["records"]) != ROSTER_IDENTITIES:
        fail(f"records: {sorted(continuity['records'])}")
    for identity in ROSTER_IDENTITIES[:5]:
        bound = (final.get(identity) or {}).get("session_id")
        if continuity["records"][identity]["session_id"] != bound:
            fail(f"{identity}: record and final status name different sessions")
    if "app:app-06" in final:
        fail("app:app-06 still has a status after the roster dropped it")
    readded = final["app:app-05"]["session_id"]
    if sessions[readded]["messages"] != 1 or sessions[readded]["store_revision"] != 1:
        fail(f"app:app-05's re-added session took a turn: {sessions[readded]}")
    if final["app:app-04"]["generation"] != 1:
        fail(f"app:app-04 was not reset: {final['app:app-04']}")
    expected = {(1, "turn", "app:app-05"), (1, "status", "app:app-05"), (3, "status", "app:app-06")}
    if {(r["boot"], r["op"], r["identity"]) for r in refusals} != expected:
        fail(f"refusals: {refusals}")


def comms_identity_files(xdg_state):
    root = os.path.join(xdg_state, "meerkat", "session_comms_identity")
    sessions = {}
    if os.path.isdir(root):
        for sid in sorted(os.listdir(root)):
            sessions[sid] = sorted(os.listdir(os.path.join(root, sid)))
    return {
        "root": "$XDG_STATE_HOME/meerkat/session_comms_identity",
        "stored": False,
        "why": "per-session keypairs (key material) that 0.9.3 keeps outside the state directory",
        "sessions": sessions,
    }


def copy_state(src, dst):
    shutil.copytree(src, dst, symlinks=True)
    empty_dirs = []
    for dirpath, dirnames, filenames in os.walk(dst):
        if not dirnames and not filenames:
            empty_dirs.append(os.path.relpath(dirpath, dst))
    files = []
    for dirpath, _dirnames, filenames in os.walk(dst):
        for name in sorted(filenames):
            full = os.path.join(dirpath, name)
            files.append(
                {"path": os.path.relpath(full, dst), "sha256": sha256_file(full), "bytes": os.path.getsize(full)}
            )
    packed = []
    for row in files:
        if row["bytes"] > PACK_ABOVE_BYTES:
            full = os.path.join(dst, row["path"])
            with open(full, "rb") as raw, gzip.open(full + ".gz", "wb", compresslevel=9, mtime=0) as out:
                shutil.copyfileobj(raw, out)
            os.remove(full)
            packed.append(dict(row, stored_as=row["path"] + ".gz"))
    return sorted(files, key=lambda r: r["path"]), packed, sorted(empty_dirs)


def write_text(out, manifest):
    p = manifest["producer"]
    lines = [
        "Meta 0.9.3 H-app (embedded builder) state directory (synthetic) for #306 A1/G1",
        f"Produced {manifest['generated_at']} by {p['wrapper']} (sha256 {p['wrapper_sha256']})",
        "",
        "PRODUCER",
        f"  binary {p['binary']} sha256 {p['binary_sha256']} (toolchain {p['toolchain']})",
    ]
    for name, digest in sorted(p["driver_files"].items()):
        lines.append(f"  driver/{name} sha256 {digest}")
    versions = sorted({c["version"] for c in p["meerkat_crates"].values()})
    lines.append(f"  {len(p['meerkat_crates'])} meerkat crates from crates.io, versions {versions}")
    lines.append(f"  model: {p['model']}")
    lines.append(f"  compaction: {json.dumps(p['compaction'], sort_keys=True)}")
    lines.append(f"  mob storage: {p['mob_storage']}")
    lines += ["", f"VARIANT {manifest['variant']}", "", "STATUSES (boot: identity state session generation)"]
    for boot, rows in sorted(manifest["statuses"].items()):
        for identity, s in sorted(rows.items()):
            lines.append(f"  boot {boot}: {identity} {s['state']} {s['session_id']} {s['generation']}")
    lines += ["", "0.9.3 REFUSALS RECORDED BY THE DRIVER (boot op identity: message)"]
    for r in manifest["refusals"]:
        lines.append(f"  boot{r['boot']} {r['op']} {r['identity']}: {r['message']}")
    lines += ["", f"0.9.3 RESTART PROBE: started {manifest['restart_probe_093'].get('started')}", ""]
    lines.append("COMMITTED SESSIONS (runtime.sqlite WholeBlob, WAL included; sessions.sqlite3; continuity record)")
    for sid, f in sorted(manifest["committed_sessions"].items()):
        lines.append(
            f"  {sid}: revision {f['store_revision']} messages {f['messages']} projection messages "
            f"{f['projection_messages']} record of {f['record_identity']} transcript {f['transcript_history']}"
        )
    c = manifest["continuity"]
    lines += ["", "CONTINUITY STORE"]
    if c is None:
        lines.append("  none (no roster provider)")
    else:
        lines.append(f"  schema {c['schema']} tables {c['tables']} snapshot rows {c['snapshot_rows']}")
        for identity, r in sorted(c["records"].items()):
            lines.append(f"  record {identity} -> {r['session_id']} generation {r['generation']}")
    o = manifest["outside_state"]["session_comms_identity"]
    lines += ["", f"OUTSIDE THE STATE DIRECTORY (not stored): {o['root']}"]
    for sid, names in sorted(o["sessions"].items()):
        lines.append(f"  {sid}/{', '.join(names)}")
    lines += ["", "PACKED (stored gzip-compressed under the repository's per-file limit; inflate before use)"]
    for row in manifest["packed"]:
        lines.append(f"  ./state/{row['stored_as']} -> ./state/{row['path']} ({row['bytes']} bytes, sha256 {row['sha256']})")
    if not manifest["packed"]:
        lines.append("  none")
    lines += ["", f"EMPTY DIRECTORIES in ./state (git does not keep them): {', '.join(manifest['empty_dirs']) or 'none'}"]
    lines += ["", "FILES (sha256 bytes path; the raw bytes, after inflating the packed files)"]
    for row in manifest["files"]["state"]:
        lines.append(f"{row['sha256']} {row['bytes']} ./state/{row['path']}")
    with open(os.path.join(out, "MANIFEST.txt"), "x", encoding="utf-8") as handle:
        handle.write("\n".join(lines) + "\n")


def write_checksums(out):
    rows = []
    for dirpath, _dirnames, filenames in os.walk(out):
        for name in filenames:
            full = os.path.join(dirpath, name)
            rel = os.path.relpath(full, out)
            if rel != "checksums.sha256":
                rows.append(f"{sha256_file(full)}  {rel}")
    with open(os.path.join(out, "checksums.sha256"), "x", encoding="utf-8") as handle:
        handle.write("\n".join(sorted(rows, key=lambda r: r.split("  ", 1)[1])) + "\n")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", required=True)
    parser.add_argument("--driver-dir", required=True)
    parser.add_argument("--work", required=True, help="new empty scratch directory")
    parser.add_argument("--out", required=True, help="new directory for the variants")
    parser.add_argument("--variants", nargs="+", choices=VARIANTS, default=list(VARIANTS))
    args = parser.parse_args()
    binary = os.path.abspath(args.binary)
    driver_dir = os.path.abspath(args.driver_dir)
    os.makedirs(args.work)
    os.makedirs(args.out)
    producer = {
        "wrapper": os.path.basename(__file__),
        "wrapper_sha256": sha256_file(os.path.abspath(__file__)),
        "binary": os.path.basename(binary),
        "binary_sha256": sha256_file(binary),
        "toolchain": TOOLCHAIN,
        "driver_files": {name: sha256_file(os.path.join(driver_dir, name)) for name in DRIVER_FILES},
        "meerkat_crates": meerkat_crates(os.path.join(driver_dir, "Cargo.lock.fixture")),
        "model": "scripted in-process client; reply 'app reply N'",
        "compaction": {
            "auto_compact_threshold": COMPACT_THRESHOLD,
            "recent_turn_budget": 1,
            "reported_prompt_tokens": REPORTED_PROMPT_TOKENS,
            "trigger": "a user message carrying [compact-now] reports a cost above the threshold once",
        },
    }
    bad = {n: c for n, c in producer["meerkat_crates"].items() if c != {"version": "0.9.3", "source": "registry"}}
    if bad or not producer["meerkat_crates"]:
        raise SystemExit(f"Cargo.lock.fixture does not pin every meerkat crate to 0.9.3 from crates.io: {bad}")
    for variant in args.variants:
        work = os.path.join(os.path.abspath(args.work), variant)
        os.makedirs(work)
        run_dir, xdg_state = run_variant(binary, work, variant)
        state = os.path.join(run_dir, "state")
        probe = json.load(open(os.path.join(run_dir, "probe.json"), encoding="utf-8"))
        statuses, refusals, ops = read_events(os.path.join(run_dir, "events.jsonl"))
        sessions, projection, continuity = read_stores(state)
        check_variant(variant, state, sessions, projection, continuity, statuses, refusals, probe)
        out = os.path.join(os.path.abspath(args.out), variant)
        os.makedirs(out)
        files, packed, empty_dirs = copy_state(state, os.path.join(out, "state"))
        shutil.copyfile(os.path.join(run_dir, "events.jsonl"), os.path.join(out, "events.jsonl"))
        shutil.copyfile(os.path.join(run_dir, "probe.json"), os.path.join(out, "probe.json"))
        records = {r["session_id"]: identity for identity, r in ((continuity or {}).get("records") or {}).items()}
        manifest = {
            "corpus": "released_0_9_3_h_app",
            "variant": variant,
            "generated_at": datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
            "producer": dict(
                producer,
                mob_storage=f"persistent, state/{MOB_STORES[variant]}"
                if variant in MOB_STORES
                else "in-memory (the 0.9.3 builder default)",
            ),
            "operations": ops,
            "statuses": statuses,
            "refusals": refusals,
            "restart_probe_093": probe,
            "committed_sessions": {
                sid: dict(
                    facts,
                    projection_messages=projection[sid]["messages"],
                    record_identity=records.get(sid),
                )
                for sid, facts in sessions.items()
            },
            "continuity": continuity,
            "outside_state": {"session_comms_identity": comms_identity_files(xdg_state)},
            "files": {"state": files},
            "packed": packed,
            "empty_dirs": empty_dirs,
        }
        with open(os.path.join(out, "MANIFEST.json"), "x", encoding="utf-8") as handle:
            json.dump(manifest, handle, indent=2, sort_keys=True)
            handle.write("\n")
        write_text(out, manifest)
        write_checksums(out)
        print(f"{variant}: written to {out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
