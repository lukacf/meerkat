#!/usr/bin/env python3
"""Mint the released Meta 0.9.3 H-local state directory fixtures.

The state directories beside this script were written by the PUBLIC RELEASE
0.9.3 ``rpc_gateway`` (release v0.9.3 of lukacf/meerkat-meta, archive
``meta-rpc-gateway-0.9.3-x86_64-unknown-linux-gnu.tar.gz``, sha256
b416df1ba5648ca3ed849dec2692f664c37b0e7277232a6d2421ac53961a18b0), never by
the build under test. This script is how they were minted, so the capture
can be reproduced; no test runs it.

It drives the released gateway the way an H-local host runs it: one
``rpc_gateway --persistent`` process per boot speaking JSON-RPC over
stdin/stdout, ``meta/init`` with persistent state, a roster served through
``callback/roster_provider/roster``, eager identity bootstrap and automatic
compaction. A loopback chat-completions server answers every model request
("corpus reply N"); a user message carrying ``[ct:<key>]`` reports usage
above the compaction threshold once, so the session compacts at its next
boundary. Nothing leaves the machine and no provider key is read. The
gateway runs with a minimal environment (PATH, LANG, RUST_LOG, and HOME and
XDG roots inside --work), so it reads and writes nothing outside --work.

Boots (identities agent:hl-01 .. agent:hl-18, profile "specialist"):
  boot 1  roster of 18; a status read and one turn each; compactions for
          hl-01..hl-03; retire hl-04 and hl-05, then wake hl-04 by a send
          (0.9.3 refuses it: stale continuity checkpoint, identity Broken);
          reset hl-06; delete_identity hl-07; a roster reconcile (hl-04
          active again); turns for hl-04 and hl-07
  boot 2  follow-up turns for hl-01, hl-04, hl-06, hl-07; a second
          compaction for hl-01 and one for hl-08; two resets of hl-09; retire
          then wake hl-10; meta/compact_member on hl-11 (0.9.3 fails it and
          leaves the identity Broken)
  boot 3  a status read for every identity; a turn for hl-12; retire then
          wake hl-13; retire hl-15 (it stays retired)

Variants (--variant):
  bootable  boots 1-3, the roster drops hl-18 at boot 3, every boot ends with
            a clean shutdown
  stuck     boots 1-2, the roster drops hl-18 at boot 2
  killed    as bootable, then meta/compact_member on hl-14 at the end of
            boot 3: the maintenance turn never completes, meta/shutdown gets
            no answer within 180 s and the gateway is killed (SIGKILL)
Every 0.9.3 refusal is recorded as data (events.jsonl, MANIFEST), not raised.
After the last boot the released gateway starts once more on a copy of the
final state (the restart probe) and the outcome is recorded: the stuck
variant's meta/init fails ("mob actor command channel closed"), so 0.9.3
itself cannot start that directory again.

Usage (--work and --out must not exist):

    gh release download v0.9.3 -R lukacf/meerkat-meta \\
        -p 'meta-rpc-gateway-0.9.3-x86_64-unknown-linux-gnu.tar.gz' \\
        -p checksums.sha256
    # verify the archive against checksums.sha256, extract it into its own
    # empty directory, then for each variant:
    python3 -I mint_released_0_9_3_h_local.py --variant bootable \\
        --binary <dir>/rpc_gateway --release-asset-sha256 <archive sha256> \\
        --work /tmp/h-local-work --out /tmp/h-local-bootable

Outputs under --out:
  runs/bootN/    the state directory copied after boot N ended
  state/         the final state directory without the omitted files
                 (the gateway peer key; see OMITTED); a file above 900 KiB
                 is stored as <name>.gz (see PACK_ABOVE_BYTES)
  events.jsonl   every operation with its result or error
  MANIFEST.json  producer, rosters, shutdown outcomes, per-identity session
                 history and last status, 0.9.3 refusals, the restart probe,
                 per-session committed transcript facts (runtime.sqlite read
                 with its WAL), file digests, empty directories
  MANIFEST.txt   the human-readable twin
  checksums.sha256  sha256sum lines for every stored file above
The fixture keeps everything except ``runs/``. To use a variant, copy its
``state/`` somewhere else, inflate each ``.gz`` it lists under "packed",
recreate the empty directories it lists, and never open the files in place.
"""

import argparse
import datetime
import gzip
import hashlib
import http.server
import json
import os
import queue
import shutil
import socketserver
import sqlite3
import subprocess
import sys
import tempfile
import threading
import time

