"""Pinned PairedHost management owners with synthetic git/build evidence.

No app main, gateway, provider, robot, real credentials or deployment is started.
Pairing, source parsing, role selection, prepare/submit/status, intent and vault
behavior comes from the complete immutable PairedHost import closure.
"""
import asyncio
import copy
import hashlib
from http.cookies import SimpleCookie
import json
import logging
import os
from pathlib import Path
import socket
import sys

HERE = Path(__file__).resolve().parent
SOURCE = Path(sys.argv[1]).resolve()
ROOT = Path(sys.argv[2]).resolve()
MANIFEST = json.loads((HERE / "source.json").read_text())
for name, record in MANIFEST["files"].items():
    assert hashlib.sha256((SOURCE / name).read_bytes()).hexdigest() == record["sha256"], name
sys.path.insert(0, str(SOURCE / "src"))
os.environ.update(PAIRED_HOST_DEV_CONSOLE_AUTH="1", PAIRED_HOST_CONSOLE_PAIRING="1",
                  PAIRED_HOST_CONSOLE_TRUSTED_NETWORKS="127.0.0.0/8",
                  PAIRED_HOST_EXTENSION_SECRET_ENTRY="1",
                  PAIRED_HOST_DEV_HARNESS_TOKEN="fixture-host-confirm-authority",
                  PAIRED_HOST_AUTH_SECRET="fixture-console-signing-key")
import structlog
structlog.configure(logger_factory=structlog.stdlib.LoggerFactory())
logging.disable(logging.CRITICAL)
import httpx
import tomli_w
import tomllib
import uvicorn
from cryptography.fernet import Fernet
import paired_host.app as app_mod
from paired_host import frontend_prepare as fp, frontend_sources as fs
from paired_host import frontend_change_requests as fcr
from paired_host.console_pairing import ConsolePairing
from paired_host.extension_secrets.service import IntentDependencies, IntentService, TerminalPresenter, INTENTS_PATH
from paired_host.frontend_setup import FrontendSurface
from paired_host.secrets import VaultStore

REPO = "https://github.com/fixture/paired-host-robot.git"
TAG = "v0.5.6-fixture"
COMMIT = "e" * 40
WHEEL = "d" * 64
POLICY = "hc-policy-v1:sha256:" + "a" * 64
SECRET = "fixture-role-secret-never-model-729afe16"
PYPROJECT = b'''[project]
name = "paired-host-ext-robot"
version = "0.5.1.dev2026100301"
[project.entry-points."paired_host.extensions"]
robot = "paired_host_ext_robot.extension:RobotExtension"
'''
RECIPE = json.dumps({"schema": 1, "wheel_project": "extension", "artifacts_hook": None},
                    sort_keys=True, separators=(",", ":")).encode()


