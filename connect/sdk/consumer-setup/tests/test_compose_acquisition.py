"""Compose acquisition uses real setup process/job control and retained outputs."""

import asyncio
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest

from test_selected_setup import adapter, backend

RUNTIME = r"""
import hashlib,json,pathlib,sys,time,signal,subprocess,os
p=lambda x:pathlib.Path(sys.argv[sys.argv.index(x)+1])
def wire(x):return (json.dumps(x,sort_keys=True,separators=(',',':'))+'\n').encode()
def sha(x):return hashlib.sha256(x).hexdigest()
work=p('--workspace');host=p('--host-profile').read_bytes();raw=sys.stdin.buffer.read();selected=json.loads(raw)
work.mkdir();(work/'entered').write_text('yes')
if selected.get('hang'):
 child=subprocess.Popen([sys.executable,'-c','import time; time.sleep(60)'],start_new_session=True)
 (work/'child.pid').write_text(str(child.pid))
 def stop(*_):
  child.terminate();child.wait();(work/'joined').write_text('yes');sys.exit(1)
 signal.signal(signal.SIGTERM,stop)
 time.sleep(60)
prepared=wire({'schema_version':1,'status':'prepared','plan_sha256':'a'*64,'plan':{}})
acquired=wire({'schema_version':1,'scope':'registry_image_acquisition_only','status':'observed','plan_sha256':'a'*64,'host_profile_sha256':sha(host),'selection_sha256':sha(raw)})
files={'host-profile.json':host,'selection.json':raw,'backend-profile.json':b'{}','inputs.json':b'{}','request.json':b'{}','prepared.json':prepared,'image-acquisition.json':acquired}
receipt={'schema_version':1,'status':'proposed','scope':'compose_plan_for_host_review','plan_sha256':'a'*64,'files':{k:sha(v) for k,v in files.items()}}
for k,v in files.items():(work/k).write_bytes(v)
if selected.get('corrupt'):(work/'inputs.json').write_bytes(b'changed')
(work/'proposal.json').write_bytes(wire(receipt));sys.stdout.buffer.write(wire(receipt))
"""


class ComposeAcquisitionTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name).resolve()
        self.stage = self.root / "stage"
        self.stage.mkdir()
        self.python = Path(sys.executable).resolve()
        self.controllers = []
        self.counter = 0
        runtime = self.root / "runtime.py"
        runtime.write_text(RUNTIME)
        planner = self.root / "planner"
        planner.write_text("inert pinned planner")
        planner.chmod(0o700)
        inner = self.root / "inner.json"
        inner.write_text("{}")
        self.host = {
            "schema_version": 1,
            "scope": "acquire_agent_selected_container_images",
            "staging_root": str(self.stage),
            "python_sha256": backend.digest(self.python.read_bytes()),
            "operation_timeout_seconds": 20,
        }
        for name, path in [
            ("runtime", runtime),
            ("planner", planner),
            ("host_profile", inner),
        ]:
            self.host[name] = {
                "path": str(path),
                "sha256": backend.digest(path.read_bytes()),
            }

    def tearDown(self):
        for owner in self.controllers:
            owner.close_admission()
            for job in list(owner.jobs.values()):
                if job["status"] == "running":
                    owner._job_controls[job["id"]]["requested"].set()
                    job["stop"] = {"status": "requested", "scope": "local_job_only"}
            owner.join()
        self.temp.cleanup()

    def controller(self, **kwargs):
        self.counter += 1
        profile = self.root / ("host%d.json" % self.counter)
        profile.write_bytes(backend.encoded(self.host))
        owner = backend.Controller(
            None,
            None,
            self.root / ("work%d" % self.counter),
            (),
            str(self.python),
            None,
            None,
            compose_acquisition_profile=profile,
            compose_acquisition_profile_sha256=backend.digest(profile.read_bytes()),
            **kwargs,
        )
        self.controllers.append(owner)
        return owner

    def request(self, **values):
        path = self.stage / "chosen"
        path.mkdir(exist_ok=True)
        raw = backend.encoded(values)
        (path / "selection.json").write_bytes(raw)
        return {
            "request_id": "1" * 32,
            "staged_id": "chosen",
            "selection_sha256": backend.digest(raw),
        }

    def test_real_worker_retains_receipt_and_replays_without_second_effect(self):
        owner = self.controller()
        request = self.request()
        job = owner.acquire_compose(request)["job"]
        owner.join()
        final = owner.acquire_compose(request)["job"]
        self.assertEqual(final["id"], job["id"])
        self.assertEqual(final["status"], "succeeded", final)
        result = final["result"]
        self.assertEqual(result["scope"], "compose_plan_for_host_review")
        proposal = Path(result["proposal_directory"])
        self.assertEqual(
            backend.digest((proposal / "proposal.json").read_bytes()),
            result["receipt_sha256"],
        )
        self.assertEqual(len(owner.jobs), 1)
        self.assertFalse(owner.install_intents)
        self.assertIsNone(owner.compose)
        self.assertFalse(owner.state()["compose_acquisition"]["execution_uncertain"])
        changed = {**request, "selection_sha256": "f" * 64}
        with self.assertRaisesRegex(backend.SetupError, "acquisition_request_changed"):
            owner.acquire_compose(changed)

    def test_changed_host_refuses_before_worker_entry(self):
        owner = self.controller()
        request = self.request()
        Path(self.host["runtime"]["path"]).write_text("changed")
        owner.acquire_compose(request)
        owner.join()
        job = owner.acquire_compose(request)["job"]
        self.assertEqual(job["error"], "compose_acquisition_host_changed")
        self.assertFalse(list(owner.workspace.glob("acquisitions/*/proposal")))

    def test_corrupt_retained_result_fails_and_blocks_fresh_attempt(self):
        owner = self.controller()
        request = self.request(corrupt=True)
        owner.acquire_compose(request)
        owner.join()
        self.assertEqual(
            owner.acquire_compose(request)["job"]["error"],
            "compose_acquisition_result_invalid",
        )
        self.assertTrue(owner.state()["compose_acquisition"]["execution_uncertain"])
        other = {**request, "request_id": "2" * 32}
        owner.acquire_compose(other)
        owner.join()
        self.assertEqual(
            owner.acquire_compose(other)["job"]["error"],
            "compose_acquisition_reopen_required",
        )
        self.assertEqual(len(list(owner.workspace.glob("acquisitions/*/proposal"))), 1)

    def test_stop_gracefully_joins_nested_child_and_keeps_uncertain_outcome(self):
        owner = self.controller()
        request = self.request(hang=True)
        job = owner.acquire_compose(request)["job"]
        proposal = owner.workspace / "acquisitions" / job["id"] / "proposal"
        deadline = time.monotonic() + 5
        while not (proposal / "child.pid").exists() and time.monotonic() < deadline:
            time.sleep(0.01)
        self.assertTrue((proposal / "child.pid").exists())
        owner.stop({"job_id": job["id"]})
        owner.join()
        final = owner.acquire_compose(request)["job"]
        self.assertEqual(final["status"], "unconfirmed")
        self.assertEqual(final["error"], "operation_stopped")
        self.assertEqual(final["stop"]["status"], "settled")
        self.assertTrue((proposal / "joined").exists())
        self.assertFalse((proposal / "proposal.json").exists())
        self.assertFalse(owner.operation_lock.locked())
        self.assertTrue(owner.compose_acquisition.execution_uncertain)

    def test_timeout_also_joins_nested_child(self):
        self.host["operation_timeout_seconds"] = 1
        owner = self.controller()
        request = self.request(hang=True)
        job = owner.acquire_compose(request)["job"]
        owner.join()
        proposal = owner.workspace / "acquisitions" / job["id"] / "proposal"
        self.assertTrue((proposal / "joined").exists())
        self.assertEqual(owner.acquire_compose(request)["job"]["status"], "unconfirmed")

    def test_grace_is_bounded_when_owned_process_ignores_term(self):
        owner = self.controller()
        script = self.root / "ignore-term.py"
        script.write_text(
            "import signal,time,os\n"
            "signal.signal(signal.SIGTERM,signal.SIG_IGN)\n"
            "print(os.getpid(),flush=True)\n"
            "time.sleep(60)\n"
        )
        started = time.monotonic()
        with self.assertRaisesRegex(backend.SetupError, "inspection_unavailable"):
            owner.runner.run_compose(
                [str(self.python), str(script)], timeout=0.5, termination_grace=0.1
            )
        self.assertLess(time.monotonic() - started, 3)

    def test_bad_hash_and_recovery_refuse_before_workspace(self):
        self.host["runtime"]["sha256"] = "0" * 64
        with self.assertRaisesRegex(
            backend.SetupError, "invalid_compose_acquisition_profile"
        ):
            self.controller()
        self.assertFalse((self.root / "work1").exists())

    def test_selection_path_cannot_escape_or_substitute_symlink(self):
        owner = self.controller()
        request = self.request()
        with self.assertRaisesRegex(
            backend.SetupError, "invalid_compose_acquisition_arguments"
        ):
            owner.acquire_compose({**request, "staged_id": "../chosen"})
        raw = (self.stage / "chosen/selection.json").read_bytes()
        (self.stage / "chosen/selection.json").unlink()
        target = self.root / "outside.json"
        target.write_bytes(raw)
        (self.stage / "chosen/selection.json").symlink_to(target)
        owner.acquire_compose(request)
        owner.join()
        self.assertEqual(owner.acquire_compose(request)["job"]["status"], "unconfirmed")
        self.assertFalse(list(owner.workspace.glob("acquisitions/*/proposal")))

    def test_actual_cli_flags_and_clean_eof(self):
        owner = self.controller()
        profile = owner.compose_acquisition.profile_path
        result = subprocess.run(
            [
                sys.executable,
                "-I",
                "-B",
                str(Path(adapter.__file__)),
                "--workspace",
                str(self.root / "cli"),
                "--families",
                "",
                "--python",
                str(self.python),
                "--compose-acquisition-profile",
                str(profile),
                "--compose-acquisition-profile-sha256",
                backend.digest(profile.read_bytes()),
            ],
            input=b"",
            capture_output=True,
            timeout=10,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, b"")

    def test_mcp_exposes_and_dispatches_optional_acquisition(self):
        from mcp import Client

        owner = self.controller()
        request = self.request()

        async def run():
            async with Client(
                adapter.create_server(adapter.SetupAdapter(owner))
            ) as client:
                tools = await client.list_tools()
                self.assertIn("setup_acquire_compose", [t.name for t in tools.tools])
                reply = await client.call_tool("setup_acquire_compose", request)
                self.assertFalse(reply.is_error, reply)
                await asyncio.to_thread(owner.join)
                self.assertEqual(
                    owner.acquire_compose(request)["job"]["status"], "succeeded"
                )

        asyncio.run(run())


if __name__ == "__main__":
    unittest.main()