MODEL = "hl-corpus-model"
MOB_ID = "hl-corpus-mob"
PROFILE = "specialist"
COMPACT_THRESHOLD = 50000

IDENTITIES = [f"agent:hl-{n:02d}" for n in range(1, 19)]

# The repository refuses a pushed file above 1000 KB, so a final-state file
# above this size is stored gzip-compressed as <name>.gz (mtime 0, no name in
# the header, so the bytes are reproducible); MANIFEST.json "packed" names the
# raw size and sha256 that inflating it must give back.
PACK_ABOVE_BYTES = 900 * 1024

# Left out of the copied final state.
OMITTED = {
    "peer_key.ed25519": "gateway peer key (key material); an installation without host pairings mints a new one on boot",
}


def ident(n):
    return f"agent:hl-{n:02d}"


def request_texts(request):
    texts = []
    for message in (request or {}).get("messages") or []:
        content = message.get("content")
        if isinstance(content, str):
            texts.append((message.get("role"), content))
        elif isinstance(content, list):
            texts.append(
                (
                    message.get("role"),
                    "\n".join(part.get("text", "") for part in content if isinstance(part, dict)),
                )
            )
    return texts


class Model:
    """Loopback chat-completions model.

    A user message carrying "[ct:<key>]" makes the request that answers it
    report a provider input cost above the auto-compaction threshold, once per
    key, so the session compacts at its next boundary.
    """

    def __init__(self):
        self.lock = threading.Lock()
        self.count = 0
        self.consumed = set()
        model = self

        class Handler(http.server.BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def log_message(self, *args):
                pass

            def _reply(self, status, ctype, body):
                self.send_response(status)
                self.send_header("content-type", ctype)
                self.send_header("content-length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)

            def do_GET(self):
                if self.path.rstrip("/") in ("/v1/models", "/models"):
                    self._reply(200, "application/json", b'{"data": []}')
                else:
                    self._reply(404, "text/plain", b"")

            def do_POST(self):
                length = int(self.headers.get("content-length") or 0)
                raw = self.rfile.read(length)
                try:
                    request = json.loads(raw)
                except ValueError:
                    request = None
                prompt_tokens = model.prompt_tokens(request)
                with model.lock:
                    model.count += 1
                    n = model.count
                chunks = [
                    {"choices": [{"delta": {"content": f"corpus reply {n}"}}]},
                    {
                        "choices": [{"finish_reason": "stop"}],
                        "usage": {"prompt_tokens": prompt_tokens, "completion_tokens": 1},
                    },
                ]
                payload = "".join(f"data: {json.dumps(c)}\n\n" for c in chunks)
                payload += "data: [DONE]\n\n"
                self._reply(200, "text/event-stream", payload.encode())

        self.server = socketserver.ThreadingTCPServer(("127.0.0.1", 0), Handler)
        self.server.daemon_threads = True
        threading.Thread(target=self.server.serve_forever, daemon=True).start()

    def prompt_tokens(self, request):
        users = [text for role, text in request_texts(request) if role == "user"]
        if not users:
            return 10
        last = users[-1]
        start = last.find("[ct:")
        if start < 0:
            return 10
        end = last.find("]", start)
        key = last[start + 4 : end]
        with self.lock:
            if key in self.consumed:
                return 10
            self.consumed.add(key)
        return COMPACT_THRESHOLD + 10000

    @property
    def base_url(self):
        return f"http://127.0.0.1:{self.server.server_address[1]}"

    def served(self):
        with self.lock:
            return self.count


def mob_config():
    return f"""
[mob]
id = "{MOB_ID}"

[profiles.{PROFILE}]
model = "{MODEL}"
provider = "self_hosted"
external_addressable = true
runtime_mode = "turn_driven"

[profiles.{PROFILE}.tools]
comms = true
"""


def realm_binding(realm):
    return f"""
[realm."{realm}"]
default_binding = "loop"

[realm."{realm}".backend.loop]
provider = "self_hosted"
backend_kind = "self_hosted"

[realm."{realm}".auth.loop]
provider = "self_hosted"
auth_method = "none"
source = {{ kind = "platform_default" }}

[realm."{realm}".binding.loop]
backend_profile = "loop"
auth_profile = "loop"
default_model = "{MODEL}"
provider_default = true
"""


def host_config(base_url):
    text = f"""
[self_hosted]
default_model = "{MODEL}"

[self_hosted.servers.loop]
base_url = "{base_url}"

[self_hosted.models."{MODEL}"]
server = "loop"
remote_model = "{MODEL}"
display_name = "H-local corpus model"
family = "{MODEL}"
"""
    text += realm_binding("global")
    text += realm_binding(f"mob.{MOB_ID}")
    return text


def roster_spec(identity):
    return {
        "identity": identity,
        "profile": PROFILE,
        "addressability": "addressable",
        "display_name": None,
        "labels": {},
        "context": None,
        "additional_instructions": [],
    }


class Events:
    def __init__(self, path):
        self.handle = open(path, "x", encoding="utf-8")

    def record(self, boot, op, identity, result=None, error=None):
        row = {"boot": boot, "op": op, "identity": identity}
        if result is not None:
            row["result"] = result
        if error is not None:
            row["error"] = error
        self.handle.write(json.dumps(row, sort_keys=True) + "\n")
        self.handle.flush()

    def close(self):
        self.handle.close()


class Gateway:
    def __init__(self, binary, work, label, log_path, roster):
        self.label = label
        self.roster = roster
        self.next_id = 0
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
        cwd = os.path.join(work, "cwd")
        os.makedirs(cwd, exist_ok=True)
        self.stderr = open(log_path, "x", encoding="utf-8")
        self.proc = subprocess.Popen(
            [binary, "--persistent"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=self.stderr,
            cwd=cwd,
            env=env,
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
                "error": {"code": -32601, "message": f"corpus host does not provide {method}"},
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

    def shutdown(self, kill_on_timeout=False):
        """meta/shutdown, EOF, exit 0. With kill_on_timeout a shutdown that
        gets no answer ends with SIGKILL instead (recorded, not an error)."""
        try:
            response = self.request("meta/shutdown", {}, timeout=180.0)
        except RuntimeError as error:
            if not kill_on_timeout or self.proc.poll() is not None:
                raise
            self.proc.kill()
            self.proc.wait()
            self.stderr.close()
            print(f"[{self.label}] {error}; killed (SIGKILL)", flush=True)
            return {"killed": True, "reason": str(error)}
        if response.get("error") is not None:
            raise RuntimeError(f"[{self.label}] meta/shutdown failed: {json.dumps(response)}")
        result = response.get("result")
        print(f"[{self.label}] shutdown: {json.dumps(result)[:300]}", flush=True)
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
        return {"killed": False, "result": result}


def init_params(state, host_config_path):
    return {
        "persistent_state": state,
        "mob_config": mob_config(),
        "has_roster_provider": True,
        "runtime_options": {
            "meerkat_config_path": host_config_path,
            "identity_bootstrap_mode": {"mode": "eager_materialize"},
            # 0.9.3 defaults to 16 live sessions; an H-local roster of 18
            # needs more, as production hosts configure.
            "max_sessions": 64,
            "compaction": {
                "auto_compact_threshold": COMPACT_THRESHOLD,
                "recent_turn_budget": 1,
            },
        },
    }


class Driver:
    def __init__(self, gateway, events, boot):
        self.gateway = gateway
        self.events = events
        self.boot = boot

    def status(self, identity):
        result = self.gateway.call("meta/status_identity", {"identity": identity})
        self.events.record(self.boot, "status", identity, result=result)
        return result

    def turn(self, identity, text):
        sent = self.gateway.call(
            "meta/send", {"identity": identity, "content": text, "track_turn": True}
        )
        ticket = (sent.get("turn") or {}).get("ticket")
        if ticket is None:
            raise RuntimeError(f"untracked turn for {identity}: {json.dumps(sent)}")
        waited = self.gateway.call(
            "meta/wait_for_turn",
            {"identity": identity, "ticket": ticket, "timeout_ms": 90000},
            timeout=120.0,
        )
        if waited.get("state") != "completed":
            raise RuntimeError(f"turn for {identity} did not complete: {json.dumps(waited)}")
        self.events.record(self.boot, "turn", identity, result={"text": text, "waited": waited})
        return waited

    def try_turn(self, identity, text):
        """A turn after a lifecycle verb. 0.9.3 refuses some of these (a wake
        after retire fails on a stale continuity checkpoint and leaves the
        identity Broken); the refusal is corpus data, so it is recorded and
        the boot goes on."""
        sent = self.gateway.request(
            "meta/send", {"identity": identity, "content": text, "track_turn": True}
        )
        error = sent.get("error")
        if error is not None:
            self.events.record(self.boot, "turn_refused", identity, result={"text": text}, error=error)
            print(
                f"[{self.gateway.label}] turn refused for {identity}: {json.dumps(error)[:300]}",
                flush=True,
            )
            return None
        ticket = ((sent.get("result") or {}).get("turn") or {}).get("ticket")
        if ticket is None:
            raise RuntimeError(f"untracked turn for {identity}: {json.dumps(sent)}")
        waited = self.gateway.request(
            "meta/wait_for_turn",
            {"identity": identity, "ticket": ticket, "timeout_ms": 90000},
            timeout=120.0,
        )
        result = waited.get("result") or {}
        if waited.get("error") is not None or result.get("state") != "completed":
            self.events.record(
                self.boot, "turn_incomplete", identity, result={"text": text, "waited": result},
                error=waited.get("error"),
            )
            print(f"[{self.gateway.label}] turn incomplete for {identity}: {json.dumps(waited)[:300]}", flush=True)
            return None
        self.events.record(self.boot, "turn", identity, result={"text": text, "waited": result})
        return result

    def compact(self, identity, key):
        """Two turns: the first reports a cost above the threshold, the
        second runs after the session compacted at its boundary."""
        self.turn(identity, f"boot {self.boot} please grow [ct:{key}] for {identity}")
        self.turn(identity, f"boot {self.boot} after compaction {key} for {identity}")

    def verb(self, method, identity, allow_error=False, timeout=120.0):
        response = self.gateway.request(method, {"identity": identity}, timeout=timeout)
        error = response.get("error")
        if error is not None and not allow_error:
            raise RuntimeError(f"{method} {identity} failed: {json.dumps(response)}")
        self.events.record(
            self.boot, method, identity, result=response.get("result"), error=error
        )
        print(
            f"[{self.gateway.label}] {method} {identity}: "
            f"{json.dumps(response.get('result') if error is None else error)[:300]}",
            flush=True,
        )
        return response

    def reconcile(self):
        result = self.gateway.call("meta/reconcile_identity", {}, timeout=240.0)
        self.events.record(self.boot, "meta/reconcile_identity", None, result=result)
        return result


def boot1(d):
    for identity in IDENTITIES:
        d.status(identity)
        d.turn(identity, f"boot 1 hello from the host to {identity}")
    for n in (1, 2, 3):
        d.compact(ident(n), f"b1-{n}")
    d.verb("meta/retire", ident(4), allow_error=True)
    d.verb("meta/retire", ident(5), allow_error=True)
    d.try_turn(ident(4), f"boot 1 wake after retire for {ident(4)}")
    d.status(ident(4))
    d.verb("meta/reset", ident(6), allow_error=True)
    d.try_turn(ident(6), f"boot 1 first turn after reset for {ident(6)}")
    d.verb("meta/delete_identity", ident(7), allow_error=True)
    d.reconcile()
    d.status(ident(4))
    d.try_turn(ident(4), f"boot 1 after reconcile for {ident(4)}")
    d.try_turn(ident(7), f"boot 1 first turn after delete for {ident(7)}")


def boot2(d):
    for identity in IDENTITIES:
        d.status(identity)
    for n in (1, 4, 6, 7):
        d.try_turn(ident(n), f"boot 2 follow-up for {ident(n)}")
    d.compact(ident(1), "b2-1")
    d.compact(ident(8), "b2-8")
    d.verb("meta/reset", ident(9), allow_error=True)
    d.try_turn(ident(9), f"boot 2 after first reset for {ident(9)}")
    d.verb("meta/reset", ident(9), allow_error=True)
    d.try_turn(ident(9), f"boot 2 after second reset for {ident(9)}")
    d.verb("meta/retire", ident(10), allow_error=True)
    d.try_turn(ident(10), f"boot 2 wake after retire for {ident(10)}")
    d.status(ident(10))
    d.turn(ident(11), f"boot 2 before compact_member for {ident(11)}")
    d.verb("meta/compact_member", ident(11), allow_error=True, timeout=180.0)
    d.status(ident(11))


def boot3(d):
    for identity in IDENTITIES:
        d.status(identity)
    d.turn(ident(12), f"boot 3 follow-up for {ident(12)}")
    d.verb("meta/retire", ident(13), allow_error=True)
    d.try_turn(ident(13), f"boot 3 wake after retire for {ident(13)}")
    d.status(ident(13))
    d.verb("meta/retire", ident(15), allow_error=True)
    d.status(ident(15))


def boot3_compact_member(d):
    boot3(d)
    d.turn(ident(14), f"boot 3 before compact_member for {ident(14)}")
    d.verb("meta/compact_member", ident(14), allow_error=True, timeout=180.0)
    d.status(ident(14))


# (boot, roster, plan, kill the gateway when meta/shutdown gets no answer)
PLANS = {
    # The roster drops hl-18 at boot 3; hl-15 stays retired. The 0.9.3
    # gateway starts on the final state again (the restart probe).
    "bootable": [
        (1, IDENTITIES, boot1, False),
        (2, IDENTITIES, boot2, False),
        (3, IDENTITIES[:-1], boot3, False),
    ],
    # The roster drops hl-18 at boot 2, which then runs the lifecycle verbs
    # and ends with hl-11 Broken. The 0.9.3 gateway does not start on the
    # final state again.
    "stuck": [
        (1, IDENTITIES, boot1, False),
        (2, IDENTITIES[:-1], boot2, False),
    ],
    # As "bootable", then meta/compact_member on hl-14 at boot 3: the
    # maintenance turn never completes, meta/shutdown gets no answer and the
    # gateway is killed, the way a host ends a wedged 0.9.3 process.
    "killed": [
        (1, IDENTITIES, boot1, False),
        (2, IDENTITIES, boot2, False),
        (3, IDENTITIES[:-1], boot3_compact_member, True),
    ],
}


def restart_probe(binary, work, logs, state, host_config_path, roster_identities):
    """Starts the 0.9.3 gateway once more on a copy of the final state and
    records whether it comes up. The copy keeps the corpus state untouched."""
    probe_state = os.path.join(work, "probe-state")
    shutil.copytree(state, probe_state, symlinks=True)
    label = "hl-probe"
    gateway = Gateway(
        binary,
        work,
        label,
        os.path.join(logs, f"{label}-stderr.log"),
        [roster_spec(i) for i in roster_identities],
    )
    try:
        response = gateway.request(
            "meta/init", init_params(probe_state, host_config_path), timeout=300.0
        )
        error = response.get("error")
        if error is None:
            gateway.shutdown()
            outcome = {"started": True}
        else:
            gateway.proc.stdin.close()
            deadline = time.monotonic() + 120
            while gateway.proc.poll() is None and time.monotonic() < deadline:
                gateway.pump(0.2)
            if gateway.proc.poll() is None:
                gateway.proc.kill()
                gateway.proc.wait()
            gateway.stderr.close()
            outcome = {
                "started": False,
                "code": error.get("code"),
                "message": error.get("message"),
                "exit_status": gateway.proc.returncode,
            }
    except BaseException:
        if gateway.proc.poll() is None:
            gateway.proc.kill()
            gateway.proc.wait()
        raise
    print(f"[{label}] restart probe: {json.dumps(outcome)[:400]}", flush=True)
    return outcome


def sha256_file(path):
    with open(path, "rb") as handle:
        return hashlib.sha256(handle.read()).hexdigest()


def tree_digest(root):
    rows = []
    for base, dirs, files in os.walk(root):
        dirs.sort()
        for name in sorted(files):
            path = os.path.join(base, name)
            rel = os.path.relpath(path, root)
            if os.path.islink(path):
                rows.append({"path": rel, "sha256": "symlink", "bytes": 0})
                continue
            with open(path, "rb") as handle:
                data = handle.read()
            rows.append({"path": rel, "sha256": hashlib.sha256(data).hexdigest(), "bytes": len(data)})
    return rows


def runtime_sessions(state_dir):
    """Committed WholeBlob sessions, read from a copy of runtime.sqlite with
    its -wal and -shm so committed WAL frames are included."""
    source = os.path.join(state_dir, "runtime.sqlite")
    with tempfile.TemporaryDirectory() as scratch:
        for suffix in ("", "-wal", "-shm"):
            if os.path.exists(source + suffix):
                shutil.copy2(source + suffix, os.path.join(scratch, "runtime.sqlite" + suffix))
        con = sqlite3.connect(os.path.join(scratch, "runtime.sqlite"))
        try:
            sessions = {}
            rows = con.execute(
                "SELECT runtime_id, session_id, store_revision, blob_sha256 "
                "FROM runtime_whole_blob_authority ORDER BY runtime_id"
            ).fetchall()
            for runtime_id, session_id, revision, blob_sha in rows:
                (body,) = con.execute(
                    "SELECT session_snapshot FROM runtime_whole_blob_bodies WHERE blob_sha256=?",
                    (blob_sha,),
                ).fetchone()
                snapshot = json.loads(body)
                messages = snapshot.get("messages") or []
                metadata = snapshot.get("metadata") or {}
                sessions[session_id] = {
                    "runtime_id": runtime_id,
                    "store_revision": revision,
                    "blob_sha256": blob_sha,
                    "messages": len(messages),
                    "compacted_markers": sum(
                        1 for m in messages if "[Context compacted]" in json.dumps(m)
                    ),
                    "rewrite_prefix_authority": metadata.get(
                        "session_transcript_rewrite_prefix_authority_v1"
                    ),
                    "messages_sha256": hashlib.sha256(
                        json.dumps(messages, sort_keys=True).encode()
                    ).hexdigest(),
                }
            return sessions
        finally:
            con.close()


def session_history(events_path):
    history = {}
    final = {}
    refusals = []
    with open(events_path, encoding="utf-8") as handle:
        for line in handle:
            row = json.loads(line)
            identity = row.get("identity")
            result = row.get("result")
            error = row.get("error")
            if error is not None:
                refusals.append(
                    {
                        "boot": row["boot"],
                        "op": row["op"],
                        "identity": identity,
                        "code": error.get("code"),
                        "message": (error.get("message") or "")[:400],
                    }
                )
            if identity is None or not isinstance(result, dict):
                continue
            session = result.get("session_id")
            if session is not None:
                seen = history.setdefault(identity, [])
                if not seen or seen[-1] != session:
                    seen.append(session)
            if row["op"] == "status":
                final[identity] = {
                    "boot": row["boot"],
                    "state": result.get("state"),
                    "session_id": result.get("session_id"),
                    "generation": result.get("generation"),
                    "agent_runtime_id": result.get("agent_runtime_id"),
                }
    return history, final, refusals


def write_text_manifest(out, manifest):
    """The human-readable twin of MANIFEST.json."""
    producer = manifest["producer"]
    lines = [
        "Meta 0.9.3 H-local production-shaped state directory (synthetic) for #306 A1/G1",
        f"Produced {manifest['generated_at']} by {manifest['producer']['driver']}",
        "",
        "PRODUCER",
        f"  release asset: {producer['release_asset']} sha256 {producer['release_asset_sha256']}",
        f"  binary: {producer['binary']} sha256 {producer['binary_sha256']}",
        f"  driver: {producer['driver']} sha256 {producer['driver_sha256']}",
        f"  model: {producer['model']}",
        f"  compaction: {json.dumps(producer['compaction'], sort_keys=True)}",
        "",
        f"VARIANT {manifest['variant']}",
        "",
        "RUNS (runs/bootN = the state directory copied after boot N ended; "
        f"./state = runs/boot{max(int(b) for b in manifest['rosters'])} without the omitted files)",
    ]
    for boot, roster in sorted(manifest["rosters"].items()):
        shutdown = manifest["shutdowns"][boot]
        ending = "killed (SIGKILL) after meta/shutdown got no answer" if shutdown["killed"] else "clean shutdown"
        lines.append(f"  boot{boot}: roster of {len(roster)}: {roster[0]} .. {roster[-1]}; {ending}")
    lines.append("  omitted from ./state: " + "; ".join(f"{k} ({v})" for k, v in manifest["omitted_from_state"].items()))
    lines.append("  empty directories in ./state (git does not keep them): " + (", ".join(manifest["empty_dirs"]) or "none"))
    lines += ["", "IDENTITIES (sessions seen in order; final status)"]
    for identity, facts in sorted(manifest["identities"].items()):
        final = facts.get("final_status") or {}
        lines.append(
            f"  {identity}: sessions {len(facts['sessions_seen'])} {facts['sessions_seen']}; "
            f"final {final.get('state')} session {final.get('session_id')} "
            f"generation {final.get('generation')} (boot {final.get('boot')})"
        )
    probe = manifest["restart_probe_093"]
    lines += [
        "",
        "0.9.3 RESTART PROBE (the 0.9.3 gateway started once more on a copy of ./state)",
        f"  started {probe['started']}"
        + ("" if probe["started"] else f"; {probe.get('code')} {probe.get('message')}"),
    ]
    lines += ["", "0.9.3 REFUSALS RECORDED BY THE DRIVER (boot op identity: code message)"]
    for row in manifest["refusals"]:
        lines.append(f"  boot{row['boot']} {row['op']} {row['identity']}: {row['code']} {row['message']}")
    lines += ["", "COMMITTED SESSIONS (runtime.sqlite WholeBlob authority, WAL included)"]
    for session, facts in sorted(manifest["committed_sessions"].items()):
        lines.append(
            f"  {session}: revision {facts['store_revision']} messages {facts['messages']} "
            f"compacted_markers {facts['compacted_markers']} messages_sha256 {facts['messages_sha256']}"
        )
    lines += ["", "PACKED (stored gzip-compressed under the repository's per-file limit; inflate before use)"]
    for row in manifest["packed"]:
        lines.append(
            f"  ./state/{row['stored_as']} -> ./state/{row['path']} "
            f"({row['bytes']} bytes, sha256 {row['sha256']})"
        )
    if not manifest["packed"]:
        lines.append("  none")
    lines += ["", "FILES (sha256 bytes path; the raw bytes, after inflating the packed files)"]
    for row in manifest["files"]["state"]:
        lines.append(f"{row['sha256']} {row['bytes']} ./state/{row['path']}")
    with open(os.path.join(out, "MANIFEST.txt"), "x", encoding="utf-8") as handle:
        handle.write("\n".join(lines) + "\n")


def copy_final_state(state, final_state):
    """Copy the final state without the omitted files; files above
    PACK_ABOVE_BYTES are written as <name>.gz instead of raw."""

    def skip(directory, names):
        skipped = {name for name in names if name in OMITTED}
        for name in names:
            path = os.path.join(directory, name)
            if (
                os.path.isfile(path)
                and not os.path.islink(path)
                and os.path.getsize(path) > PACK_ABOVE_BYTES
            ):
                skipped.add(name)
        return skipped

    shutil.copytree(state, final_state, symlinks=True, ignore=skip)
    packed = []
    for base, dirs, files in os.walk(state):
        dirs.sort()
        for name in sorted(files):
            path = os.path.join(base, name)
            rel = os.path.relpath(path, state)
            if rel in OMITTED or os.path.islink(path) or os.path.getsize(path) <= PACK_ABOVE_BYTES:
                continue
            with open(path, "rb") as handle:
                data = handle.read()
            stored = gzip.compress(data, compresslevel=9, mtime=0)
            with open(os.path.join(final_state, rel + ".gz"), "xb") as handle:
                handle.write(stored)
            packed.append(
                {
                    "path": rel,
                    "stored_as": rel + ".gz",
                    "bytes": len(data),
                    "sha256": hashlib.sha256(data).hexdigest(),
                    "stored_bytes": len(stored),
                    "stored_sha256": hashlib.sha256(stored).hexdigest(),
                }
            )
    return packed


def write_checksums(out):
    """checksums.sha256 (sha256sum format) over every stored fixture file."""
    rows = []
    for rel in ("events.jsonl", "MANIFEST.json", "MANIFEST.txt"):
        rows.append((sha256_file(os.path.join(out, rel)), rel))
    for row in tree_digest(os.path.join(out, "state")):
        rows.append((row["sha256"], "state/" + row["path"]))
    with open(os.path.join(out, "checksums.sha256"), "x", encoding="utf-8") as handle:
        for digest, rel in sorted(rows, key=lambda r: r[1]):
            handle.write(f"{digest}  {rel}\n")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", required=True)
    parser.add_argument("--release-asset-sha256", required=True)
    parser.add_argument("--work", required=True, help="new empty scratch directory")
    parser.add_argument("--out", required=True, help="new directory for the corpus")
    parser.add_argument("--variant", choices=sorted(PLANS), default="bootable")
    parser.add_argument(
        "--probe-only",
        metavar="STATE_DIR",
        help="only start the 0.9.3 gateway on a copy of STATE_DIR (final roster) and record the outcome",
    )
    args = parser.parse_args()
    plan_rows = PLANS[args.variant]
    # The gateway runs with its own cwd, so every path it reads is absolute.
    args.work = os.path.abspath(args.work)
    args.out = os.path.abspath(args.out)
    args.binary = os.path.abspath(args.binary)

    os.makedirs(args.work)
    os.makedirs(args.out)
    runs = os.path.join(args.out, "runs")
    os.makedirs(runs)
    state = os.path.join(args.work, "state")
    config_dir = os.path.join(args.work, "config")
    logs = os.path.join(args.work, "logs")
    os.makedirs(config_dir)
    os.makedirs(logs)
    model = Model()
    host_config_path = os.path.join(config_dir, "config.toml")
    with open(host_config_path, "x", encoding="utf-8") as handle:
        handle.write(host_config(model.base_url))
    if args.probe_only is not None:
        probe = restart_probe(
            args.binary,
            args.work,
            logs,
            os.path.abspath(args.probe_only),
            host_config_path,
            plan_rows[-1][1],
        )
        probe["state_dir"] = os.path.abspath(args.probe_only)
        with open(os.path.join(args.out, "probe.json"), "x", encoding="utf-8") as handle:
            json.dump(probe, handle, indent=2, sort_keys=True)
            handle.write("\n")
        return 0
    events_path = os.path.join(args.out, "events.jsonl")
    events = Events(events_path)

    rosters = {}
    shutdowns = {}
    for boot, roster_identities, plan, kill_on_timeout in plan_rows:
        rosters[boot] = list(roster_identities)
        label = f"hl-boot{boot}"
        gateway = Gateway(
            args.binary,
            args.work,
            label,
            os.path.join(logs, f"{label}-stderr.log"),
            [roster_spec(i) for i in roster_identities],
        )
        try:
            result = gateway.call("meta/init", init_params(state, host_config_path), timeout=300.0)
            events.record(boot, "meta/init", None, result=result)
            print(f"[{label}] init ok: {json.dumps(result)[:300]}", flush=True)
            plan(Driver(gateway, events, boot))
            shutdowns[boot] = gateway.shutdown(kill_on_timeout=kill_on_timeout)
            events.record(boot, "meta/shutdown", None, result=shutdowns[boot])
        except BaseException:
            if gateway.proc.poll() is None:
                gateway.proc.kill()
                gateway.proc.wait()
            raise
        snapshot = os.path.join(runs, f"boot{boot}")
        shutil.copytree(state, snapshot, symlinks=True)
        print(f"[{label}] model served {model.served()} requests; snapshot {snapshot}", flush=True)
    events.close()
    probe = restart_probe(
        args.binary, args.work, logs, state, host_config_path, rosters[plan_rows[-1][0]]
    )

    final_state = os.path.join(args.out, "state")
    packed = copy_final_state(state, final_state)
    raw_files = [row for row in tree_digest(state) if row["path"] not in OMITTED]
    empty_dirs = sorted(
        os.path.relpath(base, state)
        for base, dirs, files in os.walk(state)
        if not dirs and not files
    )
    history, final_status, refusals = session_history(events_path)
    sessions = runtime_sessions(state)
    manifest = {
        "corpus": "meta-0.9.3 H-local production-shaped state directory (#306 A1, G1)",
        "generated_at": datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "producer": {
            "binary": os.path.basename(args.binary),
            "binary_sha256": sha256_file(args.binary),
            "release_asset": "meta-rpc-gateway-0.9.3-x86_64-unknown-linux-gnu.tar.gz",
            "release_asset_sha256": args.release_asset_sha256,
            "driver": os.path.basename(__file__),
            "driver_sha256": sha256_file(os.path.abspath(__file__)),
            "model": "loopback chat-completions fake; reply 'corpus reply N'",
            "compaction": {
                "auto_compact_threshold": COMPACT_THRESHOLD,
                "recent_turn_budget": 1,
                "trigger": "a user message carrying [ct:<key>] reports a cost above the threshold once",
            },
        },
        "variant": args.variant,
        "mob_id": MOB_ID,
        "profile": PROFILE,
        "rosters": rosters,
        "identities": {
            identity: {
                "sessions_seen": history.get(identity, []),
                "final_status": final_status.get(identity),
            }
            for identity in IDENTITIES
        },
        "shutdowns": shutdowns,
        "omitted_from_state": {name: OMITTED[name] for name in OMITTED},
        "empty_dirs": empty_dirs,
        "refusals": refusals,
        "restart_probe_093": probe,
        "committed_sessions": sessions,
        "packed": packed,
        "files": {"state": raw_files},
    }
    with open(os.path.join(args.out, "MANIFEST.json"), "x", encoding="utf-8") as handle:
        json.dump(manifest, handle, indent=2, sort_keys=True)
        handle.write("\n")
    write_text_manifest(args.out, manifest)
    write_checksums(args.out)
    print(f"corpus written to {args.out}", flush=True)


if __name__ == "__main__":
    sys.exit(main())