class World:
    def __init__(self):
        self.data = ROOT / "data"
        self.source = ROOT / "selected-source"
        (self.source / "deployment").mkdir(parents=True)
        ledger = tomllib.loads((SOURCE / "deployment/extensions.toml").read_text())
        del ledger["ext"]["robot"]
        self.ledger = tomli_w.dumps(ledger).encode()
        (self.source / "deployment/extensions.toml").write_bytes(self.ledger)
        (self.source / "deployment/extension_sources.toml").write_bytes(
            (SOURCE / "deployment/extension_sources.toml").read_bytes())
        self.vault = VaultStore(self.data / "secrets/vault", Fernet.generate_key().decode(),
                                backups_dir=ROOT / "backups")
        self.store = fcr.ChangeRequestStore(fcr.store_root(self.data))
        self.git_calls = []
        self.git = fs.GitAccess(runner=self.git_run, tmp_parent=ROOT)
        self.env = fp.PrepareEnvironment(source_root=self.source,
            snapshot=lambda: fp.SnapshotIdentity("214", POLICY),
            secret_present=lambda name: False, tag_resolver=self.resolve,
            package_reader=fs.package_reader(self.git), tmp_parent=ROOT)
        self.service = IntentService(IntentDependencies(current=self.current,
            change_store=self.store, vault=lambda: self.vault, data_dir=self.data,
            presenter=TerminalPresenter(), host_label="paired-host-role-fixture"))
        self.surface = FrontendSurface(env_factory=lambda: self.env, git=self.git,
                                      change_store=self.store, secret_service=self.service)
        self.pairing = ConsolePairing(self.data)
        self.code = self.pairing.issue_terminal_code(label="copied-connect-process")
        self.canaries = {SECRET, self.code, os.environ["PAIRED_HOST_DEV_HARNESS_TOKEN"],
                         os.environ["PAIRED_HOST_AUTH_SECRET"]}
        routes = app_mod._frontend_routes(project_root=self.source, deploy_root=ROOT,
            bundle_path=ROOT / "unused-bundle", bundle_digest=POLICY,
            data_root=self.data, surface=self.surface)
        self.app = app_mod.build_public_app(rust_http_base_url="http://127.0.0.1:9",
            channel_routes=routes, console_pairing=self.pairing)
        self.requests = []
        self.submitted = []
        self.built = []
        self.committed = []

    def current(self):
        return {"activation_id": "214", "policy_digest": POLICY,
                "ledger_digest": fp.ledger_digest(self.ledger)}, self.ledger

    def resolve(self, repo, tag):
        assert (repo, tag) == (REPO, TAG)
        return COMMIT

    def git_run(self, argv, **kwargs):
        argv = list(argv)
        self.git_calls.append(argv)
        def ok(value=b""): return fs.GitResult(0, value, b"")
        if "ls-remote" in argv:
            assert REPO in argv
            return ok(f"{COMMIT}\trefs/tags/{TAG}\n".encode())
        if any(command in argv for command in ("init", "remote", "fetch")):
            return ok()
        blobs = {"1" * 40: PYPROJECT, "2" * 40: RECIPE}
        if "ls-tree" in argv:
            path = argv[-1]
            sha = {"extension/pyproject.toml": "1" * 40,
                   "paired-host-release-v1.json": "2" * 40}[path]
            return ok(f"100644 blob {sha}\t{path}\0".encode())
        assert "cat-file" in argv
        if "-t" in argv: return ok(b"commit\n")
        if "-s" in argv: return ok(str(len(blobs[argv[-1]])).encode())
        assert "blob" in argv
        return ok(blobs[argv[-1]])

    async def tracked(self, scope, receive, send):
        if scope["type"] != "http":
            return await self.app(scope, receive, send)
        status = None
        async def observed(message):
            nonlocal status
            if message["type"] == "http.response.start":
                status = message["status"]
                for key, value in message.get("headers", []):
                    if key.lower() == b"set-cookie":
                        cookies = SimpleCookie(); cookies.load(value.decode())
                        self.canaries.update(c.value for c in cookies.values())
            await send(message)
        await self.app(scope, receive, observed)
        self.requests.append({"method": scope["method"], "path": scope["path"], "status": status})

    def build(self):
        records = [self.store.get(digest) for digest in self.store.digests()]
        assert len(records) == 1
        record = records[0]
        digest = record["request_id"]
        change = record["change"]
        options = {"extension_id": "robot", "distribution": change["proposal"]["distribution"],
            "version": change["proposal"]["version"], "source_commit": COMMIT,
            "wheel_sha256": WHEEL, "release_evidence_sha256": "f" * 64,
            "package_path": "extension", "facades": change["proposal"]["roles"]["required_facades"],
            "secret_names": ["api_key=ROBOT_API_KEY"]}
        self.store.add_stage(digest, fcr.make_stage(change, "building", {}))
        built = fcr.make_stage(change, "built", options)
        self.store.add_stage(digest, built)
        self.built.append(copy.deepcopy(built))
        return {"request_id": digest, "built": built}

    async def complete(self, origin, intent_id):
        # A separate human-side device earns its own cookie, never the child's.
        transport = httpx.ASGITransport(app=self.tracked, client=("127.0.0.1", 31000))
        async with httpx.AsyncClient(transport=transport, base_url=origin,
                                    headers={"origin": origin}) as browser:
            code = self.pairing.issue_terminal_code(label="fixture-private-entry")
            self.canaries.add(code)
            response = await browser.post("/console/pair/terminal", json={"code": code})
            assert response.status_code == 200
            response = await browser.post(f"{INTENTS_PATH}/{intent_id}/entry",
                                          json={"value": SECRET, "overwrite": False})
            assert response.status_code == 202
        async with httpx.AsyncClient(transport=transport, base_url=origin,
            headers={"x-paired-host-dev-token": os.environ["PAIRED_HOST_DEV_HARNESS_TOKEN"]}) as terminal:
            description = await terminal.get(f"{INTENTS_PATH}/{intent_id}/confirm")
            assert description.status_code == 200
            body = description.json()
            response = await terminal.post(f"{INTENTS_PATH}/{intent_id}/confirm", json={
                "decision": "approve", "confirm_word": body["confirm_word"],
                "expected_fingerprint": body["fingerprint"]})
            assert response.status_code == 200, response.status_code
            assert response.json()["generation"] == 1
        assert self.vault.slot_value("ext/robot/api_key") == (SECRET, 1)
        self.committed.append(response.json())
        return response.json()

    def result(self):
        imported = {}
        for module in tuple(sys.modules.values()):
            path = getattr(module, "__file__", None)
            if path and Path(path).resolve().is_relative_to(SOURCE):
                path = Path(path).resolve(); name = str(path.relative_to(SOURCE))
                digest = hashlib.sha256(path.read_bytes()).hexdigest()
                assert digest == MANIFEST["files"][name]["sha256"]
                imported[name] = digest
        return {"requests": self.requests, "built": self.built, "committed": self.committed,
                "imported_owner_files": imported, "git_calls": len(self.git_calls),
                "native_runtime_started": False, "provider_started": False}


