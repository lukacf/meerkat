#!/usr/bin/env python3
"""Tiny, offline protocol tests. All gh calls use an executable fixture double."""

import contextlib
import copy
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import shutil
import stat
import subprocess
import sys
import tarfile
import unittest
from unittest import mock
import uuid
import zipfile


SCRIPT = Path(__file__).with_name("release-candidate.py")
SPEC = importlib.util.spec_from_file_location("release_candidate", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
helper = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(helper)
SHA = "a" * 40
OTHER_SHA = "b" * 40
VERSION = "0.8.31"
TAG = helper.TAG_PREFIX + VERSION
REPO = helper.REPOSITORY
RUN = 101
TRAILING_RUN = 102
ATTEMPT = 2
CI = 99
# The release commit's first parent, whose copy of the root's product-scoped
# CI evidence reader judges a failed exact-main CI run, and the commit's tree.
PARENT_SHA = "1" * 40
SOURCE_TREE = "2" * 40
EVIDENCE_READER = Path(__file__).resolve().parents[2] / "scripts" / "ci-product-evidence.mjs"
# The Core release the root Cargo.toml pins, and the tree of crates/ that
# both the source commit and Core's tag carry unless a test changes one.
CORE = "0.8.31"
CORE_TAG = f"refs/tags/v{CORE}"
CRATES_TREE = "c" * 40
# Core's crates as the checkout lists them, each one's checksum in the
# crates.io index, the toolchain's rustc -vV, and Meta's publish order.
CORE_CRATE_NAMES = ("meerkat-sqlite", "meerkat-core", "meerkat")
INDEX_CKSUM = {name: hashlib.sha256(name.encode()).hexdigest() for name in CORE_CRATE_NAMES}
TOOLCHAIN = "rustc 1.97.0 (fixture 2026-10-01)\nbinary: rustc\nhost: x86_64-unknown-linux-gnu"
META_ORDER = ("meta-extension-state", "meerkat-meta")
META_SHA256 = {"meta-extension-state": "e" * 64, "meerkat-meta": "f" * 64}


FAKE_GH = r'''
import hashlib
import json
import os
from pathlib import Path
import shutil
import sys

args = sys.argv[1:]
state_path = Path(os.environ["FAKE_GH_STATE"])
state = json.loads(state_path.read_text())
entry = {"args": args, "gh_token": "GH_TOKEN" in os.environ,
         "github_api_token": "GITHUB_API_TOKEN" in os.environ}
if args[:2] == ["workflow", "run"]:
    entry["inputs"] = json.loads(sys.stdin.read())
state["calls"].append(entry)
state_path.write_text(json.dumps(state))

def fail(message):
    print(message, file=sys.stderr)
    sys.exit(1)

def emit(value):
    print(json.dumps(value))

if "--clobber" in args or any(a in ("delete", "--delete", "--method", "-X") for a in args):
    fail("forbidden mutation")
if args[:2] == ["workflow", "run"]:
    sys.exit(0)
if args[:2] == ["attestation", "verify"]:
    if state.get("verify_failure"):
        fail("cryptographic signature verification failed")
    name = Path(args[2]).name
    if name not in state["verification"]:
        fail("missing original verified attestation fixture")
    emit(state["verification"][name])
elif args[0] == "api":
    endpoint = args[1]
    if "/actions/artifacts/" in endpoint and endpoint.endswith("/zip"):
        if args != ["api", endpoint]:
            fail("Actions ZIP API requires default JSON negotiation before redirect")
    elif "/releases/assets/" in endpoint:
        if args != ["api", endpoint, "--header", "Accept: application/octet-stream"]:
            fail("release asset byte downloads require octet-stream")
    if "/git/ref/tags/" in endpoint and state.get("move_tag_at"):
        reads = sum(c["args"][:2] == ["api", endpoint] for c in state["calls"])
        if reads >= state["move_tag_at"]:
            state["api"][endpoint]["object"]["sha"] = "b" * 40
            state_path.write_text(json.dumps(state))
    script = state.get("api_script", {}).get(endpoint)
    if script:
        step = script.pop(0)
        if "update" in step:
            state["api"][endpoint].update(step["update"])
        state_path.write_text(json.dumps(state))
        if "fail" in step:
            fail(step["fail"])
    if endpoint in state.get("fail_endpoints", []):
        sys.stdout.write(state.get("download_error_body", ""))
        fail("fixture API failure (HTTP 403)")
    if endpoint in state["api"]:
        value = state["api"][endpoint]
        if isinstance(value, dict) and "bytes_file" in value:
            sys.stdout.buffer.write(Path(value["bytes_file"]).read_bytes())
        else:
            emit(value)
    elif endpoint.endswith("/releases?per_page=100"):
        if state.get("release") and state.get("hidden_new_draft_reads", 0):
            state["hidden_new_draft_reads"] -= 1
            state_path.write_text(json.dumps(state))
            emit([[]])
        else:
            emit([[state["release"]]] if state.get("release") else [[]])
    elif "/releases/" in endpoint and endpoint.endswith("/assets?per_page=100"):
        emit([state["release"]["assets"]])
    elif "/releases/assets/" in endpoint:
        aid = int(endpoint.rsplit("/", 1)[1])
        asset = next(a for a in state["release"]["assets"] if a["id"] == aid)
        if state.get("count_downloads"):
            asset["download_count"] = asset.get("download_count", 0) + 1
            state_path.write_text(json.dumps(state))
        sys.stdout.buffer.write(Path(asset["local_file"]).read_bytes())
    else:
        fail("unhandled API endpoint: " + endpoint)
elif args[:2] == ["release", "create"]:
    if state.get("release"):
        fail("release already exists")
    if "--verify-tag" not in args or "--draft" not in args or "--latest=false" not in args:
        fail("unsafe release creation")
    state["release"] = {"id": 800, "tag_name": args[2], "draft": True,
                        "name": args[2], "body": "original metadata", "assets": []}
    state["hidden_new_draft_reads"] = state.get("new_draft_visibility_delay", 0)
    state_path.write_text(json.dumps(state))
elif args[:2] == ["release", "upload"]:
    path = Path(args[3])
    release = state["release"]
    if any(a["name"] == path.name for a in release["assets"]):
        fail("no-clobber upload collision")
    if state.get("fail_upload") == path.name:
        fail("upload interrupted")
    destination = state_path.parent / ("uploaded-" + str(len(release["assets"])))
    shutil.copyfile(path, destination)
    if state.get("corrupt_upload") == path.name:
        destination.write_bytes(b"server-side upload corruption")
    content = destination.read_bytes()
    release["assets"].append({"id": 1000 + len(release["assets"]), "name": path.name,
        "size": len(content), "state": "uploaded", "digest": "sha256:" + hashlib.sha256(content).hexdigest(),
        "local_file": str(destination)})
    state_path.write_text(json.dumps(state))
elif args[:2] == ["release", "edit"]:
    if args[3:] != ["--repo", "lukacf/meerkat", "--draft=false", "--latest=false"]:
        fail("metadata mutation forbidden")
    state["release"]["draft"] = False
    state_path.write_text(json.dumps(state))
else:
    fail("unhandled gh command: " + repr(args))
'''


def archive_bytes(member_name, content, extension):
    output = io.BytesIO()
    if extension == "zip":
        with zipfile.ZipFile(output, "w") as archive:
            archive.writestr(member_name, content)
    else:
        with tarfile.open(fileobj=output, mode="w:gz") as archive:
            info = tarfile.TarInfo(member_name)
            info.size = len(content)
            archive.addfile(info, io.BytesIO(content))
    return output.getvalue()


def verified_fixture(name, file_hash, *, sha=SHA, run_id=RUN, attempt=ATTEMPT,
                     ref=helper.MAIN, event="workflow_dispatch"):
    repo_url = "https://github.com/" + REPO
    identity = f"{repo_url}/{helper.WORKFLOW}@{ref}"
    invocation = f"{repo_url}/actions/runs/{run_id}/attempts/{attempt}"
    return [{
        "verificationResult": {
            "signature": {"certificate": {
                "issuer": "https://token.actions.githubusercontent.com",
                "subjectAlternativeName": identity, "buildSignerURI": identity,
                "buildSignerDigest": sha, "sourceRepositoryURI": repo_url,
                "sourceRepositoryDigest": sha, "sourceRepositoryRef": ref,
                "buildConfigURI": identity, "buildConfigDigest": sha,
                "runnerEnvironment": "github-hosted", "buildTrigger": event,
                "runInvocationURI": invocation,
            }},
            "verifiedTimestamps": [{"type": "Tlog", "timestamp": "2026-09-05T00:00:00Z"}],
            "statement": {
                "_type": "https://in-toto.io/Statement/v1", "predicateType": helper.SLSA,
                "subject": [{"name": name, "digest": {"sha256": file_hash}}],
                "predicate": {
                    "buildDefinition": {
                        "buildType": "https://actions.github.io/buildtypes/workflow/v1",
                        "externalParameters": {"workflow": {
                            "repository": repo_url, "path": helper.WORKFLOW, "ref": ref}},
                        "internalParameters": {"github": {
                            "event_name": event, "runner_environment": "github-hosted"}},
                        "resolvedDependencies": [{"uri": f"git+{repo_url}@{ref}", "digest": {"gitCommit": sha}}],
                    },
                    "runDetails": {"builder": {"id": identity}, "metadata": {"invocationId": invocation}},
                },
            },
        },
    }]


class CandidateProtocolTests(unittest.TestCase):
    def setUp(self):
        self.root = Path.cwd() / (".release-candidate-test-" + uuid.uuid4().hex)
        self.root.mkdir()
        # Registered at once: the directory goes even when the rest of setUp
        # fails.
        self.addCleanup(shutil.rmtree, self.root, True)
        bin_dir = self.root / "bin"
        bin_dir.mkdir()
        gh = bin_dir / "gh"
        gh.write_text(f"#!{sys.executable}\n" + FAKE_GH)
        gh.chmod(0o755)
        self.state_path = self.root / "gh-state.json"
        self.state = {"api": {}, "verification": {}, "calls": [], "release": None}
        self.environment = {
            "PATH": str(bin_dir) + os.pathsep + os.environ["PATH"],
            "FAKE_GH_STATE": str(self.state_path),
            "GITHUB_REPOSITORY": REPO, "GITHUB_RUN_ID": str(RUN),
            "GITHUB_RUN_ATTEMPT": str(ATTEMPT), "GITHUB_REF": helper.MAIN,
            "GITHUB_SHA": SHA, "GITHUB_WORKFLOW_SHA": SHA, "GITHUB_EVENT_NAME": "workflow_dispatch",
            "RELEASE_MODE": "candidate", "SOURCE_SHA": SHA,
            "RELEASE_SHA": SHA, "RELEASE_VERSION": VERSION, "RELEASE_TAG": TAG,
            "ARTIFACT_SELECTION": "", "ARTIFACT_SELECTION_FILE": "",
            "PUBLISH_RELEASE_PACKAGES": "false", "REGISTRY_DRY_RUN": "false",
            "GITHUB_OUTPUT": "", "GITHUB_STEP_SUMMARY": "",
            "GH_TOKEN": "fixture-workflow-token", "GITHUB_API_TOKEN": "fixture-local-token",
        }
        self.env_patch = mock.patch.dict(os.environ, self.environment)
        self.env_patch.start()
        self.protocol = True
        # git as the release jobs' checkouts answer it: the crates/ trees by
        # revision, the root manifest's Core pin, and whether origin has
        # Core's tag. Every git call is recorded in order.
        self.crates_trees = {f"{SHA}:crates": CRATES_TREE, f"{CORE_TAG}^{{commit}}:crates": CRATES_TREE}
        self.core_pin = f'"={CORE}"'
        self.core_tag_on_origin = True
        self.git_calls = []
        # The crates.io sparse index by crate (a crate absent here reads as
        # HTTP 404), crates whose read fails, and the toolchain.
        self.index = {name: [{"name": name, "vers": "0.8.30", "cksum": "0" * 64, "yanked": False},
                             {"name": name, "vers": CORE, "cksum": INDEX_CKSUM[name], "yanked": False}]
                      for name in CORE_CRATE_NAMES}
        self.unreadable = set()
        self.toolchain = TOOLCHAIN
        # The release commit's first parent holds the CI evidence reader, and
        # the commit changes these paths against it.
        self.parent_reader = True
        self.changed_paths = ("meta/crates/meerkat-meta/src/lib.rs",)
        # Root build inputs whose id differs from the default, by "REV:PATH".
        self.root_inputs = {}
        for name, effect in (("index_entries", self.index_response),
                             ("core_crate_list", lambda: list(CORE_CRATE_NAMES)),
                             ("rustc_version", lambda: self.toolchain)):
            # An older release-candidate.py may lack one: only the tests of
            # what it lacks fail then, so the suite still shows which.
            if not hasattr(helper, name):
                continue
            patch = mock.patch.object(helper, name, side_effect=effect)
            patch.start()
            self.addCleanup(patch.stop)
        self.git_patch = mock.patch.object(helper, "git", side_effect=self.git_response)
        self.git_patch.start()
        self.addCleanup(self.env_patch.stop)
        self.addCleanup(self.git_patch.stop)
        self.state["api"][helper.repo_endpoint(f"git/ref/tags/{TAG}")] = {
            "object": {"type": "commit", "sha": SHA}}
        self.state["api"][helper.repo_endpoint(f"actions/runs/{RUN}")] = self.run_info()
        self.state["api"][helper.repo_endpoint(f"actions/runs/{CI}")] = self.run_info(ci=True)
        self.target_files = {}
        self.artifact_ids = []
        listing = []
        for offset, (target, extension) in enumerate(helper.assets.TARGET_ARCHIVES):
            aid = 201 + offset
            folder = self.root / target
            folder.mkdir()
            names = []
            for prefix, binary in helper.BINARY_NAMES.items():
                name = f"{prefix}-{VERSION}-{target}.{extension}"
                inner = f"target/{target}/release-meta/{binary}.exe" if extension == "zip" else binary
                content = f"original-signed-{binary}-{target}".encode()
                path = folder / name
                path.write_bytes(archive_bytes(inner, content, extension))
                names.append(name)
                self.target_files[name] = path
                self.state["verification"][name] = verified_fixture(name, helper.assets.sha256(path))
            (folder / helper.PROVENANCE).write_text('{"original":"signed bundle fixture"}\n')
            packed = self.root / f"artifact-{aid}.zip"
            self.pack(packed, folder, [*names, helper.PROVENANCE])
            metadata = self.add_artifact(aid, f"meta-gateway-binaries-{target}-{ATTEMPT}", packed)
            listing.append(metadata)
            self.artifact_ids.append(aid)
        self.state["api"][helper.repo_endpoint(f"actions/runs/{RUN}/artifacts?per_page=100")] = [
            {"artifacts": listing, "total_count": 5}]
        self.save()

    def git_response(self, *args):
        self.git_calls.append(args)
        if args == ("rev-parse", "--verify", f"{SHA}^1"):
            return PARENT_SHA
        if args == ("rev-parse", "--verify", f"{SHA}^{{tree}}"):
            return SOURCE_TREE
        if args == ("show", f"{PARENT_SHA}:scripts/ci-product-evidence.mjs"):
            if not self.parent_reader:
                raise helper.ProtocolError("git show failed: fatal: path 'scripts/ci-product-evidence.mjs' "
                                           f"does not exist in '{PARENT_SHA}'")
            return EVIDENCE_READER.read_text().strip()
        if args == ("diff", "--name-only", PARENT_SHA, SHA):
            return "\n".join(self.changed_paths)
        if args[:3] == ("rev-parse", "--verify", "--quiet"):
            # A root build input's id: the same at every revision unless a
            # test says otherwise ("" when the revision has no such path).
            return self.root_inputs.get(args[3], "root-input:" + args[3].split(":", 1)[1])
        if args[:2] == ("rev-parse", "--verify") and args[2].endswith(":crates"):
            if args[2] not in self.crates_trees:
                raise helper.ProtocolError(f"git rev-parse failed: fatal: invalid object name '{args[2]}'")
            return self.crates_trees[args[2]]
        if args[:2] == ("rev-parse", "HEAD"):
            return SHA
        if args[:2] == ("rev-parse", "--verify"):
            return SHA
        if args == ("show", f"{SHA}:Cargo.toml"):
            return f'[workspace.dependencies]\nmeerkat-core = {{ version = {self.core_pin}, path = "crates/meerkat-core" }}\n'
        if args == ("show", f"{SHA}:ci/products.json"):
            return json.dumps({"products": [{"name": "core", "root": ""},
                                            {"name": "meta", "root": "meta", "publish_order": list(META_ORDER)}]})
        if args[0] == "show":
            return f'[package]\nname = "meerkat-meta"\nversion = "{VERSION}"\n'
        if args[0] == "fetch":
            self.assertEqual(args, ("fetch", "--no-tags", "origin", f"+{CORE_TAG}:{CORE_TAG}"))
            if not self.core_tag_on_origin:
                raise helper.ProtocolError(f"git fetch failed: fatal: couldn't find remote ref {CORE_TAG}")
            return ""
        if args[0] == "ls-tree":
            return "meta/scripts/release-candidate.py\n" if self.protocol else "Cargo.toml\n"
        self.fail(f"unexpected git command: {args}")

    def index_response(self, name):
        if name in self.unreadable or name not in self.index:
            raise helper.ProtocolError(f"crates.io index read for {name} failed: HTTP Error 404: Not Found")
        return copy.deepcopy(self.index[name])

    def fresh(self, label):
        return self.root / f"{label}-{uuid.uuid4().hex[:8]}"

    def step_files(self):
        """Point GITHUB_OUTPUT and GITHUB_STEP_SUMMARY at new files."""
        os.environ.update(GITHUB_OUTPUT=str(self.fresh("output")), GITHUB_STEP_SUMMARY=str(self.fresh("summary")))

    def outputs(self):
        path = Path(os.environ["GITHUB_OUTPUT"])
        return dict(line.split("=", 1) for line in path.read_text().splitlines()) if path.exists() else {}

    def summary(self):
        path = Path(os.environ["GITHUB_STEP_SUMMARY"])
        return path.read_text() if path.exists() else ""

    def floor_dir(self, *, stage="build", problems=(), dependencies=("meerkat 0.8.31", "meerkat-core 0.8.31"),
                  lock=b"# resolved lock\n", lock_sha256=None, resolution=False):
        """A floor check's --out directory: floor.json and the scratch
        workspace's resolved lock (stage resolve for a --no-build run)."""
        folder = self.fresh("floor")
        (folder / "ws").mkdir(parents=True)
        (folder / "ws" / "Cargo.lock").write_bytes(lock)
        report = {"product": "meta", "mode": "release", "crates": list(META_ORDER),
                  "stage": "resolve" if resolution else stage, "problems": list(problems),
                  "registry_dependencies": sorted(dependencies),
                  "lock_sha256": lock_sha256 or hashlib.sha256(lock).hexdigest()}
        (folder / "floor.json").write_text(json.dumps(report))
        return folder

    def floor_bind(self, folder):
        """floor-bind in candidate mode; returns the record it wrote (None
        when it bound none)."""
        os.environ["RELEASE_MODE"] = "candidate"
        self.step_files()
        self.command("floor-bind", "--floor-dir", str(folder))
        record = self.outputs()["record"]
        return json.loads(record) if record else None

    def floor_args(self, record, checksums=META_SHA256):
        args = ["--floor-record-json", json.dumps(record)]
        for name, value in checksums.items():
            args += ["--meta-crate-sha256", f"{name}={value}"]
        return args

    def bound_record(self):
        """The record a core candidate binds once its floor build succeeded."""
        return {**self.floor_bind(self.floor_dir()), "meta_crates": dict(META_SHA256)}

    def run_info(self, *, ci=False):
        return {"id": CI if ci else RUN, "run_attempt": 1 if ci else ATTEMPT,
                "repository": {"full_name": REPO, "id": 42},
                "head_repository": {"full_name": REPO, "id": 42},
                "head_sha": SHA, "head_branch": "main",
                "event": "push" if ci else "workflow_dispatch",
                "path": ".github/workflows/ci.yml" if ci else helper.WORKFLOW,
                "status": "completed" if ci else "in_progress",
                "conclusion": "success" if ci else None}

    def pack(self, destination, folder, names):
        with zipfile.ZipFile(destination, "w") as archive:
            for name in names:
                archive.write(folder / name, name)

    def add_artifact(self, aid, name, packed, run_id=RUN):
        metadata = {
            "id": aid, "name": name, "expired": False, "digest": "sha256:" + helper.assets.sha256(packed),
            "workflow_run": {"id": run_id, "head_sha": SHA, "repository_id": 42, "head_repository_id": 42},
        }
        self.state["api"][helper.repo_endpoint(f"actions/artifacts/{aid}")] = metadata
        self.state["api"][helper.repo_endpoint(f"actions/artifacts/{aid}/zip")] = {"bytes_file": str(packed)}
        return metadata

    def save(self):
        self.state_path.write_text(json.dumps(self.state))

    def load(self):
        self.state = json.loads(self.state_path.read_text())
        return self.state

    def command(self, *argv, success=True):
        stdout, stderr = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
            code = helper.main(list(argv))
        self.load()
        if success:
            self.assertEqual(code, 0, stderr.getvalue())
        else:
            self.assertEqual(code, 1, stdout.getvalue())
        return stdout.getvalue(), stderr.getvalue()

    def writes(self):
        return [c for c in self.state["calls"] if c["args"][0] == "release"]

    def candidate(self):
        directory = self.root / "candidate"
        self.command("collect", "--output-dir", str(directory), "--ci-run-id", str(CI), "--ci-run-attempt", "1")
        manifest = helper.read_json(directory / helper.MANIFEST)
        self.state["verification"][helper.MANIFEST] = verified_fixture(
            helper.MANIFEST, helper.assets.sha256(directory / helper.MANIFEST))
        (directory / helper.PROVENANCE).write_text('{"original":"manifest attestation fixture"}\n')
        packed = self.root / "candidate.zip"
        self.pack(packed, directory, [helper.MANIFEST, helper.PROVENANCE, "index.json", "checksums.sha256"])
        metadata = self.add_artifact(300, f"meta-candidate-manifest-{ATTEMPT}", packed)
        self.state["api"][helper.repo_endpoint(f"actions/runs/{RUN}")].update(status="completed", conclusion="success")
        self.save()
        stdout, _ = self.command("selection", "--manifest-file", str(directory / helper.MANIFEST),
                                "--artifact-id", "300", "--artifact-digest", metadata["digest"][7:])
        selected = json.loads(stdout)
        os.environ.update(RELEASE_MODE="promote", SOURCE_SHA="", ARTIFACT_SELECTION=json.dumps(selected))
        return selected, manifest

    def staged(self):
        selected, manifest = self.candidate()
        self.command("stage", "--output-dir", str(self.root / "release-assets"),
                     "--proof-dir", str(self.root / "release-proofs"))
        return selected, manifest

    def publish(self, *, success=True):
        return self.command("publish", "--assets-dir", str(self.root / "release-assets"),
                            "--proof-dir", str(self.root / "release-proofs"), success=success)

    def seed_release(self, paths, *, draft=False):
        self.state["release"] = {"id": 800, "tag_name": TAG, "draft": draft,
                                 "name": "old title", "body": "old immutable metadata", "assets": []}
        for path in paths:
            content = path.read_bytes()
            self.state["release"]["assets"].append({
                "id": 900 + len(self.state["release"]["assets"]), "name": path.name,
                "size": len(content), "digest": "sha256:" + hashlib.sha256(content).hexdigest(),
                "state": "uploaded", "local_file": str(path),
            })
        self.save()

    def tier_run(self, tier, run_id):
        """A tiered candidate run in progress whose artifact listing holds
        only that tier's targets, each archive attested by that run."""
        run = dict(self.run_info(), id=run_id)
        self.state["api"][helper.repo_endpoint(f"actions/runs/{run_id}")] = run
        listing = []
        for offset, target in enumerate(helper.assets.TIER_TARGETS[tier]):
            extension = dict(helper.assets.TARGET_ARCHIVES)[target]
            folder = self.root / f"run-{run_id}-{target}"
            folder.mkdir()
            names = [f"{prefix}-{VERSION}-{target}.{extension}" for prefix in helper.BINARY_NAMES]
            for name in names:
                shutil.copyfile(self.target_files[name], folder / name)
                self.state["verification"][name] = verified_fixture(
                    name, helper.assets.sha256(folder / name), run_id=run_id)
            (folder / helper.PROVENANCE).write_text(f'{{"run":{run_id},"target":"{target}"}}\n')
            packed = self.root / f"artifact-{run_id}-{target}.zip"
            self.pack(packed, folder, [*names, helper.PROVENANCE])
            listing.append(self.add_artifact(run_id * 10 + offset, f"meta-gateway-binaries-{target}-{ATTEMPT}",
                                             packed, run_id))
        self.state["api"][helper.repo_endpoint(f"actions/runs/{run_id}/artifacts?per_page=100")] = [
            {"artifacts": listing, "total_count": len(listing)}]
        self.save()

    def tier_candidate(self, tier, run_id, manifest_aid, *, schema=None, collect_args=()):
        """A tiered candidate run that built and binds exactly one tier.

        The run's artifact listing holds only that tier's targets, each
        archive attested by that run; collection, the manifest artifact and
        the emitted selection follow meta-release.yml's candidate jobs.
        schema=2 or 3 rewrites the collected manifest as an earlier workflow
        wrote it, before it is attested and selected; collect_args are the
        floor record arguments meta-release.yml hands collection.
        """
        self.tier_run(tier, run_id)
        directory = self.root / f"candidate-{tier}-{run_id}"
        with mock.patch.dict(os.environ, {"GITHUB_RUN_ID": str(run_id), "RELEASE_MODE": "candidate",
                                          "RELEASE_TIER": tier}):
            self.command("collect", "--output-dir", str(directory), "--ci-run-id", str(CI),
                         "--ci-run-attempt", "1", "--tier", tier, *collect_args)
        _, manifest_name, _ = helper.tier_proof_names(tier)
        manifest = helper.read_json(directory / manifest_name)
        if schema in (2, 3):
            dropped = ("floor_record",) if schema == 3 else ("floor_record", "core_crates_tree")
            manifest = {key: value for key, value in manifest.items() if key not in dropped}
            manifest["schema_version"] = schema
            (directory / manifest_name).write_bytes(helper.json_bytes(manifest))
        self.state["verification"][manifest_name] = verified_fixture(
            manifest_name, helper.assets.sha256(directory / manifest_name), run_id=run_id)
        (directory / helper.PROVENANCE).write_text(f'{{"manifest":{run_id}}}\n')
        packed = self.root / f"candidate-{tier}-{run_id}.zip"
        self.pack(packed, directory, [manifest_name, helper.PROVENANCE, *helper.assets.metadata_names(tier)])
        metadata = self.add_artifact(manifest_aid, f"meta-candidate-manifest-{ATTEMPT}", packed, run_id)
        self.state["api"][helper.repo_endpoint(f"actions/runs/{run_id}")].update(
            status="completed", conclusion="success")
        self.save()
        stdout, _ = self.command("selection", "--manifest-file", str(directory / manifest_name),
                                 "--artifact-id", str(manifest_aid), "--artifact-digest", metadata["digest"][7:])
        return json.loads(stdout), manifest

    def promote_core(self):
        """Collect, accept, stage and publish the schema 2 core tier."""
        core, manifest = self.tier_candidate("core", RUN, 300)
        os.environ.update(RELEASE_MODE="promote", SOURCE_SHA="", ARTIFACT_SELECTION=json.dumps(core))
        self.command("stage", "--output-dir", str(self.root / "release-assets"),
                     "--proof-dir", str(self.root / "release-proofs"))
        self.publish()
        return core, manifest

    def assets_mode(self, selected):
        os.environ.update(RELEASE_MODE="assets", SOURCE_SHA="", ARTIFACT_SELECTION=json.dumps(selected),
                          PUBLISH_RELEASE_PACKAGES="false", REGISTRY_DRY_RUN="false")

    def release_files(self):
        return {asset["name"]: (asset["id"], Path(asset["local_file"]).read_bytes())
                for asset in self.state["release"]["assets"]}

    def test_schema2_core_promotion_publishes_only_the_core_tier(self):
        core, manifest = self.promote_core()
        self.assertEqual((core["schema_version"], core["tier"]), (helper.TIERED_SCHEMA, "core"))
        self.assertEqual((manifest["schema_version"], manifest["tier"]), (helper.TIERED_SCHEMA, "core"))
        self.assertEqual({item["target"] for item in manifest["artifacts"]},
                         set(helper.assets.TIER_TARGETS["core"]))
        self.assertFalse(self.state["release"]["draft"])
        self.assertEqual(set(self.release_files()), helper.tier_file_names(VERSION, "core"))
        index = json.loads(self.release_files()["index.json"][1])
        self.assertEqual(index["tier"], "core")
        self.assertEqual(index["artifacts"], helper.assets.expected_archive_names(VERSION, "core"))
        # Verification and registry retry need the core tier only.
        self.state["calls"] = []
        self.save()
        os.environ.update(RELEASE_MODE="existing", ARTIFACT_SELECTION="")
        stdout, _ = self.command("verify-published")
        self.assertIn("trailing tier not added yet", stdout)
        self.assertFalse(self.writes())

    def test_trailing_tier_is_added_add_only_and_bound_to_the_published_core_receipt(self):
        self.promote_core()
        before = self.release_files()
        trailing, manifest = self.tier_candidate("trailing", TRAILING_RUN, 301)
        self.assertEqual((trailing["schema_version"], trailing["tier"]), (helper.TIERED_SCHEMA, "trailing"))
        self.state["calls"] = []
        self.save()
        self.assets_mode(trailing)
        self.command("recover")
        after = self.release_files()
        self.assertEqual(set(after), set(before) | helper.tier_file_names(VERSION, "trailing"))
        for name, (asset_id, content) in before.items():
            self.assertEqual(after[name], (asset_id, content), name)
        uploads = [Path(call["args"][3]).name for call in self.writes()]
        self.assertTrue(all(call["args"][1] == "upload" for call in self.writes()))
        self.assertEqual(uploads[0], "candidate-selection-trailing.json")
        self.assertEqual(set(uploads[-2:]), {"index-trailing.json", "checksums-trailing.sha256"})
        # Each file only after everything that verifies it.
        position = uploads.index
        self.assertLess(position("manifest-provenance-trailing.jsonl"), position("candidate-manifest-trailing.json"))
        bundles = [position(f"target-{t}-provenance.jsonl") for t in helper.assets.TIER_TARGETS["trailing"]]
        archives = [position(name) for name in helper.assets.expected_archive_names(VERSION, "trailing")]
        self.assertLess(position("candidate-manifest-trailing.json"), min(bundles))
        self.assertLess(max(bundles), min(archives))
        index = json.loads(after["index-trailing.json"][1])
        self.assertEqual((index["tier"], index["artifacts"]),
                         ("trailing", helper.assets.expected_archive_names(VERSION, "trailing")))
        # A retry adds nothing, and verification now covers both tiers.
        self.state["calls"] = []
        self.save()
        self.command("recover")
        self.assertFalse(self.writes())
        os.environ.update(RELEASE_MODE="existing", ARTIFACT_SELECTION="")
        stdout, _ = self.command("verify-published")
        self.assertIn("verified the trailing tier", stdout)

    def test_tier_mismatch_refuses(self):
        # The collection step's tier must be the resolver's.
        with mock.patch.dict(os.environ, {"RELEASE_TIER": "trailing"}):
            _, error = self.command("collect", "--output-dir", str(self.root / "mismatch"),
                                    "--ci-run-id", str(CI), "--ci-run-attempt", "1", "--tier", "core",
                                    success=False)
        self.assertIn("collection tier differs", error)
        # A core collection refuses a run that also built trailing targets.
        with mock.patch.dict(os.environ, {"RELEASE_TIER": "core"}):
            _, error = self.command("collect", "--output-dir", str(self.root / "mixed"),
                                    "--ci-run-id", str(CI), "--ci-run-attempt", "1", "--tier", "core",
                                    success=False)
        self.assertIn("unexpected target artifacts", error)
        core, _ = self.tier_candidate("core", RUN, 300)
        # A selection claiming another tier than its attested manifest.
        relabelled = dict(core, tier="trailing")
        os.environ.update(RELEASE_MODE="assets", ARTIFACT_SELECTION=json.dumps(relabelled))
        _, error = self.command("stage", "--output-dir", str(self.root / "a1"),
                                "--proof-dir", str(self.root / "p1"), success=False)
        self.assertIn("schema/tier mismatch", error)
        # Promotion refuses a trailing selection before reading any artifact.
        trailing, _ = self.tier_candidate("trailing", TRAILING_RUN, 301)
        os.environ.update(RELEASE_MODE="promote", ARTIFACT_SELECTION=json.dumps(trailing))
        self.state["calls"] = []
        self.save()
        _, error = self.command("stage", "--output-dir", str(self.root / "a2"),
                                "--proof-dir", str(self.root / "p2"), success=False)
        self.assertIn("promotion accepts only a core selection", error)
        self.assertFalse(self.state["calls"])
        self.assertFalse(self.writes())

    def test_trailing_without_a_published_core_receipt_refuses_before_writes(self):
        trailing, _ = self.tier_candidate("trailing", TRAILING_RUN, 301)
        self.assets_mode(trailing)
        # No release at all.
        _, error = self.command("recover", success=False)
        self.assertIn("cannot create a release", error)
        # A release without the core receipt.
        core_archive = self.target_files[f"meta-gateway-{VERSION}-x86_64-unknown-linux-gnu.tar.gz"]
        self.seed_release([core_archive])
        _, error = self.command("recover", success=False)
        self.assertIn("requires the published core receipt", error)
        # A draft (promotion not finished) is not a published core.
        self.seed_release([core_archive], draft=True)
        _, error = self.command("recover", success=False)
        self.assertIn("published release only", error)
        self.assertFalse(self.writes())

    def test_trailing_tier_refuses_a_schema_1_release(self):
        # A schema 1 receipt binds all five targets already, not a core tier.
        self.staged()
        self.publish()
        trailing, _ = self.tier_candidate("trailing", TRAILING_RUN, 301)
        self.assets_mode(trailing)
        self.state["calls"] = []
        self.save()
        _, error = self.command("recover", success=False)
        self.assertIn("schema 2 core receipt", error)
        self.assertFalse(self.writes())

    def test_trailing_name_collision_with_different_bytes_refuses_before_writes(self):
        self.promote_core()
        trailing, _ = self.tier_candidate("trailing", TRAILING_RUN, 301)
        squatter = self.root / f"meta-gateway-{VERSION}-x86_64-pc-windows-msvc.zip"
        squatter.write_bytes(b"not the attested trailing archive")
        self.state["release"]["assets"].append({
            "id": 990, "name": squatter.name, "size": squatter.stat().st_size, "state": "uploaded",
            "digest": "sha256:" + helper.assets.sha256(squatter), "local_file": str(squatter)})
        self.state["calls"] = []
        self.save()
        before = self.release_files()
        self.assets_mode(trailing)
        _, error = self.command("recover", success=False)
        self.assertIn(f"existing release asset conflict: {squatter.name}", error)
        self.assertFalse(self.writes())
        self.assertEqual(self.release_files(), before)

    def test_partial_trailing_tier_never_blocks_core_verification(self):
        self.promote_core()
        trailing, _ = self.tier_candidate("trailing", TRAILING_RUN, 301)
        self.assets_mode(trailing)
        self.state["fail_upload"] = "meta-gateway-0.8.31-x86_64-apple-darwin.tar.gz"
        self.save()
        self.command("recover", success=False)
        self.assertIn("candidate-selection-trailing.json", self.release_files())
        os.environ.update(RELEASE_MODE="existing", ARTIFACT_SELECTION="")
        stdout, _ = self.command("verify-published")
        self.assertIn("trailing tier incomplete", stdout)
        # Once complete, a tampered trailing archive fails verification.
        self.state.pop("fail_upload")
        self.save()
        self.assets_mode(trailing)
        self.command("recover")
        archive = next(a for a in self.state["release"]["assets"] if a["name"].endswith("windows-msvc.zip"))
        Path(archive["local_file"]).write_bytes(b"tampered after publication")
        archive["digest"] = "sha256:" + helper.assets.sha256(Path(archive["local_file"]))
        archive["size"] = Path(archive["local_file"]).stat().st_size
        self.save()
        os.environ.update(RELEASE_MODE="existing", ARTIFACT_SELECTION="")
        self.command("verify-published", success=False)

    def append_release_asset(self, path, *, asset_id):
        content = path.read_bytes()
        self.state["release"]["assets"].append({
            "id": asset_id, "name": path.name, "size": len(content), "state": "uploaded",
            "digest": "sha256:" + hashlib.sha256(content).hexdigest(), "local_file": str(path)})
        self.save()

    def test_probe_1_a_trailing_file_on_a_failed_promotion_draft_never_goes_public(self):
        core, _ = self.tier_candidate("core", RUN, 300)
        os.environ.update(RELEASE_MODE="promote", SOURCE_SHA="", ARTIFACT_SELECTION=json.dumps(core))
        self.command("stage", "--output-dir", str(self.root / "release-assets"),
                     "--proof-dir", str(self.root / "release-proofs"))
        planted = self.root / f"meta-gateway-{VERSION}-x86_64-pc-windows-msvc.zip"
        planted.write_bytes(b"planted while the promotion was interrupted")
        self.seed_release([self.root / "release-proofs" / helper.SELECTION, planted], draft=True)
        self.state["calls"] = []
        self.save()
        _, error = self.publish(success=False)
        self.assertIn("trailing files on an unpublished release", error)
        self.assertFalse(self.writes())
        self.assertTrue(self.state["release"]["draft"])

    def test_probe_2_a_replaced_trailing_archive_fails_verification_even_while_incomplete(self):
        self.promote_core()
        trailing, _ = self.tier_candidate("trailing", TRAILING_RUN, 301)
        self.assets_mode(trailing)
        self.state["fail_upload"] = f"meta-rpc-gateway-{VERSION}-x86_64-pc-windows-msvc.zip"
        self.save()
        self.command("recover", success=False)
        replaced = next(a for a in self.state["release"]["assets"]
                        if a["name"] == f"meta-gateway-{VERSION}-x86_64-apple-darwin.tar.gz")
        # A well-formed archive of the right member, with other bytes.
        Path(replaced["local_file"]).write_bytes(archive_bytes("meta_gateway", b"replaced after upload", "tar.gz"))
        replaced["digest"] = "sha256:" + helper.assets.sha256(Path(replaced["local_file"]))
        replaced["size"] = Path(replaced["local_file"]).stat().st_size
        self.save()
        os.environ.update(RELEASE_MODE="existing", ARTIFACT_SELECTION="")
        _, error = self.command("verify-published", success=False)
        self.assertIn("does not match its manifest record", error)

    def test_probe_3_a_planted_trailing_receipt_and_archive_fail_and_block_nothing_silently(self):
        self.promote_core()
        trailing, _ = self.tier_candidate("trailing", TRAILING_RUN, 301)
        planted_receipt = self.root / "candidate-selection-trailing.json"
        planted_receipt.write_text(json.dumps(dict(trailing, run_id=999)))
        planted_archive = self.root / f"meta-repair-{VERSION}-x86_64-pc-windows-msvc.zip"
        planted_archive.write_bytes(b"planted archive")
        self.append_release_asset(planted_receipt, asset_id=1990)
        self.append_release_asset(planted_archive, asset_id=1991)
        os.environ.update(RELEASE_MODE="existing", ARTIFACT_SELECTION="")
        # The planted receipt names a run that does not exist.
        _, error = self.command("verify-published", success=False)
        self.assertIn("actions/runs/999", error)
        # Naming the real run with another manifest artifact fails its record.
        planted_receipt.write_text(json.dumps(dict(trailing, manifest_artifact=dict(
            trailing["manifest_artifact"], digest="c" * 64))))
        receipt_asset = next(a for a in self.state["release"]["assets"] if a["id"] == 1990)
        receipt_asset["digest"] = "sha256:" + helper.assets.sha256(planted_receipt)
        receipt_asset["size"] = planted_receipt.stat().st_size
        self.save()
        _, error = self.command("verify-published", success=False)
        self.assertIn("selected manifest artifact digest mismatch", error)
        # The real trailing tier refuses to bind over the planted files
        # before any write; an operator removes them first.
        self.state["calls"] = []
        self.save()
        self.assets_mode(trailing)
        _, error = self.command("recover", success=False)
        self.assertIn("existing release asset conflict", error)
        self.assertIn(f"names run {TRAILING_RUN}/{ATTEMPT}", error)
        self.assertIn("operator cleanup required", error)
        self.assertFalse(self.writes())

    def test_a_tampered_trailing_index_fails_verification_while_an_archive_is_missing(self):
        self.promote_core()
        trailing, _ = self.tier_candidate("trailing", TRAILING_RUN, 301)
        self.assets_mode(trailing)
        self.command("recover")
        missing = next(a for a in self.state["release"]["assets"]
                       if a["name"] == f"meta-repair-{VERSION}-x86_64-pc-windows-msvc.zip")
        self.state["release"]["assets"].remove(missing)
        index = next(a for a in self.state["release"]["assets"] if a["name"] == "index-trailing.json")
        Path(index["local_file"]).write_text('{"tampered": true}\n')
        index["digest"] = "sha256:" + helper.assets.sha256(Path(index["local_file"]))
        index["size"] = Path(index["local_file"]).stat().st_size
        self.save()
        os.environ.update(RELEASE_MODE="existing", ARTIFACT_SELECTION="")
        _, error = self.command("verify-published", success=False)
        self.assertIn("published trailing index-trailing.json hash mismatch", error)

    def test_promotion_rerun_verifies_trailing_files_already_on_the_public_release(self):
        core, _ = self.promote_core()
        trailing, _ = self.tier_candidate("trailing", TRAILING_RUN, 301)
        self.assets_mode(trailing)
        self.command("recover")
        replaced = next(a for a in self.state["release"]["assets"]
                        if a["name"] == f"meta-gateway-{VERSION}-x86_64-apple-darwin.tar.gz")
        Path(replaced["local_file"]).write_bytes(archive_bytes("meta_gateway", b"swapped", "tar.gz"))
        replaced["digest"] = "sha256:" + helper.assets.sha256(Path(replaced["local_file"]))
        replaced["size"] = Path(replaced["local_file"]).stat().st_size
        self.state["calls"] = []
        self.save()
        os.environ.update(RELEASE_MODE="promote", ARTIFACT_SELECTION=json.dumps(core))
        _, error = self.publish(success=False)
        self.assertIn("does not match its manifest record", error)
        self.assertFalse(self.writes())

    def partial_trailing(self, stop_before):
        """Promote the core, then add the trailing tier until `stop_before`
        fails to upload; returns the trailing selection."""
        self.promote_core()
        trailing, _ = self.tier_candidate("trailing", TRAILING_RUN, 301)
        self.assets_mode(trailing)
        self.state["fail_upload"] = stop_before
        self.save()
        self.command("recover", success=False)
        self.state.pop("fail_upload")
        self.save()
        os.environ.update(RELEASE_MODE="existing", ARTIFACT_SELECTION="")
        return trailing

    def replace_release_asset(self, name, content):
        asset = next(a for a in self.state["release"]["assets"] if a["name"] == name)
        Path(asset["local_file"]).write_bytes(content)
        asset["digest"] = "sha256:" + hashlib.sha256(content).hexdigest()
        asset["size"] = len(content)
        self.save()

    def test_a_receipt_alone_is_bound_through_its_run_before_it_counts_as_verified(self):
        self.partial_trailing("manifest-provenance-trailing.jsonl")
        names = {asset["name"] for asset in self.state["release"]["assets"]}
        self.assertIn("candidate-selection-trailing.json", names)
        self.assertNotIn("candidate-manifest-trailing.json", names)
        self.state["calls"] = []
        self.save()
        stdout, _ = self.command("verify-published")
        self.assertIn("every present file verified", stdout)
        reads = [call["args"][1] for call in self.state["calls"] if call["args"][0] == "api"]
        self.assertIn(helper.repo_endpoint("actions/artifacts/301/zip"), reads)
        self.assertIn(helper.repo_endpoint(f"actions/runs/{TRAILING_RUN}"), reads)

    def test_a_planted_manifest_bundle_beside_a_receipt_fails(self):
        trailing = self.partial_trailing("candidate-manifest-trailing.json")
        self.replace_release_asset("manifest-provenance-trailing.jsonl", b'{"planted": true}\n')
        _, error = self.command("verify-published", success=False)
        self.assertIn("manifest bundle is not the candidate's", error)

    def test_target_bundles_waiting_for_their_archives_must_be_the_candidates(self):
        first_archive = helper.trailing_upload_order(helper.tier_file_names(VERSION, "trailing"))[5]
        self.assertTrue(first_archive.startswith("meta-"), first_archive)
        self.partial_trailing(first_archive)
        stdout, _ = self.command("verify-published")
        self.assertIn("every present file verified", stdout)
        self.replace_release_asset("target-x86_64-pc-windows-msvc-provenance.jsonl", b'{"planted": true}\n')
        _, error = self.command("verify-published", success=False)
        self.assertIn("is not the candidate's", error)

    def test_verification_requires_green_candidate_and_ci_runs(self):
        self.promote_core()
        trailing, _ = self.tier_candidate("trailing", TRAILING_RUN, 301)
        self.assets_mode(trailing)
        self.command("recover")
        os.environ.update(RELEASE_MODE="existing", ARTIFACT_SELECTION="")
        self.command("verify-published")
        # A failed CI run counts only through its attestation: none here.
        self.state["api"][helper.repo_endpoint(
            f"actions/runs/{CI}/artifacts?name=meerkat-ci-attestation-{SHA}-1&per_page=100")] = [
            {"artifacts": [], "total_count": 0}]
        for endpoint, change, message in (
            (f"actions/runs/{TRAILING_RUN}", {"conclusion": "failure"}, "not completed-success"),
            (f"actions/runs/{RUN}", {"conclusion": "cancelled"}, "not completed-success"),
            (f"actions/runs/{CI}", {"conclusion": "failure"}, "exact-tree attestation"),
            (f"actions/runs/{CI}", {"conclusion": "cancelled"}, "not completed-success"),
        ):
            with self.subTest(endpoint=endpoint, change=change):
                original = dict(self.state["api"][helper.repo_endpoint(endpoint)])
                self.state["api"][helper.repo_endpoint(endpoint)].update(change)
                self.save()
                _, error = self.command("verify-published", success=False)
                self.assertIn(message, error)
                self.state["api"][helper.repo_endpoint(endpoint)] = original
                self.save()

    def test_core_verification_requires_the_manifests_ci_run_green(self):
        """...or red only in rows Meta does not depend on: a red Meta row
        fails the published-release check."""
        self.promote_core()
        os.environ.update(RELEASE_MODE="existing", ARTIFACT_SELECTION="")
        self.command("verify-published")
        self.red_ci({"Main unit tests (meta)": "meta"})
        _, error = self.command("verify-published", success=False)
        self.assertIn("does not count for Meta", error)
        self.assertIn("meta depends on job Main unit tests (meta)", error)

    def test_assets_outside_the_tier_file_names_fail_verification(self):
        self.promote_core()
        os.environ.update(RELEASE_MODE="existing", ARTIFACT_SELECTION="")
        for name in ("install.sh", f"meta-gateway-{VERSION}-riscv64gc-unknown-linux-gnu.tar.gz"):
            with self.subTest(name=name):
                extra = self.root / name
                extra.write_bytes(b"planted")
                self.append_release_asset(extra, asset_id=1980)
                _, error = self.command("verify-published", success=False)
                self.assertIn(f"unexpected release assets: {name}", error)
                self.state["release"]["assets"] = [a for a in self.state["release"]["assets"] if a["id"] != 1980]
                self.save()
        self.command("verify-published")

    def test_a_schema_1_release_refuses_extra_assets_too(self):
        self.staged()
        self.publish()
        os.environ.update(RELEASE_MODE="existing", ARTIFACT_SELECTION="")
        extra = self.root / "install.sh"
        extra.write_bytes(b"planted")
        self.append_release_asset(extra, asset_id=1981)
        _, error = self.command("verify-published", success=False)
        self.assertIn("unexpected release assets: install.sh", error)

    def test_local_dispatch_refuses_a_trailing_promotion(self):
        trailing, _ = self.tier_candidate("trailing", TRAILING_RUN, 301)
        selection = self.root / "accepted-trailing.json"
        selection.write_text(json.dumps(trailing))
        self.state["calls"] = []
        self.save()
        os.environ.update(RELEASE_MODE="", SOURCE_SHA="", RELEASE_TIER="", RELEASE_TAG=TAG,
                          ARTIFACT_SELECTION_FILE=str(selection), PUBLISH_RELEASE_PACKAGES="true")
        _, error = self.command("dispatch", "--mode", "promote", success=False)
        self.assertIn("promotion accepts only a core selection", error)
        self.assertFalse(self.state["calls"])
        os.environ.update(PUBLISH_RELEASE_PACKAGES="false")
        self.command("dispatch", "--mode", "assets")
        self.assertEqual(json.loads(self.state["calls"][-1]["inputs"]["artifact_selection"]), trailing)

    def test_promotion_of_a_trailing_selection_fails_at_resolution(self):
        trailing, _ = self.tier_candidate("trailing", TRAILING_RUN, 301)
        os.environ.update(RELEASE_MODE="promote", SOURCE_SHA="", RELEASE_TIER="",
                          ARTIFACT_SELECTION=json.dumps(trailing))
        _, error = self.command("resolve", success=False)
        self.assertIn("promotion accepts only a core selection", error)
        os.environ["RELEASE_MODE"] = "assets"
        self.command("resolve")

    def test_schema_1_documents_keep_verifying_and_schema_2_requires_its_tier(self):
        v1_selection, v1_manifest = self.staged()
        self.assertEqual(v1_selection["schema_version"], 1)
        self.assertNotIn("tier", v1_selection)
        self.publish()
        self.state["calls"] = []
        self.save()
        os.environ.update(RELEASE_MODE="existing", ARTIFACT_SELECTION="")
        self.command("verify-published")
        self.assertFalse(self.writes())
        for change, message in (
            ({"tier": "core"}, "schema 1 has no tier"),
            ({"schema_version": 2}, "schema 2 requires tier"),
            ({"schema_version": 2, "tier": "everything"}, "schema 2 requires tier"),
            ({"schema_version": 3}, "schema 3 requires tier"),
            ({"schema_version": 4}, "schema 4 requires tier"),
            ({"schema_version": 5}, "unsupported schema version"),
        ):
            with self.subTest(change=change):
                with self.assertRaisesRegex(helper.ProtocolError, message):
                    helper.validate_manifest(dict(v1_manifest, **change))
                with self.assertRaisesRegex(helper.ProtocolError, message):
                    helper.validate_selection(dict(v1_selection, **change))
        # A tiered manifest must hold exactly its tier's targets.
        core, core_manifest = self.tier_candidate("core", 103, 302)
        widened = dict(core_manifest, artifacts=v1_manifest["artifacts"])
        with self.assertRaisesRegex(helper.ProtocolError, "target outside the candidate tier|invalid target"):
            helper.validate_manifest(widened)

    def test_collection_binds_the_core_crates_tree_the_legs_built(self):
        """A candidate may build before Core's tag exists: collection reads no
        tag, and binds the tree of crates/ at the source commit (the in-tree
        Core the gateways embed) in the attested schema 3 manifest."""
        core, manifest = self.tier_candidate("core", RUN, 300)
        self.assertEqual((core["schema_version"], manifest["schema_version"]),
                         (helper.TIERED_SCHEMA, helper.TIERED_SCHEMA))
        self.assertEqual(manifest["core_crates_tree"], CRATES_TREE)
        self.assertIn(("rev-parse", "--verify", f"{SHA}:crates"), self.git_calls)
        self.assertFalse([call for call in self.git_calls if call[0] == "fetch" or CORE_TAG in " ".join(call)])
        for value in (None, "C" * 40, "c" * 64, "c" * 39, 7):
            with self.subTest(value=value), self.assertRaisesRegex(helper.ProtocolError, "invalid Core crates tree"):
                helper.validate_manifest(dict(manifest, core_crates_tree=value))
        unbound = {key: value for key, value in manifest.items() if key != "core_crates_tree"}
        with self.assertRaisesRegex(helper.ProtocolError, "invalid candidate manifest fields"):
            helper.validate_manifest(unbound)
        # Schema 2 manifests carry no tree (nor a floor record), and accept none.
        legacy = {key: value for key, value in unbound.items() if key != "floor_record"}
        helper.validate_manifest(dict(legacy, schema_version=2))
        with self.assertRaisesRegex(helper.ProtocolError, "invalid candidate manifest fields"):
            helper.validate_manifest(dict(manifest, schema_version=2))

    def stage_core(self, selected, name, *, success=True):
        os.environ.update(RELEASE_MODE="promote", SOURCE_SHA="", ARTIFACT_SELECTION=json.dumps(selected))
        self.state["calls"] = []
        self.save()
        self.git_calls.clear()
        return self.command("stage", "--output-dir", str(self.root / f"assets-{name}"),
                            "--proof-dir", str(self.root / f"proofs-{name}"), success=success)

    def test_promotion_compares_the_bound_core_with_cores_tag_after_fetching_it(self):
        core, _ = self.tier_candidate("core", RUN, 300)
        self.stage_core(core, "match")
        fetch = self.git_calls.index(("fetch", "--no-tags", "origin", f"+{CORE_TAG}:{CORE_TAG}"))
        self.assertLess(self.git_calls.index(("show", f"{SHA}:Cargo.toml")), fetch)
        self.assertLess(fetch, self.git_calls.index(("rev-parse", "--verify", f"{CORE_TAG}^{{commit}}:crates")))
        self.assertTrue((self.root / "assets-match").is_dir())

    def test_promotion_refuses_a_candidate_built_with_other_root_build_inputs_than_cores_tag(self):
        """Besides crates/, the workspace's root manifest, lock, toolchain and
        Cargo configuration decide how the gateways compile Core: each must
        be Core's tag's (a tag that only changed Cargo.lock does not admit a
        candidate built with the older lock), and a path neither has is
        equal."""
        core, _ = self.tier_candidate("core", RUN, 300)
        for path in helper.CORE_ROOT_INPUTS:
            for case, overrides in (("differs", {f"{CORE_TAG}^{{commit}}:{path}": "f" * 40}),
                                    ("only the source has it", {f"{CORE_TAG}^{{commit}}:{path}": ""})):
                with self.subTest(path=path, case=case):
                    self.root_inputs = overrides
                    _, error = self.stage_core(core, f"{path.strip('.')}-{case.split()[0]}", success=False)
                    self.assertIn(f"{path} differ from Core's v{CORE}", error)
                    self.assertFalse(self.writes())
        self.root_inputs = {f"{SHA}:.cargo": "", f"{CORE_TAG}^{{commit}}:.cargo": ""}
        self.stage_core(core, "neither")

    def test_promotion_refuses_a_candidate_whose_core_is_not_cores_tag(self):
        """The check the legs made before schema 3 (git diff --quiet v<Core>
        HEAD -- crates/), moved to promotion: it refuses before any target
        artifact is read and writes nothing."""
        core, _ = self.tier_candidate("core", RUN, 300)
        trees = dict(self.crates_trees)
        cases = (
            ("differs", lambda: self.crates_trees.update({f"{CORE_TAG}^{{commit}}:crates": "d" * 40}),
             f"crates/ differs from Core's v{CORE}: this candidate embeds unreleased Core"),
            ("no-tag", lambda: setattr(self, "core_tag_on_origin", False), "couldn't find remote ref"),
            ("no-pin", lambda: setattr(self, "core_pin", f'"{CORE}"'), "pins no exact meerkat-core version"),
            ("moved-source", lambda: self.crates_trees.update({f"{SHA}:crates": "e" * 40}),
             "candidate Core crates tree does not match its source commit"),
        )
        for name, change, message in cases:
            with self.subTest(case=name):
                self.crates_trees, self.core_tag_on_origin, self.core_pin = dict(trees), True, f'"={CORE}"'
                change()
                _, error = self.stage_core(core, name, success=False)
                self.assertIn(message, error)
                self.assertFalse((self.root / f"assets-{name}").exists())
                zips = [call["args"][1] for call in self.state["calls"]
                        if call["args"][0] == "api" and call["args"][1].endswith("/zip")]
                self.assertEqual(zips, [helper.repo_endpoint("actions/artifacts/300/zip")])
                self.assertFalse(self.writes())

    def test_trailing_recovery_refuses_a_candidate_whose_core_is_not_cores_tag(self):
        """The trailing tier's legs build the same commit; its recovery stages
        it the same way, so its gateways are held to Core's tag too."""
        self.promote_core()
        before = self.release_files()
        trailing, _ = self.tier_candidate("trailing", TRAILING_RUN, 301)
        self.crates_trees[f"{CORE_TAG}^{{commit}}:crates"] = "d" * 40
        self.state["calls"] = []
        self.save()
        self.assets_mode(trailing)
        _, error = self.command("recover", success=False)
        self.assertIn(f"crates/ differs from Core's v{CORE}", error)
        self.assertFalse(self.writes())
        self.assertEqual(self.release_files(), before)

    def test_schema_2_candidates_stage_without_the_core_tag_check(self):
        """Schema 2 candidates built in this repository compared crates/ with
        Core's tag in their build legs, so staging reads no tag for them; a
        selection must still name its manifest's schema."""
        core, manifest = self.tier_candidate("core", RUN, 300, schema=2)
        self.assertEqual((core["schema_version"], manifest["schema_version"]), (2, 2))
        self.assertNotIn("core_crates_tree", manifest)
        del self.crates_trees[f"{CORE_TAG}^{{commit}}:crates"]
        self.core_tag_on_origin = False
        self.stage_core(core, "schema2")
        self.assertFalse([call for call in self.git_calls if call[0] == "fetch" or call[-1].endswith(":crates")])
        _, error = self.stage_core(dict(core, schema_version=3), "relabelled", success=False)
        self.assertIn("candidate schema/tier mismatch", error)

    # The published-floor record (schema 4).

    def floor_index(self):
        os.environ["RELEASE_MODE"] = "candidate"
        self.step_files()
        self.command("floor-index")
        return self.outputs(), self.summary()

    def test_floor_index_lets_a_candidate_bind_a_record_only_with_every_core_crate_published(self):
        outputs, summary = self.floor_index()
        self.assertEqual(outputs, {"core_published": "true"})
        self.assertIn(f"Every Core crate is on crates.io at {CORE}", summary)
        pristine = copy.deepcopy(self.index)
        for case, change, named in (
            ("older version only", lambda: self.index["meerkat"].pop(), "meerkat"),
            ("yanked", lambda: self.index["meerkat"][1].update(yanked=True), "meerkat (yanked)"),
            ("absent", lambda: self.index.pop("meerkat-core"), "meerkat-core (index unreadable"),
            ("unreadable", lambda: self.unreadable.add("meerkat-sqlite"), "meerkat-sqlite (index unreadable"),
        ):
            with self.subTest(case=case):
                self.index, self.unreadable = copy.deepcopy(pristine), set()
                change()
                outputs, summary = self.floor_index()
                self.assertEqual(outputs, {"core_published": "false"})
                self.assertIn("binds no floor record", summary)
                self.assertIn(named, summary)
        os.environ["RELEASE_MODE"] = "promote"
        self.command("floor-index", success=False)

    def test_floor_bind_records_what_the_floor_build_proved(self):
        lock = b"# the resolved lock\n"
        record = self.floor_bind(self.floor_dir(lock=lock))
        self.assertEqual(record, {
            "core_version": CORE,
            "core_crates": [{"name": "meerkat", "version": CORE, "checksum": INDEX_CKSUM["meerkat"]},
                            {"name": "meerkat-core", "version": CORE, "checksum": INDEX_CKSUM["meerkat-core"]}],
            "source_sha": SHA, "crates_tree": CRATES_TREE, "lock_sha256": hashlib.sha256(lock).hexdigest(),
            "toolchain": TOOLCHAIN})
        self.assertIn("Bound the floor record", self.summary())

    def test_floor_bind_binds_nothing_it_cannot_prove(self):
        pristine = copy.deepcopy(self.index)
        for case, folder, change in (
            ("resolution only", lambda: self.floor_dir(resolution=True), None),
            ("build problems", lambda: self.floor_dir(problems=["error: x"]), None),
            ("another lock", lambda: self.floor_dir(lock_sha256="9" * 64), None),
            ("not a Core crate", lambda: self.floor_dir(dependencies=("serde 1.0.0", "meerkat 0.8.31")), None),
            ("another version", lambda: self.floor_dir(dependencies=("meerkat 0.8.30",)), None),
            ("no Core crate", lambda: self.floor_dir(dependencies=()), None),
            ("yanked", self.floor_dir, lambda: self.index["meerkat"][1].update(yanked=True)),
            ("unreadable index", self.floor_dir, lambda: self.unreadable.add("meerkat-core")),
        ):
            with self.subTest(case=case):
                self.index, self.unreadable = copy.deepcopy(pristine), set()
                if change:
                    change()
                self.assertIsNone(self.floor_bind(folder()))
                self.assertIn("could not be bound", self.summary())

    def test_collection_binds_the_core_floor_record_with_the_meta_crate_checksums(self):
        record = self.bound_record()
        bound = {key: value for key, value in record.items() if key != "meta_crates"}
        core, manifest = self.tier_candidate("core", RUN, 300, collect_args=self.floor_args(bound))
        self.assertEqual(manifest["floor_record"], record)
        self.assertEqual(manifest["schema_version"], helper.TIERED_SCHEMA)
        # Without one (Core not yet published) the manifest binds null.
        _, plain = self.tier_candidate("core", 103, 302)
        self.assertIsNone(plain["floor_record"])

    def test_collection_refuses_a_floor_record_that_does_not_fit(self):
        record = {key: value for key, value in self.bound_record().items() if key != "meta_crates"}
        self.tier_run("core", 105)
        self.tier_run("trailing", 106)

        def collect(tier, run_id, args):
            with mock.patch.dict(os.environ, {"GITHUB_RUN_ID": str(run_id), "RELEASE_MODE": "candidate",
                                              "RELEASE_TIER": tier}):
                return self.command("collect", "--output-dir", str(self.fresh("refused")), "--ci-run-id", str(CI),
                                    "--ci-run-attempt", "1", "--tier", tier, *args, success=False)[1]

        for case, args, message in (
            ("another commit", self.floor_args(dict(record, source_sha=OTHER_SHA)), "another source commit"),
            ("another tree", self.floor_args(dict(record, crates_tree="d" * 40)), "another crates/ tree"),
            ("a missing checksum", self.floor_args(record, {"meerkat-meta": "f" * 64}), "invalid floor record Meta crates"),
            ("an unknown crate", self.floor_args(record, {**META_SHA256, "other": "1" * 64}), "unexpected Meta crate"),
            ("checksums in the record", self.floor_args(dict(record, meta_crates=META_SHA256)), "without Meta crate"),
            ("an unknown field", self.floor_args(dict(record, extra=1)), "invalid floor record fields"),
            ("checksums alone", ["--meta-crate-sha256", "meerkat-meta=" + "f" * 64], "checksums without a floor record"),
        ):
            with self.subTest(case=case):
                self.assertIn(message, collect("core", 105, args))
        self.assertIn("only the core tier binds a floor record", collect("trailing", 106, self.floor_args(record)))
        self.assertIn("release protocol error", collect("core", 105, ["--floor-record-json", "{not json"]))

    def read_floor_record(self, selected):
        os.environ.update(RELEASE_MODE="promote", SOURCE_SHA="", ARTIFACT_SELECTION=json.dumps(selected))
        self.step_files()
        output = self.fresh("floor-record")
        self.command("floor-record", "--output", str(output))
        return json.loads(output.read_text()), self.outputs()

    def test_floor_record_reads_the_verified_core_candidates_record(self):
        record = self.bound_record()
        bound = {key: value for key, value in record.items() if key != "meta_crates"}
        core, _ = self.tier_candidate("core", RUN, 300, collect_args=self.floor_args(bound))
        self.assertEqual(self.read_floor_record(core), (record, {"present": "true"}))
        # Read through the same verification stage applies: the attestation.
        self.state["verify_failure"] = True
        self.save()
        self.assertEqual(self.read_floor_record(core), (None, {"present": "false"}))
        self.state["verify_failure"] = False
        self.save()
        for schema, run_id, aid in ((3, 103, 302), (None, 104, 303)):
            with self.subTest(schema=schema):
                legacy, _ = self.tier_candidate("core", run_id, aid, schema=schema)
                self.assertEqual(self.read_floor_record(legacy), (None, {"present": "false"}))
        self.assertEqual(self.read_floor_record(dict(core, run_id=999)), (None, {"present": "false"}))

    def reuse(self, *, record=None, resolution=None, checksums=META_SHA256):
        """floor-reuse in promote mode: (reuse output, job summary)."""
        record = self.bound_record() if record is None else record
        record_file = self.fresh("record")
        record_file.write_text(record if isinstance(record, str) else json.dumps(record))
        if resolution is None:
            resolution = self.floor_dir(resolution=True, dependencies=("meerkat 0.8.31", "meerkat-core 0.8.31"))
        os.environ["RELEASE_MODE"] = "promote"
        self.step_files()
        args = ["floor-reuse", "--record", str(record_file), "--resolution", str(resolution)]
        for name, value in checksums.items():
            args += ["--meta-crate-sha256", f"{name}={value}"]
        self.command(*args)
        return self.outputs()["reuse"], self.summary()

    def assert_full_build(self, reason, **kwargs):
        reuse, summary = self.reuse(**kwargs)
        self.assertEqual(reuse, "false", summary)
        self.assertIn("The floor check builds", summary)
        self.assertIn(reason, summary)

    def test_floor_reuse_when_everything_the_record_binds_still_holds(self):
        reuse, summary = self.reuse()
        self.assertEqual(reuse, "true", summary)
        self.assertIn("Reused the candidate's floor record", summary)

    def test_floor_reuse_refuses_an_absent_record(self):
        self.assert_full_build("the candidate bound no floor record", record="null")

    def test_floor_reuse_refuses_an_unreadable_record(self):
        self.assert_full_build("unreadable floor record", record="{not json")

    def test_floor_reuse_refuses_a_record_with_an_unknown_field(self):
        self.assert_full_build("invalid floor record", record=dict(self.bound_record(), extra=1))

    def test_floor_reuse_refuses_a_record_of_another_commit_or_crates_tree(self):
        self.assert_full_build("another source commit", record=dict(self.bound_record(), source_sha=OTHER_SHA))
        self.assert_full_build("another crates/ tree", record=dict(self.bound_record(), crates_tree="d" * 40))

    def test_floor_reuse_refuses_another_declared_core_version(self):
        record = self.bound_record()
        self.core_pin = '"=0.8.32"'
        self.assert_full_build("now declares Core 0.8.32", record=record)

    def test_floor_reuse_refuses_a_changed_index_checksum(self):
        record = self.bound_record()
        self.index["meerkat-core"][1]["cksum"] = "1" * 64
        self.assert_full_build("meerkat-core 0.8.31 has another index checksum", record=record)

    def test_floor_reuse_refuses_a_yanked_core_crate(self):
        record = self.bound_record()
        self.index["meerkat"][1]["yanked"] = True
        self.assert_full_build("meerkat 0.8.31 is yanked", record=record)

    def test_floor_reuse_refuses_a_core_crate_gone_from_the_index(self):
        record = self.bound_record()
        self.index["meerkat"].pop()
        self.assert_full_build("meerkat 0.8.31 is not in the crates.io index", record=record)

    def test_floor_reuse_refuses_an_unreadable_index(self):
        record = self.bound_record()
        self.unreadable.add("meerkat-core")
        self.assert_full_build("crates.io index read for meerkat-core failed", record=record)

    def test_floor_reuse_refuses_a_changed_lock(self):
        self.assert_full_build("the re-resolved lock differs", resolution=self.floor_dir(
            resolution=True, lock=b"# another resolution\n"))

    def test_floor_reuse_refuses_a_failed_re_resolution(self):
        self.assert_full_build("the re-resolution failed", resolution=self.floor_dir(
            resolution=True, problems=["declared dependency versions do not resolve from crates.io: x"]))
        self.assert_full_build("no re-resolution report", resolution=self.fresh("missing"))

    def test_floor_reuse_refuses_other_resolved_core_crates(self):
        self.assert_full_build("resolves other Core crates", resolution=self.floor_dir(
            resolution=True, dependencies=("meerkat 0.8.31", "meerkat-core 0.8.31", "meerkat-sqlite 0.8.31")))

    def test_floor_reuse_refuses_changed_meta_crate_bytes(self):
        self.assert_full_build("meerkat-meta now packages to " + "0" * 64,
                               checksums={**META_SHA256, "meerkat-meta": "0" * 64})
        self.assert_full_build("meta-extension-state now packages to nothing",
                               checksums={"meerkat-meta": META_SHA256["meerkat-meta"]})

    def test_floor_reuse_refuses_another_toolchain(self):
        record = self.bound_record()
        self.toolchain = TOOLCHAIN.replace("1.97.0", "1.97.1")
        self.assert_full_build("the toolchain differs", record=record)

    def test_floor_reuse_is_promote_only_and_never_fails_the_job(self):
        with mock.patch.object(helper, "floor_mismatches", side_effect=RuntimeError("boom")):
            reuse, summary = self.reuse()
        self.assertEqual(reuse, "false")
        self.assertIn("the record check failed", summary)
        os.environ["RELEASE_MODE"] = "candidate"
        self.command("floor-reuse", "--record", str(self.fresh("x")), "--resolution", str(self.fresh("y")),
                     success=False)

    def test_collection_refuses_rather_than_waits_while_main_ci_is_not_green(self):
        """Builds start right after the merge, so collection is where main CI
        must already be green (or red only in rows Meta does not depend on,
        which its attestation shows); an unfinished run is refused at once,
        and so is a failed run without an attestation."""
        endpoint = helper.repo_endpoint(f"actions/runs/{CI}")
        listing = helper.repo_endpoint(f"actions/runs/{CI}/artifacts?name=meerkat-ci-attestation-{SHA}-1&per_page=100")
        self.state["api"][listing] = [{"artifacts": [], "total_count": 0}]
        for status, conclusion, reason in (("in_progress", None, "not completed-success"),
                                           ("queued", None, "not completed-success"),
                                           ("completed", "failure", "exact-tree attestation")):
            with self.subTest(status=status, conclusion=conclusion):
                self.state["api"][endpoint].update(status=status, conclusion=conclusion)
                self.state["calls"] = []
                self.save()
                _, error = self.command("collect", "--output-dir", str(self.root / f"refused-{status}"),
                                        "--ci-run-id", str(CI), "--ci-run-attempt", "1", success=False)
                self.assertIn(reason, error)
                reads = [c for c in self.state["calls"] if c["args"][:2] == ["api", endpoint]]
                self.assertEqual(len(reads), 1, "collection polled main CI instead of refusing")
                self.assertFalse(any("/zip" in c["args"][1] for c in self.state["calls"] if c["args"][0] == "api"))
                self.assertFalse((self.root / f"refused-{status}").exists())

    def red_ci(self, failed, *, result="failure", **overrides):
        """Main CI run CI attempt 1 failed, and left an exact-tree attestation
        whose product evidence shows only the `failed` jobs (name: product)
        red; `overrides` replace attestation fields."""
        self.state["api"][helper.repo_endpoint(f"actions/runs/{CI}")].update(conclusion="failure")
        jobs = [{"name": "Change classification", "conclusion": "success", "product": "core", "downstream": False},
                {"name": "Main unit tests (core)", "conclusion": "success", "product": "core", "downstream": False},
                {"name": "Main unit tests (meta)", "conclusion": "success", "product": "meta", "downstream": False},
                *({"name": name, "conclusion": "failure", "product": product, "downstream": False}
                  for name, product in failed.items())]
        record = {"schema_version": 4, "validation_backend": "github-hosted-cargo", "validation_result": result,
                  "repository": REPO, "commit_sha": SHA, "tree_sha": SOURCE_TREE, "workflow": "CI",
                  "workflow_run_id": str(CI), "workflow_run_attempt": "1", "event": "push",
                  "ref": "refs/heads/main", "plan": {"mode": "workspace"},
                  "product_evidence": {"version": 1, "plan_errors": 0, "failed_lanes": ["Main unit tests"],
                                       "jobs": jobs},
                  **overrides}
        folder = self.fresh("ci-attestation")
        folder.mkdir()
        (folder / "attestation.json").write_text(json.dumps(record))
        packed = self.fresh("ci-attestation-zip")
        self.pack(packed, folder, ["attestation.json"])
        name = f"meerkat-ci-attestation-{SHA}-1"
        metadata = self.add_artifact(777, name, packed, run_id=CI)
        self.state["api"][helper.repo_endpoint(f"actions/runs/{CI}/artifacts?name={name}&per_page=100")] = [
            {"artifacts": [metadata], "total_count": 1}]
        self.save()

    def test_the_candidate_path_completes_on_a_red_ci_run_meta_does_not_depend_on(self):
        """A red main CI run that the candidate's gate accepted for Meta (only
        a Connect row failed) carries the candidate through collection,
        promotion, publication and the published-release check: each re-check
        of the bound CI run judges its attestation with the release commit's
        first parent's copy of the root's evidence reader, as the gate did."""
        self.red_ci({"Main unit tests (connect[console])": "connect"})
        core, manifest = self.promote_core()
        self.assertEqual((manifest["ci"]["run_id"], manifest["ci"]["run_attempt"]), (CI, 1))
        self.assertFalse(self.state["release"]["draft"])
        reads = [c["args"][1] for c in self.state["calls"] if c["args"][0] == "api"]
        self.assertTrue(any(f"actions/runs/{CI}/artifacts?name=meerkat-ci-attestation-{SHA}-1" in r for r in reads),
                        "the CI run's attestation was read")
        self.assertIn(("show", f"{PARENT_SHA}:scripts/ci-product-evidence.mjs"), self.git_calls)
        os.environ.update(RELEASE_MODE="existing", ARTIFACT_SELECTION="")
        stdout, _ = self.command("verify-published")
        self.assertIn("trailing tier not added yet", stdout)

    def test_a_red_ci_run_refuses_the_candidate_unless_the_parents_reader_accepts_it_for_meta(self):
        connect = {"Main unit tests (connect[console])": "connect"}
        for case, failed, setup, reason in (
                ("a Meta row failed", {"Main unit tests (meta)": "meta"}, None,
                 "meta depends on job Main unit tests (meta)"),
                ("a Core row failed", {"Clippy (core)": "core"}, None, "meta depends on job Clippy (core)"),
                ("the parent has no reader", connect, lambda: setattr(self, "parent_reader", False),
                 "ci-product-evidence.mjs"),
                ("the commit changes the reader", connect,
                 lambda: setattr(self, "changed_paths", ("scripts/ci-product-evidence.mjs",)),
                 "changes scripts/ci-product-evidence.mjs"),
                ("the attestation names another commit", connect, None, "attestation")):
            with self.subTest(case=case):
                self.parent_reader, self.changed_paths = True, ("meta/crates/meerkat-meta/src/lib.rs",)
                if setup:
                    setup()
                overrides = {"commit_sha": OTHER_SHA} if case == "the attestation names another commit" else {}
                self.red_ci(failed, **overrides)
                _, error = self.command("collect", "--output-dir", str(self.fresh("refused")),
                                        "--ci-run-id", str(CI), "--ci-run-attempt", "1", success=False)
                self.assertIn(reason, error)

    def collect_with_own_run_reads(self, script, *, success=True, name="re-read"):
        """collect() with a scripted sequence of reads of its own run record."""
        endpoint = helper.repo_endpoint(f"actions/runs/{RUN}")
        self.state["api_script"] = {endpoint: script}
        self.state["calls"] = []
        self.save()
        with mock.patch.object(helper.time, "sleep") as sleep:
            _, error = self.command("collect", "--output-dir", str(self.root / name),
                                    "--ci-run-id", str(CI), "--ci-run-attempt", "1", success=success)
        reads = [c["args"][1] for c in self.state["calls"] if c["args"][0] == "api"]
        return reads, sleep, error

    def test_collect_re_reads_its_own_run_until_github_reports_it_in_progress(self):
        """GitHub reported a running candidate as queued, then waiting, for a while."""
        run = helper.repo_endpoint(f"actions/runs/{RUN}")
        reads, sleep, error = self.collect_with_own_run_reads(
            [{"update": {"status": "queued"}}, {"update": {"status": "waiting"}},
             {"update": {"status": "in_progress"}}])
        self.assertEqual(reads[:4], [run, run, run, helper.repo_endpoint(f"actions/runs/{CI}")])
        self.assertEqual(sleep.call_args_list, [mock.call(helper.RUN_REREAD_DELAY)] * 2)
        self.assertIn("read 1/40 missed", error)
        self.assertIn("status=queued", error)
        self.assertIn("status=waiting", error)
        self.assertTrue((self.root / "re-read" / helper.MANIFEST).exists())

    def test_collect_re_reads_its_own_run_after_a_transient_api_failure(self):
        """GitHub answered 502, then dropped the connection, then answered."""
        run = helper.repo_endpoint(f"actions/runs/{RUN}")
        reads, sleep, error = self.collect_with_own_run_reads(
            [{"fail": "gh: HTTP 502: Bad Gateway"}, {"fail": "error connecting to api.github.com"}])
        self.assertEqual(reads[:3], [run, run, run])
        self.assertEqual(sleep.call_count, 2)
        self.assertEqual(error.count("transient API failure"), 2)
        self.assertTrue((self.root / "re-read" / helper.MANIFEST).exists())

    def test_collect_refuses_at_once_when_its_own_run_is_wrong_or_the_read_fails_for_good(self):
        """Only a lagging status or a transient failure is read again."""
        run = helper.repo_endpoint(f"actions/runs/{RUN}")
        cases = {
            "identity": ([{"update": {"head_sha": OTHER_SHA, "status": "queued"}}],
                         "run source/workflow/event/ref mismatch"),
            "attempt": ([{"update": {"run_attempt": ATTEMPT + 1}}], "run identity/attempt changed"),
            "forbidden": ([{"fail": "gh: HTTP 403: Resource not accessible by integration"}], "HTTP 403"),
        }
        for label, (script, message) in cases.items():
            with self.subTest(label):
                self.state["api"][run] = self.run_info()
                reads, sleep, error = self.collect_with_own_run_reads(script, success=False, name=f"refused-{label}")
                self.assertIn(message, error)
                self.assertEqual(reads, [run], "a refusal that is no lagging read was read again")
                sleep.assert_not_called()
                self.assertFalse((self.root / f"refused-{label}").exists())

    def test_collect_refuses_once_its_own_run_never_reports_in_progress(self):
        run = helper.repo_endpoint(f"actions/runs/{RUN}")
        reads, sleep, error = self.collect_with_own_run_reads(
            [{"update": {"status": "queued"}}], success=False, name="never")
        self.assertEqual(reads, [run] * 40)
        self.assertEqual(sleep.call_count, 39)
        self.assertIn("collection requires its in-progress candidate run (after 40 reads over", error)
        self.assertEqual((helper.RUN_REREAD_ATTEMPTS, helper.RUN_REREAD_DELAY), (40, 15), "10 minutes")
        self.assertFalse((self.root / "never").exists())

    def collect_with_listing_reads(self, script, *, success=True, name="listing"):
        """collect() with a scripted sequence of reads of its run's artifact listing."""
        listing = helper.repo_endpoint(f"actions/runs/{RUN}/artifacts?per_page=100")
        self.state["api_script"] = {listing: script}
        self.state["calls"] = []
        self.save()
        with mock.patch.object(helper.time, "sleep") as sleep:
            _, error = self.command("collect", "--output-dir", str(self.root / name),
                                    "--ci-run-id", str(CI), "--ci-run-attempt", "1", success=success)
        reads = [c["args"][1] for c in self.state["calls"] if c["args"][0] == "api" and c["args"][1] == listing]
        return reads, sleep, error

    def test_collect_re_reads_the_artifact_listing_after_a_transient_api_failure(self):
        """GitHub answered the listing with 503, then reset the connection, then answered."""
        reads, sleep, error = self.collect_with_listing_reads(
            [{"fail": "gh: HTTP 503: Service Unavailable"}, {"fail": "read: connection reset by peer"}])
        self.assertEqual(len(reads), 3)
        self.assertEqual(sleep.call_args_list, [mock.call(helper.RUN_REREAD_DELAY)] * 2)
        self.assertEqual(error.count("transient API failure"), 2)
        self.assertTrue((self.root / "listing" / helper.MANIFEST).exists())

    def test_collect_refuses_at_once_when_the_artifact_listing_fails_for_good_or_is_incomplete(self):
        """Only a transient failure of the listing is read again: an incomplete listing is no lag here."""
        listing = helper.repo_endpoint(f"actions/runs/{RUN}/artifacts?per_page=100")
        complete = copy.deepcopy(self.state["api"][listing])
        partial = [{"artifacts": complete[0]["artifacts"][:-1], "total_count": len(complete[0]["artifacts"]) - 1}]
        cases = {
            "not found": ([{"fail": "gh: HTTP 404: Not Found"}], complete, "HTTP 404"),
            "incomplete": ([], partial, "missing/duplicate/unexpected target artifacts"),
        }
        for label, (script, value, message) in cases.items():
            with self.subTest(label):
                self.state["api"][listing] = copy.deepcopy(value)
                reads, sleep, error = self.collect_with_listing_reads(script, success=False, name=f"listing-{label}")
                self.assertIn(message, error)
                self.assertEqual(len(reads), 1, "a refusal that is no transient failure was read again")
                sleep.assert_not_called()
                self.assertFalse((self.root / f"listing-{label}").exists())

    def test_download_media_negotiation_uses_explicit_endpoint_intent(self):
        aid = self.artifact_ids[0]
        metadata = self.state["api"][helper.repo_endpoint(f"actions/artifacts/{aid}")]
        identity = {"id": aid, "name": metadata["name"], "digest": metadata["digest"][7:]}
        downloads = self.root / "downloads"
        downloads.mkdir()
        path = helper.download_artifact(identity, self.run_info(), downloads)
        self.assertEqual(helper.assets.sha256(path), identity["digest"])
        self.load()
        self.assertIn(
            ["api", helper.repo_endpoint(f"actions/artifacts/{aid}/zip")],
            [call["args"] for call in self.state["calls"]],
        )
        original = next(iter(self.target_files.values()))
        self.seed_release([original])
        inventory = helper.release_inventory(self.state["release"])
        helper.download_release_inventory(inventory, self.root / "release-download")
        self.load()
        self.assertIn(
            ["api", helper.repo_endpoint("releases/assets/900"),
             "--header", "Accept: application/octet-stream"],
            [call["args"] for call in self.state["calls"]],
        )
        self.assertEqual((self.root / "release-download" / original.name).read_bytes(),
                         original.read_bytes())

    def test_nonzero_artifact_download_discards_error_body_before_hashing(self):
        aid = self.artifact_ids[0]
        endpoint = helper.repo_endpoint(f"actions/artifacts/{aid}/zip")
        metadata = self.state["api"][helper.repo_endpoint(f"actions/artifacts/{aid}")]
        identity = {"id": aid, "name": metadata["name"], "digest": metadata["digest"][7:]}
        downloads = self.root / "downloads"
        downloads.mkdir()
        self.state.update(fail_endpoints=[endpoint],
                          download_error_body='{"message":"download failed","status":"415"}')
        self.save()
        with mock.patch.object(helper.assets, "sha256") as hash_file:
            with self.assertRaisesRegex(helper.ProtocolError, "fixture API failure"):
                helper.download_artifact(identity, self.run_info(), downloads)
            hash_file.assert_not_called()
        self.assertFalse((downloads / f"artifact-{aid}.zip").exists())
        self.load()
        self.state["fail_endpoints"] = []
        self.save()
        path = helper.download_artifact(identity, self.run_info(), downloads)
        self.assertEqual(helper.assets.sha256(path), identity["digest"])

    def test_nonzero_release_download_discards_error_body_before_hashing(self):
        original = next(iter(self.target_files.values()))
        self.seed_release([original])
        self.state.update(fail_endpoints=[helper.repo_endpoint("releases/assets/900")],
                          download_error_body='{"message":"download failed"}')
        self.save()
        inventory = helper.release_inventory(self.state["release"])
        output = self.root / "release-download"
        with mock.patch.object(helper.assets, "sha256") as hash_file:
            with self.assertRaisesRegex(helper.ProtocolError, "fixture API failure"):
                helper.download_release_inventory(inventory, output)
            hash_file.assert_not_called()
        self.assertFalse((output / original.name).exists())

    def test_download_does_not_overwrite_or_remove_preexisting_output(self):
        path = self.root / "existing.zip"
        path.write_bytes(b"original evidence")
        with self.assertRaises(FileExistsError):
            helper.api_download(helper.repo_endpoint("actions/artifacts/201/zip"), path,
                                kind="actions-artifact")
        self.assertEqual(path.read_bytes(), b"original evidence")
        self.load()
        self.assertFalse(self.state["calls"])
        with self.assertRaisesRegex(helper.ProtocolError, "unknown download kind"):
            helper.api_download("unused", self.root / "unknown.zip", kind="unknown")
        self.assertFalse((self.root / "unknown.zip").exists())

    def test_round_trip_exact_original_archives_metadata_and_proof(self):
        selected, manifest = self.staged()
        self.assertEqual(len(manifest["artifacts"]), 5)
        self.assertEqual(sum(len(a["archives"]) for a in manifest["artifacts"]), 15)
        self.assertNotIn("manifest_artifact", manifest)
        self.assertNotIn("manifest_sha256", manifest)
        self.assertEqual(selected["manifest_artifact"]["id"], 300)
        for name, path in self.target_files.items():
            self.assertEqual(path.read_bytes(), (self.root / "release-assets" / name).read_bytes())
        for name in manifest["metadata"]:
            self.assertEqual((self.root / "candidate" / name).read_bytes(),
                             (self.root / "release-assets" / name).read_bytes())
        self.assertEqual(len(list((self.root / "release-assets").iterdir())), 17)
        self.assertEqual(len(list((self.root / "release-proofs").iterdir())), 8)
        self.assertFalse(self.writes())

    def test_collect_output_contains_only_manifest_and_metadata_not_original_archives(self):
        directory = self.root / "candidate"
        self.command("collect", "--output-dir", str(directory),
                     "--ci-run-id", str(CI), "--ci-run-attempt", "1")
        self.assertEqual({path.name for path in directory.iterdir()},
                         {helper.MANIFEST, "index.json", "checksums.sha256"})
        self.assertFalse(any(helper.assets.is_archive(path) for path in directory.iterdir()))
        manifest = helper.read_json(directory / helper.MANIFEST)
        for name, expected_hash in manifest["metadata"].items():
            self.assertEqual(helper.assets.sha256(directory / name), expected_hash)
        self.assertFalse(self.writes())

    def test_exact_verifier_flags_and_workflow_token_retained(self):
        self.candidate()
        calls = [call for call in self.state["calls"] if call["args"][:2] == ["attestation", "verify"]]
        self.assertEqual(len(calls), 15)
        for call in calls:
            args = call["args"]
            for flag, value in {
                "--source-digest": SHA, "--source-ref": helper.MAIN, "--signer-digest": SHA,
                "--cert-identity": f"https://github.com/{REPO}/{helper.WORKFLOW}@{helper.MAIN}",
                "--predicate-type": helper.SLSA, "--format": "json", "--repo": REPO,
            }.items():
                self.assertEqual(args[args.index(flag) + 1], value)
            self.assertIn("--deny-self-hosted-runners", args)
            self.assertIn("--bundle", args)
            self.assertTrue(call["gh_token"])

    def test_candidate_collection_missing_duplicate_mixed_attempt_targets(self):
        endpoint = helper.repo_endpoint(f"actions/runs/{RUN}/artifacts?per_page=100")
        original = copy.deepcopy(self.state["api"][endpoint])
        variants = [
            original[0]["artifacts"][:-1],
            original[0]["artifacts"] + [original[0]["artifacts"][0]],
            [{**a, "name": a["name"].removesuffix("-2") + "-1"} for a in original[0]["artifacts"]],
            [{**a, "id": 201} for a in original[0]["artifacts"]],
        ]
        for listing in variants:
            with self.subTest(listing=listing):
                self.state["api"][endpoint] = [{"artifacts": listing}]
                self.save()
                self.command("collect", "--output-dir", str(self.root / "candidate"),
                             "--ci-run-id", str(CI), "--ci-run-attempt", "1", success=False)
                self.assertFalse(self.writes())

    def test_strict_selection_and_manifest_schema(self):
        selected, manifest = self.candidate()
        for key, value in [
            ("repository", "attacker/fork"), ("schema_version", True), ("schema_version", 2),
            ("run_id", "101"), ("run_id", 0), ("run_attempt", True), ("kind", "latest"),
            ("unknown", "not allowed"),
        ]:
            changed = {**selected, key: value}
            with self.subTest(key=key, value=value), self.assertRaises(helper.ProtocolError):
                helper.validate_selection(changed)
        for key in selected:
            with self.subTest(missing=key), self.assertRaises(helper.ProtocolError):
                helper.validate_selection({k: v for k, v in selected.items() if k != key})
        for key in ("id", "digest", "manifest_sha256"):
            changed = copy.deepcopy(selected)
            changed["manifest_artifact"][key] = None
            with self.subTest(key=key), self.assertRaises(helper.ProtocolError):
                helper.validate_selection(changed)
        changed = copy.deepcopy(manifest)
        changed["artifacts"][1]["id"] = changed["artifacts"][0]["id"]
        with self.assertRaises(helper.ProtocolError):
            helper.validate_manifest(changed)
        with self.assertRaises(helper.ProtocolError):
            helper.parse_json('{"kind":"accepted-candidate","kind":"original-assets"}')

    def test_selection_summary_and_no_self_identity(self):
        selected, manifest = self.candidate()
        summary = self.root / "summary"
        os.environ["GITHUB_STEP_SUMMARY"] = str(summary)
        stdout, _ = self.command("selection", "--manifest-file", str(self.root / "candidate" / helper.MANIFEST),
                                "--artifact-id", "300", "--artifact-digest", selected["manifest_artifact"]["digest"])
        self.assertEqual(json.loads(stdout), selected)
        self.assertIn("/attempts/2", summary.read_text())
        self.assertIn("/artifacts/300/zip", summary.read_text())
        self.assertNotIn("manifest_artifact", manifest)
        self.command("selection", "--manifest-file", str(self.root / "candidate" / helper.MANIFEST),
                     "--artifact-id", "201", "--artifact-digest", "a" * 64, success=False)

    def test_selection_requires_nonempty_upload_digest_and_positive_artifact_id(self):
        selected, _ = self.candidate()
        for artifact_id, artifact_digest in (("300", ""), ("0", "a" * 64), ("300", "sha256:")):
            with self.subTest(artifact_id=artifact_id, artifact_digest=artifact_digest):
                self.command("selection", "--manifest-file", str(self.root / "candidate" / helper.MANIFEST),
                             "--artifact-id", artifact_id, "--artifact-digest", artifact_digest, success=False)
        self.assertRegex(selected["manifest_artifact"]["manifest_sha256"], r"^[0-9a-f]{64}$")
        self.assertRegex(selected["manifest_artifact"]["digest"], r"^[0-9a-f]{64}$")

    def test_stage_refuses_wrong_run_attempt_source_workflow_event_repository_or_failure(self):
        self.candidate()
        endpoint = helper.repo_endpoint(f"actions/runs/{RUN}")
        original = copy.deepcopy(self.state["api"][endpoint])
        changes = [
            ("id", 102), ("run_attempt", 3), ("head_sha", OTHER_SHA),
            ("head_branch", "feature"), ("event", "push"), ("path", ".github/workflows/evil.yml"),
            ("status", "in_progress"), ("conclusion", "failure"),
            ("repository", {"full_name": "attacker/fork", "id": 42}),
            ("head_repository", {"full_name": "attacker/fork", "id": 42}),
        ]
        for key, value in changes:
            with self.subTest(key=key):
                self.state["api"][endpoint] = {**original, key: value}
                self.save()
                self.command("stage", "--output-dir", str(self.root / "out"),
                             "--proof-dir", str(self.root / "proof"), success=False)
                self.assertFalse((self.root / "out").exists())
                self.assertFalse(self.writes())

    def test_artifact_id_expiry_api_digest_zip_digest_and_ownership_fail_closed(self):
        self.candidate()
        endpoint = helper.repo_endpoint("actions/artifacts/300")
        original = copy.deepcopy(self.state["api"][endpoint])
        changes = [
            ("id", 301), ("name", "meta-candidate-manifest-1"), ("expired", True),
            ("digest", "sha256:" + "0" * 64),
            ("workflow_run", {**original["workflow_run"], "id": RUN + 1}),
            ("workflow_run", {**original["workflow_run"], "head_sha": OTHER_SHA}),
            ("workflow_run", {**original["workflow_run"], "repository_id": 7}),
        ]
        for key, value in changes:
            with self.subTest(key=key, value=value):
                self.state["api"][endpoint] = {**original, key: value}
                self.save()
                self.command("stage", "--output-dir", str(self.root / "out"),
                             "--proof-dir", str(self.root / "proof"), success=False)
        self.state["api"][endpoint] = original
        packed = self.root / "candidate.zip"
        packed.write_bytes(packed.read_bytes() + b"outer ZIP mutation")
        self.save()
        _, error = self.command("stage", "--output-dir", str(self.root / "out"),
                                "--proof-dir", str(self.root / "proof"), success=False)
        self.assertIn("downloaded artifact ZIP digest", error)
        self.assertFalse(self.writes())

    def test_wrong_manifest_hash_or_rebuilt_same_source_run_not_interchangeable(self):
        selected, _ = self.candidate()
        for field, value in [("manifest_sha256", "b" * 64), ("id", 201)]:
            changed = copy.deepcopy(selected)
            changed["manifest_artifact"][field] = value
            os.environ["ARTIFACT_SELECTION"] = json.dumps(changed)
            self.command("stage", "--output-dir", str(self.root / "out"),
                         "--proof-dir", str(self.root / "proof"), success=False)
        changed = {**selected, "run_id": 102}
        self.state["api"][helper.repo_endpoint("actions/runs/102")] = {
            **self.state["api"][helper.repo_endpoint(f"actions/runs/{RUN}")], "id": 102}
        self.save()
        os.environ["ARTIFACT_SELECTION"] = json.dumps(changed)
        self.command("stage", "--output-dir", str(self.root / "out"),
                     "--proof-dir", str(self.root / "proof"), success=False)
        self.assertFalse(self.writes())

    def test_verified_certificate_policy_rejects_every_missing_or_wrong_claim(self):
        path = next(iter(self.target_files.values()))
        bundle = path.parent / helper.PROVENANCE
        original = copy.deepcopy(self.state["verification"][path.name])
        certificate = original[0]["verificationResult"]["signature"]["certificate"]
        for key in certificate:
            for missing in (True, False):
                with self.subTest(key=key, missing=missing):
                    changed = copy.deepcopy(original)
                    cert = changed[0]["verificationResult"]["signature"]["certificate"]
                    if missing:
                        del cert[key]
                    else:
                        cert[key] = "incorrect"
                    self.state["verification"][path.name] = changed
                    self.save()
                    with self.assertRaises(helper.ProtocolError):
                        helper.verify_provenance(path, bundle, SHA, RUN, ATTEMPT)
        self.assertFalse(self.writes())

    def test_verified_statement_policy_rejects_predicate_subject_invocation_and_event(self):
        path = next(iter(self.target_files.values()))
        bundle = path.parent / helper.PROVENANCE
        original = copy.deepcopy(self.state["verification"][path.name])
        mutations = [
            (["verifiedTimestamps"], []),
            (["statement", "subject"], [{"name": path.name, "digest": {"sha256": "b" * 64}}]),
            (["statement", "subject"], [{"name": "wrong", "digest": {"sha256": helper.assets.sha256(path)}}]),
            (["statement", "predicateType"], "wrong"),
            (["statement", "predicate", "buildDefinition", "buildType"], "wrong"),
            (["statement", "predicate", "buildDefinition", "externalParameters", "workflow", "path"], "wrong"),
            (["statement", "predicate", "buildDefinition", "externalParameters", "workflow", "ref"], "refs/tags/" + TAG),
            (["statement", "predicate", "buildDefinition", "resolvedDependencies"],
             [{"uri": "wrong", "digest": {"gitCommit": SHA}}]),
            (["statement", "predicate", "buildDefinition", "internalParameters", "github", "event_name"], "push"),
            (["statement", "predicate", "runDetails", "metadata", "invocationId"], "wrong-attempt"),
            (["statement", "predicate", "runDetails", "builder", "id"], "wrong"),
        ]
        for keys, value in mutations:
            with self.subTest(keys=keys):
                changed = copy.deepcopy(original)
                cursor = changed[0]["verificationResult"]
                for key in keys[:-1]:
                    cursor = cursor[key]
                cursor[keys[-1]] = value
                self.state["verification"][path.name] = changed
                self.save()
                with self.assertRaises(helper.ProtocolError):
                    helper.verify_provenance(path, bundle, SHA, RUN, ATTEMPT)

    def test_no_decoded_payload_or_failed_crypto_is_accepted(self):
        path = next(iter(self.target_files.values()))
        bundle = path.parent / helper.PROVENANCE
        for response in ([], [{"attestation": "unverified bundle"}]):
            self.state["verification"][path.name] = response
            self.save()
            with self.assertRaises((helper.ProtocolError, KeyError)):
                helper.verify_provenance(path, bundle, SHA, RUN, ATTEMPT)
        self.state["verify_failure"] = True
        self.save()
        with self.assertRaisesRegex(helper.ProtocolError, "signature verification failed"):
            helper.verify_provenance(path, bundle, SHA, RUN, ATTEMPT)
        self.state["verify_failure"] = False
        bundle.unlink()
        with self.assertRaisesRegex(helper.ProtocolError, "missing original"):
            helper.verify_provenance(path, bundle, SHA, RUN, ATTEMPT)

    def test_archive_members_reject_traversal_links_duplicates_wrong_binary_and_directories(self):
        target = "x86_64-unknown-linux-gnu"
        path = self.target_files[f"meta-rpc-gateway-{VERSION}-{target}.tar.gz"]
        for name, kind, duplicate in [
            ("../rpc_gateway", tarfile.REGTYPE, False),
            ("/rpc_gateway", tarfile.REGTYPE, False),
            ("wrong_binary", tarfile.REGTYPE, False),
            ("rpc_gateway", tarfile.SYMTYPE, False),
            ("rpc_gateway", tarfile.LNKTYPE, False),
            ("rpc_gateway", tarfile.DIRTYPE, False),
            ("rpc_gateway", tarfile.REGTYPE, True),
        ]:
            with self.subTest(name=name, kind=kind):
                with tarfile.open(path, "w:gz") as archive:
                    info = tarfile.TarInfo(name)
                    info.type = kind
                    info.size = 1 if kind == tarfile.REGTYPE else 0
                    info.linkname = "outside" if kind in (tarfile.SYMTYPE, tarfile.LNKTYPE) else ""
                    archive.addfile(info, io.BytesIO(b"x") if info.size else None)
                    if duplicate:
                        archive.addfile(info, io.BytesIO(b"x"))
                with self.assertRaises(helper.ProtocolError):
                    helper.describe_archive(path, VERSION, target)
        windows = "x86_64-pc-windows-msvc"
        path = self.target_files[f"meta-rpc-gateway-{VERSION}-{windows}.zip"]
        for name, mode in [
            ("../rpc_gateway.exe", stat.S_IFREG), ("rpc_gateway", stat.S_IFREG),
            ("rpc_gateway.exe/", stat.S_IFDIR), ("rpc_gateway.exe", stat.S_IFLNK),
        ]:
            with self.subTest(zip_name=name):
                with zipfile.ZipFile(path, "w") as archive:
                    info = zipfile.ZipInfo(name)
                    info.external_attr = mode << 16
                    archive.writestr(info, b"x")
                with self.assertRaises(helper.ProtocolError):
                    helper.describe_archive(path, VERSION, windows)

    def test_producer_member_layout_is_derived_from_exact_binary_and_target(self):
        for target, extension in helper.assets.TARGET_ARCHIVES:
            for prefix, binary in helper.BINARY_NAMES.items():
                with self.subTest(target=target, binary=binary):
                    path = self.target_files[f"{prefix}-{VERSION}-{target}.{extension}"]
                    record = helper.describe_archive(path, VERSION, target)
                    expected = f"target/{target}/release-meta/{binary}.exe" if extension == "zip" else binary
                    self.assertEqual(record["inner"]["name"], expected)
                    self.assertEqual(record["inner"]["sha256"],
                                     helper.hash_bytes(f"original-signed-{binary}-{target}".encode()))
                    helper.validate_archive_record(record, VERSION, target)
                    record["inner"]["name"] = f"chosen/by/manifest/{binary}"
                    with self.assertRaisesRegex(helper.ProtocolError, "inner binary name mismatch"):
                        helper.validate_archive_record(record, VERSION, target)

    def test_windows_zip_refuses_every_nonproducer_path_and_nonregular_member(self):
        target = "x86_64-pc-windows-msvc"
        expected = f"target/{target}/release-meta/rpc_gateway.exe"
        path = self.target_files[f"meta-rpc-gateway-{VERSION}-{target}.zip"]
        cases = [
            ("rpc_gateway.exe", stat.S_IFREG << 16, False, None),
            ("elsewhere/rpc_gateway.exe", stat.S_IFREG << 16, False, None),
            (f"target/{target}/debug/rpc_gateway.exe", stat.S_IFREG << 16, False, None),
            ("target/aarch64-apple-darwin/release-meta/rpc_gateway.exe", stat.S_IFREG << 16, False, None),
            (f"target/{target}/release-meta/meta_gateway.exe", stat.S_IFREG << 16, False, None),
            (f"target/{target}/release-meta/../release-meta/rpc_gateway.exe", stat.S_IFREG << 16, False, None),
            # The standalone producer's path, before the release-meta profile.
            (f"target/{target}/release/rpc_gateway.exe", stat.S_IFREG << 16, False, None),
            ("/" + expected, stat.S_IFREG << 16, False, None),
            ("C:/" + expected, stat.S_IFREG << 16, False, None),
            (expected.replace("/", "\\"), stat.S_IFREG << 16, False, None),
            (expected + "/", stat.S_IFDIR << 16, False, None),
            (expected, stat.S_IFLNK << 16, False, None),
            (expected, stat.S_IFDIR << 16, False, None),
            (expected, 0x10, False, None),
            (expected, stat.S_IFREG << 16, True, None),
            (expected, stat.S_IFREG << 16, False, "extra.txt"),
        ]
        for name, attrs, duplicate, extra in cases:
            with self.subTest(name=name, attrs=attrs, duplicate=duplicate, extra=extra):
                with zipfile.ZipFile(path, "w") as archive:
                    info = zipfile.ZipInfo(name)
                    info.external_attr = attrs
                    archive.writestr(info, b"x")
                    if duplicate:
                        archive.writestr(info, b"duplicate")
                    if extra:
                        archive.writestr(extra, b"extra")
                with self.assertRaises(helper.ProtocolError):
                    helper.describe_archive(path, VERSION, target)
        path.write_bytes(archive_bytes(expected, b"valid", "zip"))
        with zipfile.ZipFile(path) as archive:
            with self.assertRaisesRegex(helper.ProtocolError, "unsafe ZIP member path"):
                helper.zip_members(archive)

    def test_staged_inner_mutation_resigning_and_outer_only_mutation_fail_before_writes(self):
        _, manifest = self.staged()
        record = manifest["artifacts"][0]["archives"][0]
        path = self.root / "release-assets" / record["name"]
        original = path.read_bytes()
        for changed in (original + b"compressed bytes changed",
                        archive_bytes(record["inner"]["name"], b"resigned executable",
                                      dict(helper.assets.TARGET_ARCHIVES)[record["target"]])):
            path.write_bytes(changed)
            self.publish(success=False)
            self.assertFalse(self.writes())
        path.write_bytes(original)
        # Updating checksums cannot bypass the accepted manifest's inner/outer hashes.
        path.write_bytes(archive_bytes(record["inner"]["name"], b"same-name rebuilt executable",
                                      dict(helper.assets.TARGET_ARCHIVES)[record["target"]]))
        checksums = self.root / "release-assets" / "checksums.sha256"
        checksums.write_text(checksums.read_text().replace(record["sha256"], helper.assets.sha256(path)))
        self.publish(success=False)
        self.assertFalse(self.writes())

    def test_stage_checks_inner_hash_even_if_outer_hashes_and_verifier_fixture_match(self):
        selected, manifest = self.candidate()
        item = manifest["artifacts"][0]
        record = item["archives"][0]
        path = self.target_files[record["name"]]
        path.write_bytes(archive_bytes(record["inner"]["name"], b"resigned inner executable",
                                      dict(helper.assets.TARGET_ARCHIVES)[item["target"]]))
        record.update(sha256=helper.assets.sha256(path), size=path.stat().st_size)
        packed = self.root / f"artifact-{item['id']}.zip"
        self.pack(packed, path.parent, [*[r["name"] for r in item["archives"]], helper.PROVENANCE])
        item["digest"] = helper.assets.sha256(packed)
        self.state["api"][helper.repo_endpoint(f"actions/artifacts/{item['id']}")]["digest"] = "sha256:" + item["digest"]
        self.state["verification"][record["name"]] = verified_fixture(record["name"], record["sha256"])
        manifest_path = self.root / "candidate" / helper.MANIFEST
        manifest_path.write_bytes(helper.json_bytes(manifest))
        selected["manifest_artifact"]["manifest_sha256"] = helper.assets.sha256(manifest_path)
        self.state["verification"][helper.MANIFEST] = verified_fixture(
            helper.MANIFEST, selected["manifest_artifact"]["manifest_sha256"])
        manifest_zip = self.root / "candidate.zip"
        self.pack(manifest_zip, manifest_path.parent,
                  [helper.MANIFEST, helper.PROVENANCE, "index.json", "checksums.sha256"])
        selected["manifest_artifact"]["digest"] = helper.assets.sha256(manifest_zip)
        self.state["api"][helper.repo_endpoint("actions/artifacts/300")]["digest"] = (
            "sha256:" + selected["manifest_artifact"]["digest"])
        self.save()
        os.environ["ARTIFACT_SELECTION"] = json.dumps(selected)
        _, error = self.command("stage", "--output-dir", str(self.root / "out"),
                                "--proof-dir", str(self.root / "proof"), success=False)
        self.assertIn("outer/inner bytes mismatch", error)
        self.assertFalse(self.writes())

    def test_publish_is_add_only_receipt_first_verified_then_public_complete_retry_noop(self):
        self.staged()
        self.publish()
        writes = self.writes()
        self.assertEqual(writes[0]["args"][:3], ["release", "create", TAG])
        self.assertIn("--verify-tag", writes[0]["args"])
        uploads = [call for call in writes if call["args"][1] == "upload"]
        self.assertEqual(Path(uploads[0]["args"][3]).name, helper.SELECTION)
        self.assertEqual(len(uploads), 25)
        self.assertEqual(writes[-1]["args"][1], "edit")
        self.assertFalse(self.state["release"]["draft"])
        for call in writes:
            self.assertNotIn("--clobber", call["args"])
            self.assertNotIn("delete", call["args"])
        before = copy.deepcopy(self.state["release"])
        self.state["calls"] = []
        self.save()
        self.publish()
        self.assertEqual(self.state["release"], before)
        self.assertFalse(self.writes())

    def test_existing_bound_partial_draft_fills_missing_without_metadata_replacement(self):
        self.staged()
        proofs = self.root / "release-proofs"
        one_archive = next((self.root / "release-assets").glob("*.zip"))
        self.seed_release([proofs / helper.SELECTION, one_archive], draft=True)
        old = copy.deepcopy(self.state["release"])
        self.publish()
        self.assertEqual(self.state["release"]["name"], old["name"])
        self.assertEqual(self.state["release"]["body"], old["body"])
        for item in old["assets"]:
            self.assertIn(item, self.state["release"]["assets"])
        uploads = [call for call in self.writes() if call["args"][1] == "upload"]
        self.assertEqual(len(uploads), 23)

    def test_new_draft_visibility_lag_retries_reads_not_creation(self):
        self.staged()
        self.state["new_draft_visibility_delay"] = 2
        self.save()
        with mock.patch.object(helper.time, "sleep") as sleep:
            self.publish()
        self.assertEqual(sleep.call_args_list, [mock.call(1), mock.call(1)])
        creates = [call for call in self.writes() if call["args"][1] == "create"]
        self.assertEqual(len(creates), 1)
        uploads = [call for call in self.writes() if call["args"][1] == "upload"]
        self.assertEqual(Path(uploads[0]["args"][3]).name, helper.SELECTION)
        self.assertFalse(self.state["release"]["draft"])

    def test_new_draft_visibility_timeout_leaves_empty_unbound_draft(self):
        self.staged()
        self.state["new_draft_visibility_delay"] = 30
        self.save()
        with mock.patch.object(helper.time, "sleep") as sleep:
            _, error = self.publish(success=False)
        self.assertIn("new draft release missing", error)
        self.assertEqual(sleep.call_count, 29)
        self.assertEqual([call["args"][1] for call in self.writes()], ["create"])
        self.assertTrue(self.state["release"]["draft"])
        self.assertEqual(self.state["release"]["assets"], [])

    def test_new_draft_lookup_does_not_retry_api_errors_or_duplicates(self):
        with mock.patch.object(helper, "release_for_tag", side_effect=helper.ProtocolError("API refusal")) as lookup:
            with mock.patch.object(helper.time, "sleep") as sleep:
                with self.assertRaisesRegex(helper.ProtocolError, "API refusal"):
                    helper.newly_created_release_for_tag(TAG)
            lookup.assert_called_once_with(TAG)
            sleep.assert_not_called()

    def test_conflicting_existing_archive_or_receipt_and_unbound_draft_refuse_before_write(self):
        self.staged()
        proof = self.root / "release-proofs" / helper.SELECTION
        self.seed_release([], draft=True)
        self.publish(success=False)
        self.assertFalse(self.writes())
        self.seed_release([proof], draft=True)
        proof_bytes = proof.read_bytes()
        copy_dir = self.root / "conflict"
        copy_dir.mkdir()
        conflict = copy_dir / helper.SELECTION
        conflict.write_bytes(proof_bytes.replace(b'"run_attempt": 2', b'"run_attempt": 1'))
        self.seed_release([conflict], draft=True)
        self.publish(success=False)
        self.assertFalse(self.writes())
        archive = next((self.root / "release-assets").glob("*.zip"))
        wrong = copy_dir / archive.name
        wrong.write_bytes(b"conflicting existing published bytes")
        self.seed_release([proof, wrong], draft=False)
        self.publish(success=False)
        self.assertFalse(self.writes())

    def test_publish_dry_run_and_wrong_mode_refuse_before_api(self):
        for mode, dry in (("promote", "true"), ("candidate", "false"), ("existing", "false"), ("assets", "false")):
            os.environ.update(RELEASE_MODE=mode, REGISTRY_DRY_RUN=dry)
            self.publish(success=False)
            self.assertFalse(self.state["calls"])

    def test_tag_moved_before_publish_causes_zero_writes(self):
        self.staged()
        self.state["api"][helper.repo_endpoint(f"git/ref/tags/{TAG}")]["object"]["sha"] = OTHER_SHA
        self.save()
        self.publish(success=False)
        self.assertFalse(self.writes())

    def test_tag_movement_during_upload_leaves_draft_unpublished(self):
        self.staged()
        self.state.update(calls=[], move_tag_at=4)
        self.save()
        _, error = self.publish(success=False)
        self.assertIn("tag source changed", error)
        self.assertTrue(self.state["release"]["draft"])
        self.assertFalse(any(call["args"][1] == "edit" for call in self.writes()))

    def test_upload_readback_corruption_never_publishes_draft(self):
        self.staged()
        self.state["corrupt_upload"] = "index.json"
        self.save()
        _, error = self.publish(success=False)
        self.assertIn("readback byte mismatch", error)
        self.assertTrue(self.state["release"]["draft"])
        self.assertFalse(any(call["args"][1] == "edit" for call in self.writes()))

    def test_missing_first_receipt_upload_cannot_be_silently_adopted_on_retry(self):
        self.staged()
        self.state["fail_upload"] = helper.SELECTION
        self.save()
        self.publish(success=False)
        self.assertTrue(self.state["release"]["draft"])
        self.assertFalse(self.state["release"]["assets"])
        self.state["calls"] = []
        del self.state["fail_upload"]
        self.save()
        _, error = self.publish(success=False)
        self.assertIn("unbound partial release", error)
        self.assertFalse(self.writes())

    def test_verify_published_candidate_survives_expired_actions_artifacts_and_never_writes(self):
        self.staged()
        self.publish()
        self.state["calls"] = []
        # Artifacts expire after their retention; the runs and the artifact
        # records stay readable, and verification needs nothing more.
        for key, value in self.state["api"].items():
            if "/actions/artifacts/" in key and isinstance(value, dict) and "expired" in value:
                value["expired"] = True
        self.state["fail_endpoints"] = [key for key in self.state["api"] if key.endswith("/zip")]
        self.save()
        self.command("verify-published")
        self.assertFalse(self.writes())
        reads = [call["args"][1] for call in self.state["calls"] if call["args"][0] == "api"]
        self.assertFalse([read for read in reads if read.endswith("/zip")])
        # The receipt's run, its manifest artifact record and the CI run are read.
        for endpoint in (f"actions/runs/{RUN}", "actions/artifacts/300", f"actions/runs/{CI}"):
            self.assertIn(helper.repo_endpoint(endpoint), reads)

    def test_deleted_receipt_does_not_downgrade_candidate_era_to_legacy(self):
        self.staged()
        paths = list((self.root / "release-assets").iterdir())
        self.seed_release(paths)
        _, error = self.command("verify-published", success=False)
        self.assertIn("candidate-era", error)
        self.assertFalse(self.writes())
        self.protocol = False
        self.command("verify-published")
        self.assertFalse(self.writes())

    def test_published_complete_readback_rejects_missing_archive_or_bad_hash(self):
        self.staged()
        paths = list((self.root / "release-assets").iterdir())
        self.seed_release(paths[:-1])
        self.command("verify-published", success=False)
        self.seed_release(paths)
        self.state["release"]["assets"][0]["digest"] = "sha256:" + "b" * 64
        self.save()
        self.command("verify-published", success=False)
        self.assertFalse(self.writes())

    def test_download_statistics_do_not_invalidate_verification_publication_retry_or_recovery(self):
        self.staged()
        paths = [*list((self.root / "release-assets").iterdir()),
                 *list((self.root / "release-proofs").iterdir())]
        self.seed_release(paths)
        self.state["count_downloads"] = True
        self.save()
        self.command("verify-published")
        self.assertFalse(self.writes())
        self.assertTrue(all(asset["download_count"] > 0 for asset in self.state["release"]["assets"]))
        self.command("publish", "--assets-dir", str(self.root / "release-assets"),
                     "--proof-dir", str(self.root / "release-proofs"))
        self.assertFalse(self.writes())
        missing = next(asset for asset in self.state["release"]["assets"] if asset["name"].endswith(".zip"))
        self.state["release"]["assets"].remove(missing)
        original_ids = {asset["name"]: asset["id"] for asset in self.state["release"]["assets"]}
        self.save()
        os.environ["RELEASE_MODE"] = "assets"
        self.command("recover")
        self.assertEqual(len(self.writes()), 1)
        self.assertEqual(Path(self.writes()[0]["args"][3]).name, missing["name"])
        after_ids = {asset["name"]: asset["id"] for asset in self.state["release"]["assets"]}
        self.assertTrue(original_ids.items() <= after_ids.items())

    def test_inventory_preserves_identity_integrity_and_editable_metadata_changes(self):
        original = {"id": 1, "name": "asset.zip", "state": "uploaded", "size": 123,
                    "digest": "sha256:" + "a" * 64, "label": "original",
                    "content_type": "application/zip", "download_count": 1}
        release = {"id": 800}
        with mock.patch.object(helper, "api", return_value=[[original]]):
            baseline = helper.release_inventory(release)
        for key, value in [
            ("id", 2), ("name", "renamed.zip"), ("size", 124),
            ("digest", "sha256:" + "b" * 64), ("label", "changed"),
            ("content_type", "application/octet-stream"),
        ]:
            with self.subTest(key=key), mock.patch.object(
                    helper, "api", return_value=[[{**original, key: value}]]):
                self.assertNotEqual(helper.release_inventory(release), baseline)
        with mock.patch.object(helper, "api", return_value=[[{
                **original, "download_count": 2, "uploader": {"login": "renamed-user"}}]]):
            self.assertEqual(helper.release_inventory(release), baseline)

    def test_legacy_recovery_preserves_all_existing_ids_bytes_metadata_adds_only_selected_original(self):
        selected, manifest = self.staged()
        self.protocol = False
        all_paths = list((self.root / "release-assets").iterdir())
        # Preserve all three existing Windows archives while adding one Linux original.
        item = next(a for a in manifest["artifacts"] if a["target"] == "x86_64-unknown-linux-gnu")
        record = item["archives"][0]
        self.seed_release([p for p in all_paths if p.name != record["name"]])
        before = copy.deepcopy(self.state["release"])
        receipt = {key: manifest[key] for key in (
            "schema_version", "repository", "source_sha", "version", "tag", "run_id", "run_attempt", "metadata")}
        receipt.update(kind="original-assets", artifacts=[{**item, "archives": [record]}])
        self.state["api"][helper.repo_endpoint(f"actions/runs/{RUN}")].update(event="push", head_branch=TAG)
        self.state["verification"][record["name"]] = verified_fixture(
            record["name"], record["sha256"], ref=f"refs/tags/{TAG}", event="push")
        self.save()
        os.environ.update(RELEASE_MODE="assets", ARTIFACT_SELECTION=json.dumps(receipt))
        self.command("recover")
        self.assertEqual(len(self.writes()), 1)
        self.assertEqual(Path(self.writes()[0]["args"][3]).name, record["name"])
        self.assertEqual(self.state["release"]["body"], before["body"])
        self.assertEqual(self.state["release"]["name"], before["name"])
        for asset in before["assets"]:
            self.assertIn(asset, self.state["release"]["assets"])
        self.assertEqual(len(self.state["release"]["assets"]), 17)
        self.state["calls"] = []
        self.save()
        self.command("recover")
        self.assertFalse(self.writes())

    def test_legacy_recovery_requires_original_metadata_and_unexpired_artifacts(self):
        _, manifest = self.staged()
        self.protocol = False
        item = manifest["artifacts"][0]
        record = item["archives"][0]
        receipt = {key: manifest[key] for key in (
            "schema_version", "repository", "source_sha", "version", "tag", "run_id", "run_attempt", "metadata")}
        receipt.update(kind="original-assets", artifacts=[{**item, "archives": [record]}])
        os.environ.update(RELEASE_MODE="assets", ARTIFACT_SELECTION=json.dumps(receipt))
        paths = list((self.root / "release-assets").iterdir())
        self.seed_release([p for p in paths if p.name != "index.json"])
        self.command("recover", success=False)
        self.assertFalse(self.writes())
        self.seed_release([p for p in paths if p.name != record["name"]])
        self.state["api"][helper.repo_endpoint(f"actions/runs/{RUN}")].update(event="push", head_branch=TAG)
        self.state["api"][helper.repo_endpoint(f"actions/artifacts/{item['id']}")]["expired"] = True
        self.save()
        self.command("recover", success=False)
        self.assertFalse(self.writes())

    def legacy_archive_only_fixture(self):
        _, manifest = self.staged()
        self.protocol = False
        item = next(a for a in manifest["artifacts"] if a["target"] == "x86_64-unknown-linux-gnu")
        item["name"] = f"meta-gateway-binaries-{item['target']}"
        record = item["archives"][0]
        packed = self.root / f"artifact-{item['id']}.zip"
        self.pack(packed, self.target_files[record["name"]].parent,
                  [archive["name"] for archive in item["archives"]])
        item["digest"] = helper.assets.sha256(packed)
        self.add_artifact(item["id"], item["name"], packed)
        receipt = {key: manifest[key] for key in (
            "schema_version", "repository", "source_sha", "version", "tag", "run_id", "run_attempt", "metadata")}
        receipt.update(kind="original-assets", artifacts=[{**item, "archives": [record]}])
        self.state["api"][helper.repo_endpoint(f"actions/runs/{RUN}")].update(event="push", head_branch=TAG)
        self.state["verification"][record["name"]] = verified_fixture(
            record["name"], record["sha256"], ref=f"refs/tags/{TAG}", event="push")
        paths = list((self.root / "release-assets").iterdir())
        self.state["calls"] = []
        self.seed_release([path for path in paths if path.name != record["name"]])
        os.environ.update(RELEASE_MODE="assets", ARTIFACT_SELECTION=json.dumps(receipt))
        return record

    def test_real_shaped_legacy_archive_only_zip_uses_existing_github_attestation(self):
        record = self.legacy_archive_only_fixture()
        before = copy.deepcopy(self.state["release"])
        self.command("recover")
        verifications = [call["args"] for call in self.state["calls"]
                         if call["args"][:2] == ["attestation", "verify"]]
        self.assertEqual(len(verifications), 1)
        args = verifications[0]
        self.assertNotIn("--bundle", args)
        self.assertEqual(args[args.index("--source-digest") + 1], SHA)
        self.assertEqual(args[args.index("--signer-digest") + 1], SHA)
        self.assertEqual(args[args.index("--source-ref") + 1], f"refs/tags/{TAG}")
        self.assertIn("--deny-self-hosted-runners", args)
        self.assertEqual(len(self.writes()), 1)
        self.assertEqual(Path(self.writes()[0]["args"][3]).name, record["name"])
        self.assertEqual(self.state["release"]["body"], before["body"])
        for asset in before["assets"]:
            self.assertIn(asset, self.state["release"]["assets"])

    def test_legacy_archive_only_missing_github_attestation_refuses_before_write(self):
        record = self.legacy_archive_only_fixture()
        del self.state["verification"][record["name"]]
        self.save()
        _, error = self.command("recover", success=False)
        self.assertIn("missing original verified attestation", error)
        self.assertFalse(self.writes())
        verifies = [call["args"] for call in self.state["calls"]
                    if call["args"][:2] == ["attestation", "verify"]]
        self.assertEqual(len(verifies), 1)
        self.assertNotIn("--bundle", verifies[0])

    def test_legacy_remote_attestation_still_requires_original_signed_source_and_attempt(self):
        record = self.legacy_archive_only_fixture()
        original = copy.deepcopy(self.state["verification"][record["name"]])
        for key, value in [
            ("sourceRepositoryDigest", OTHER_SHA),
            ("runInvocationURI", f"https://github.com/{REPO}/actions/runs/{RUN}/attempts/1"),
        ]:
            with self.subTest(key=key):
                changed = copy.deepcopy(original)
                changed[0]["verificationResult"]["signature"]["certificate"][key] = value
                self.state["verification"][record["name"]] = changed
                self.save()
                self.command("recover", success=False)
                self.assertFalse(self.writes())

    def test_candidate_verification_cannot_opt_into_remote_bundle_lookup(self):
        path = next(iter(self.target_files.values()))
        for allow_remote in (False, True):
            with self.subTest(allow_remote=allow_remote), self.assertRaisesRegex(
                    helper.ProtocolError, "missing original"):
                helper.verify_provenance(path, None, SHA, RUN, ATTEMPT, allow_remote=allow_remote)
        self.assertFalse(self.state["calls"])

    def test_invalid_retained_legacy_bundle_never_falls_back_to_remote_lookup(self):
        path = next(iter(self.target_files.values()))
        self.state["verify_failure"] = True
        self.save()
        with self.assertRaisesRegex(helper.ProtocolError, "signature verification failed"):
            helper.verify_provenance(path, path.parent / helper.PROVENANCE, SHA, RUN, ATTEMPT,
                                     ref=f"refs/tags/{TAG}", event="push", allow_remote=True)
        self.load()
        self.assertEqual(len(self.state["calls"]), 1)
        self.assertIn("--bundle", self.state["calls"][0]["args"])

    def test_candidate_asset_recovery_is_add_only_and_does_not_publish_draft(self):
        self.staged()
        paths = [*list((self.root / "release-assets").iterdir()), *list((self.root / "release-proofs").iterdir())]
        missing = next(p for p in paths if p.name.endswith(".zip"))
        self.seed_release([p for p in paths if p != missing], draft=True)
        before = copy.deepcopy(self.state["release"])
        os.environ["RELEASE_MODE"] = "assets"
        self.command("recover")
        self.assertEqual(len(self.writes()), 1)
        self.assertEqual(Path(self.writes()[0]["args"][3]).name, missing.name)
        self.assertTrue(self.state["release"]["draft"])
        for asset in before["assets"]:
            self.assertIn(asset, self.state["release"]["assets"])

    def test_recovery_cannot_create_release_or_publish_packages(self):
        self.candidate()
        os.environ["RELEASE_MODE"] = "assets"
        self.command("recover", success=False)
        self.assertFalse(self.writes())
        os.environ["PUBLISH_RELEASE_PACKAGES"] = "true"
        self.command("recover", success=False)
        self.assertFalse(self.writes())

    def test_resolver_candidate_equality_main_and_boolean_contract(self):
        os.environ.update(RELEASE_TAG="", RELEASE_TIER="core")
        stdout, _ = self.command("resolve")
        value = json.loads(stdout)
        self.assertEqual(value["tag"], TAG)
        self.assertEqual(value["ci_bypass"], "false")
        self.assertEqual(value["sha"], SHA)
        self.assertEqual(value["tier"], "core")
        for tier in helper.assets.TIERS:
            with mock.patch.dict(os.environ, {"RELEASE_TIER": tier}):
                stdout, _ = self.command("resolve")
            # One candidate run builds exactly its tier's legs.
            matrix = json.loads(json.loads(stdout)["build_matrix"])
            self.assertEqual([leg["target"] for leg in matrix["include"]],
                             list(helper.assets.TIER_TARGETS[tier]))
        for name, value in [
            ("SOURCE_SHA", ""), ("SOURCE_SHA", OTHER_SHA), ("GITHUB_SHA", OTHER_SHA),
            ("GITHUB_WORKFLOW_SHA", OTHER_SHA), ("GITHUB_REF", "refs/heads/feature"),
            ("RELEASE_TAG", TAG), ("ARTIFACT_SELECTION", "{}"),
            ("REGISTRY_DRY_RUN", "true"), ("PUBLISH_RELEASE_PACKAGES", "true"),
            ("PUBLISH_RELEASE_PACKAGES", "False"), ("RELEASE_TIER", ""), ("RELEASE_TIER", "all"),
        ]:
            with self.subTest(name=name, value=value), mock.patch.dict(os.environ, {name: value}):
                self.command("resolve", success=False)
        self.assertFalse(self.writes())

    def test_resolver_tag_push_ignores_all_dispatch_inputs_and_appends_outputs(self):
        output = self.root / "outputs"
        output.write_text("previous=retained\n")
        os.environ.update(GITHUB_EVENT_NAME="push", GITHUB_REF="refs/tags/" + TAG,
                          RELEASE_MODE="promote", SOURCE_SHA="invalid", ARTIFACT_SELECTION="not json",
                          PUBLISH_RELEASE_PACKAGES="not bool", REGISTRY_DRY_RUN="true",
                          GITHUB_OUTPUT=str(output))
        stdout, _ = self.command("resolve")
        value = json.loads(stdout)
        self.assertEqual(value["mode"], "tag")
        self.assertEqual(value["publish_packages"], "false")
        self.assertEqual(value["dry_run"], "false")
        self.assertEqual(value["ci_bypass"], "false")
        self.assertTrue(output.read_text().startswith("previous=retained\nmode=tag\n"))
        self.assertFalse(self.writes())

    def test_resolver_existing_truth_table_untagged_bypass_only(self):
        os.environ.update(RELEASE_MODE="existing", SOURCE_SHA="")
        for tag in ("", TAG):
            for publish in ("false", "true"):
                for dry in ("false", "true"):
                    with self.subTest(tag=tag, publish=publish, dry=dry):
                        os.environ.update(RELEASE_TAG=tag, PUBLISH_RELEASE_PACKAGES=publish, REGISTRY_DRY_RUN=dry)
                        allowed = bool(tag) or publish == "false" or dry == "true"
                        stdout, _ = self.command("resolve", success=allowed)
                        if allowed:
                            result = json.loads(stdout)
                            self.assertEqual(result["ci_bypass"],
                                             str(not tag and publish == "true" and dry == "true").lower())

    def test_resolver_existing_tag_promote_and_assets_dispatches_require_main(self):
        selected, _ = self.candidate()
        for mode in ("existing", "promote", "assets"):
            os.environ.update(RELEASE_MODE=mode, ARTIFACT_SELECTION="" if mode == "existing" else json.dumps(selected))
            for ref in ("refs/heads/feature", "refs/tags/" + TAG):
                with self.subTest(mode=mode, ref=ref), mock.patch.dict(os.environ, {"GITHUB_REF": ref}):
                    self.command("resolve", success=False)
            self.command("resolve")
        os.environ.update(RELEASE_MODE="existing", RELEASE_TAG="", ARTIFACT_SELECTION="",
                          GITHUB_REF="refs/heads/feature", PUBLISH_RELEASE_PACKAGES="true", REGISTRY_DRY_RUN="true")
        self.command("resolve")

    def test_resolver_rejects_missing_tag_wrong_version_and_unknown_modes(self):
        os.environ.update(RELEASE_MODE="promote", SOURCE_SHA="", RELEASE_TAG="")
        self.command("resolve", success=False)
        os.environ.update(RELEASE_MODE="existing", RELEASE_TAG="v9.9.9")
        self.state["api"][helper.repo_endpoint("git/ref/tags/v9.9.9")] = {"object": {"type": "commit", "sha": SHA}}
        self.save()
        self.command("resolve", success=False)
        os.environ.update(RELEASE_MODE="arbitrary", RELEASE_TAG="")
        self.command("resolve", success=False)
        os.environ.update(RELEASE_MODE="existing", GITHUB_EVENT_NAME="schedule")
        self.command("resolve", success=False)

    def test_dispatch_real_cli_uses_json_stdin_main_owner_auth_no_tag_write(self):
        os.environ.update(RELEASE_TAG="", ARTIFACT_SELECTION="", RELEASE_TIER="trailing")
        result = subprocess.run([sys.executable, str(SCRIPT), "dispatch", "--mode", "candidate"],
                                capture_output=True, text=True, check=False)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.load()
        self.assertEqual(len(self.state["calls"]), 1)
        call = self.state["calls"][0]
        self.assertEqual(call["args"], ["workflow", "run", Path(helper.WORKFLOW).name, "--repo", REPO, "--ref", "main", "--json"])
        self.assertFalse(call["gh_token"])
        self.assertFalse(call["github_api_token"])
        self.assertEqual(call["inputs"]["source_sha"], SHA)
        self.assertEqual(call["inputs"]["release_mode"], "candidate")
        self.assertEqual(call["inputs"]["candidate_tier"], "trailing")
        self.assertIn("tag pushes validate only", result.stderr)
        self.assertNotIn("fixture-workflow-token", result.stdout + result.stderr)
        self.assertFalse(self.writes())

    def test_dispatch_missing_input_invalid_boolean_and_symlink_fail_before_gh(self):
        os.environ.update(RELEASE_TAG="", SOURCE_SHA="")
        self.command("dispatch", "--mode", "candidate", success=False)
        os.environ.update(SOURCE_SHA=SHA, PUBLISH_RELEASE_PACKAGES="yes")
        self.command("dispatch", "--mode", "candidate", success=False)
        os.environ.update(SOURCE_SHA="", PUBLISH_RELEASE_PACKAGES="false", RELEASE_TAG=TAG)
        self.command("dispatch", "--mode", "promote", success=False)
        self.assertFalse(self.state["calls"])
        file = self.root / "selection"
        file.write_text("{}")
        link = self.root / "link"
        link.symlink_to(file)
        os.environ["ARTIFACT_SELECTION_FILE"] = str(link)
        self.command("dispatch", "--mode", "promote", success=False)
        self.assertFalse(self.state["calls"])

    def test_dispatch_receipt_file_and_boolean_routing(self):
        selected, _ = self.candidate()
        file = self.root / "accepted.json"
        file.write_text(json.dumps(selected))
        os.environ.update(ARTIFACT_SELECTION_FILE=str(file), SOURCE_SHA="",
                          PUBLISH_RELEASE_PACKAGES="true", REGISTRY_DRY_RUN="true")
        self.command("dispatch", "--mode", "promote")
        call = self.state["calls"][-1]
        self.assertEqual(json.loads(call["inputs"]["artifact_selection"]), selected)
        self.assertEqual(call["inputs"]["publish_release_packages"], "true")
        self.assertEqual(call["inputs"]["registry_dry_run"], "true")
        self.assertEqual(call["inputs"]["release_tag"], TAG)
        os.environ.update(ARTIFACT_SELECTION_FILE="", PUBLISH_RELEASE_PACKAGES="true")
        self.command("dispatch", "--mode", "existing")
        self.assertEqual(self.state["calls"][-1]["inputs"]["artifact_selection"], "")

    def make_command(self, target, *variables, success=True):
        env = os.environ.copy()
        for key in ("RELEASE_MODE", "SOURCE_SHA", "RELEASE_TAG", "ARTIFACT_SELECTION",
                    "ARTIFACT_SELECTION_FILE", "PUBLISH_RELEASE_PACKAGES", "REGISTRY_DRY_RUN",
                    "RELEASE_TIER", "TIER", "MAKEFLAGS", "MAKEOVERRIDES", "MFLAGS"):
            env.pop(key, None)
        result = subprocess.run(["make", "--no-print-directory", target, *variables],
                                cwd=SCRIPT.resolve().parent.parent, env=env,
                                capture_output=True, text=True, check=False)
        self.load()
        if success:
            self.assertEqual(result.returncode, 0, result.stderr)
        else:
            self.assertNotEqual(result.returncode, 0, result.stdout)
        return result

    def test_make_facades_dispatch_exact_modes_receipts_booleans_with_owner_auth(self):
        accepted = {
            "schema_version": 1, "kind": "accepted-candidate", "repository": REPO,
            "run_id": RUN, "run_attempt": ATTEMPT,
            "manifest_artifact": {"id": 300, "digest": "a" * 64, "manifest_sha256": "b" * 64},
        }
        path = self.root / "accepted candidate;not-a-command.json"
        path.write_text(json.dumps(accepted))
        cases = [
            ("release-candidate", [f"SOURCE_SHA={SHA}", "TIER=core"], "candidate", "false", "false", ""),
            ("release-candidate", [f"SOURCE_SHA={SHA}", "TIER=trailing"], "candidate", "false", "false", ""),
            ("release-promote", [f"RELEASE_TAG={TAG}", f"ARTIFACT_SELECTION={path}"],
             "promote", "true", "false", accepted),
            ("release-promote", [f"RELEASE_TAG={TAG}", f"ARTIFACT_SELECTION={path}",
                                 "PUBLISH_RELEASE_PACKAGES=false", "REGISTRY_DRY_RUN=true"],
             "promote", "false", "true", accepted),
            ("release-registry-retry", [f"RELEASE_TAG={TAG}"], "existing", "true", "false", ""),
            ("release-registry-retry", [f"RELEASE_TAG={TAG}", "REGISTRY_DRY_RUN=true"],
             "existing", "true", "true", ""),
            ("release-assets-recover", [f"RELEASE_TAG={TAG}", f"ARTIFACT_SELECTION={path}"],
             "assets", "false", "false", accepted),
        ]
        for target, variables, mode, publish, dry, selected in cases:
            with self.subTest(target=target, variables=variables):
                before = len(self.state["calls"])
                result = self.make_command(target, *variables)
                self.assertEqual(len(self.state["calls"]), before + 1)
                call = self.state["calls"][-1]
                self.assertEqual(call["args"],
                                 ["workflow", "run", Path(helper.WORKFLOW).name, "--repo", REPO, "--ref", "main", "--json"])
                self.assertEqual(call["inputs"]["release_mode"], mode)
                self.assertEqual(call["inputs"]["publish_release_packages"], publish)
                self.assertEqual(call["inputs"]["registry_dry_run"], dry)
                self.assertEqual(call["inputs"]["source_sha"], SHA if mode == "candidate" else "")
                self.assertEqual(call["inputs"]["release_tag"], "" if mode == "candidate" else TAG)
                self.assertEqual(call["inputs"]["candidate_tier"],
                                 next((v[5:] for v in variables if v.startswith("TIER=")), ""))
                receipt = call["inputs"]["artifact_selection"]
                self.assertEqual(json.loads(receipt) if receipt else "", selected)
                self.assertFalse(call["gh_token"])
                self.assertFalse(call["github_api_token"])
                self.assertIn("tag pushes validate only", result.stderr)
                self.assertNotIn("fixture-workflow-token", result.stdout + result.stderr)
        self.assertFalse(self.writes())

    def test_make_facades_refuse_missing_conflicting_or_invalid_inputs_before_gh(self):
        for target, variables in [
            ("release-candidate", []),
            ("release-candidate", [f"SOURCE_SHA={SHA}"]),
            ("release-candidate", [f"SOURCE_SHA={SHA}", "TIER=all"]),
            ("release-candidate", [f"SOURCE_SHA={SHA}", "TIER=core", "PUBLISH_RELEASE_PACKAGES=true"]),
            ("release-registry-retry", [f"RELEASE_TAG={TAG}", "TIER=core"]),
            ("release-candidate", [f"SOURCE_SHA={SHA}", "PUBLISH_RELEASE_PACKAGES=true"]),
            ("release-candidate", [f"SOURCE_SHA={SHA}", "REGISTRY_DRY_RUN=invalid"]),
            ("release-promote", []),
            ("release-promote", [f"RELEASE_TAG={TAG}"]),
            ("release-registry-retry", []),
            ("release-registry-retry", [f"RELEASE_TAG={TAG}", "REGISTRY_DRY_RUN=yes"]),
            ("release-assets-recover", [f"RELEASE_TAG={TAG}"]),
        ]:
            with self.subTest(target=target, variables=variables):
                self.make_command(target, *variables, success=False)
                self.assertFalse(self.state["calls"])

    def test_make_promote_and_assets_refuse_tier(self):
        # TIER names a candidate's tier. Promotion and asset recovery take the
        # tier from their selection and refuse TIER, as the runbook says: make
        # reads it from the environment too, so a TIER the shell still exports
        # from the candidate step is refused the same way.
        accepted = {
            "schema_version": 1, "kind": "accepted-candidate", "repository": REPO,
            "run_id": RUN, "run_attempt": ATTEMPT,
            "manifest_artifact": {"id": 300, "digest": "a" * 64, "manifest_sha256": "b" * 64},
        }
        path = self.root / "accepted.json"
        path.write_text(json.dumps(accepted))
        for target, tier in (("release-promote", "core"), ("release-assets-recover", "trailing")):
            with self.subTest(target=target):
                result = self.make_command(target, f"RELEASE_TAG={TAG}",
                                           f"ARTIFACT_SELECTION={path}", f"TIER={tier}", success=False)
                self.assertIn("TIER is only valid for candidate", result.stdout + result.stderr)
                self.assertFalse(self.state["calls"])

    def test_make_assets_facade_accepts_explicit_original_selection_and_help_warns(self):
        target = "x86_64-unknown-linux-gnu"
        path = self.target_files[f"meta-gateway-{VERSION}-{target}.tar.gz"]
        original = {
            "schema_version": 1, "kind": "original-assets", "repository": REPO,
            "source_sha": SHA, "version": VERSION, "tag": TAG, "run_id": RUN, "run_attempt": ATTEMPT,
            "metadata": {"index.json": "a" * 64, "checksums.sha256": "b" * 64},
            "artifacts": [{
                "id": 201, "name": f"meta-gateway-binaries-{target}", "digest": "c" * 64,
                "target": target, "archives": [helper.describe_archive(path, VERSION, target)],
            }],
        }
        selection_file = self.root / "original-selection.json"
        selection_file.write_text(json.dumps(original))
        self.make_command("release-assets-recover", f"RELEASE_TAG={TAG}",
                          f"ARTIFACT_SELECTION={selection_file}")
        self.assertEqual(json.loads(self.state["calls"][-1]["inputs"]["artifact_selection"]), original)
        before = len(self.state["calls"])
        result = self.make_command("help")
        self.assertIn("tag pushes validate only", result.stdout.lower())
        self.assertEqual(len(self.state["calls"]), before)
        self.assertFalse(self.writes())

    def test_source_checkout_and_ci_failure_refuse_collection_before_archives(self):
        for key, value in (("head_sha", OTHER_SHA), ("conclusion", "failure"), ("event", "workflow_dispatch"),
                           ("head_branch", "feature"), ("path", helper.WORKFLOW), ("run_attempt", 2)):
            self.state["api"][helper.repo_endpoint(f"actions/runs/{CI}")] = {**self.run_info(ci=True), key: value}
            self.save()
            self.command("collect", "--output-dir", str(self.root / "candidate"),
                         "--ci-run-id", str(CI), "--ci-run-attempt", "1", success=False)
            self.assertFalse(self.writes())
        with mock.patch.object(helper, "git", return_value=OTHER_SHA):
            self.command("collect", "--output-dir", str(self.root / "candidate"),
                         "--ci-run-id", str(CI), "--ci-run-attempt", "1", success=False)


if __name__ == "__main__":
    unittest.main()