async def main():
    # Fixed lifecycle facts only. Pairing codes, origins and control bodies stay
    # on the private pipe and are never copied into diagnostic evidence.
    lifecycle = {"stage": "world", "failure": None, "cleanup_failure": None,
                 "server_started": False, "server_task_done": False}
    def observe():
        (ROOT / "lifecycle.json").write_text(json.dumps(lifecycle) + "\n")
    observe()
    world = None
    sock = None
    server = None
    task = None
    try:
        world = World()
        sock = socket.socket(); sock.bind(("127.0.0.1", 0)); sock.listen(); sock.setblocking(False)
        origin = f"http://127.0.0.1:{sock.getsockname()[1]}"
        # Uvicorn 0.40 auto-detects this bound async method as ASGI2.
        server = uvicorn.Server(uvicorn.Config(world.tracked, log_config=None, access_log=False,
            interface="asgi3", lifespan="on", timeout_graceful_shutdown=5))
        server.install_signal_handlers = lambda: None
        task = asyncio.create_task(server.serve(sockets=[sock]))
        lifecycle["stage"] = "server_starting"; observe()
        async with asyncio.timeout(15):
            while not server.started:
                if task.done():
                    await task
                    raise RuntimeError("fixture server ended before startup")
                await asyncio.sleep(0.005)
        lifecycle.update(stage="ready", server_started=True); observe()
        print(json.dumps({"origin": origin, "code": world.code}), flush=True)
        while True:
            line = await asyncio.to_thread(sys.stdin.buffer.readline)
            if not line: break
            command = json.loads(line)
            if command == "stop": break
            if command == "build": observation = world.build()
            elif command.startswith("complete:"):
                observation = await world.complete(origin, command.split(":", 1)[1])
            elif command == "observe": observation = world.result()
            else: raise ValueError("unsupported fixture control")
            print(json.dumps({"ack": command, "observation": observation}), flush=True)
    except BaseException as error:
        lifecycle["failure"] = type(error).__name__
        raise
    finally:
        if server is not None:
            server.should_exit = True
        if task is not None:
            try:
                async with asyncio.timeout(15): await task
            except BaseException as error:
                lifecycle["cleanup_failure"] = type(error).__name__
            lifecycle["server_task_done"] = task.done()
        if sock is not None:
            sock.close()
        observe()
        if world is not None:
            result = world.result(); result["server_task_done"] = lifecycle["server_task_done"]
            (ROOT / "result.json").write_text(json.dumps(result) + "\n")
            (ROOT / "private-canaries.json").write_text(json.dumps(sorted(world.canaries)) + "\n")
        if lifecycle["cleanup_failure"] is not None and lifecycle["failure"] is None:
            raise RuntimeError("fixture server cleanup failed")


if __name__ == "__main__":
    asyncio.run(main())
