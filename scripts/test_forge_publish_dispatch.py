#!/usr/bin/env python3
"""Tests of scripts/forge-publish-dispatch.py: the publication request it builds from checked records.

No test dispatches: gh is mocked, and running any command refuses."""
from __future__ import annotations

import base64
import contextlib
import copy
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path, PurePosixPath
import shutil
import sys
import unittest
from unittest import mock
import uuid
import zipfile

ROOT = Path(__file__).resolve().parents[1]


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


tool = load("forge_publish_dispatch", ROOT / "scripts/forge-publish-dispatch.py")
forge = tool.forge
REPO = "lukacf/meerkat-meta"
SHA, MERGED = "a" * 40, "c" * 40
RUN, ATTEMPT = 501, 1
VERSION = forge.forge_version()
TARGET = "linux-x86_64"
INPUTS_BYTES = (ROOT / forge.INPUTS).read_bytes()
INPUTS = json.loads(INPUTS_BYTES)


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def manifest_value():
    """A main candidate built from this checkout's release inputs."""
    def package(target, data):
        return {"target": target, "name": forge.package_name(VERSION, target), "size": len(data), "sha256": sha256(data),
                "artifact": {"id": 11, "name": forge.package_artifact_name(target, ATTEMPT), "digest": "e" * 64}}

    return {"schema_version": 1, "kind": "forge-candidate-manifest", "repository": REPO, "source_sha": SHA,
            "version": VERSION, "tag": forge.TAG_PREFIX + VERSION, "run_id": RUN, "run_attempt": ATTEMPT,
            "workflow": {"path": forge.WORKFLOW, "sha": SHA, "ref": forge.MAIN}, "ci": {"run_id": 77, "run_attempt": 1},
            "forge_ci": {"run_id": 610, "run_attempt": 1, "head_sha": SHA, "event": "workflow_dispatch",
                         "tuple": {"trees": {path: "1" * 40 for path in forge.EVIDENCE_TREES},
                                   "files": {path: "2" * 40 for path in forge.EVIDENCE_FILES}},
                         "toolchain": {"declared": {"python": "3.12", "node": "22"},
                                       "backend": {"python": "Python 3.12.12", "node": "v22.21.1"},
                                       "frontend": {"python": "Python 3.12.3", "node": "v22.21.1"}}},
            "inputs_sha256": sha256(INPUTS_BYTES),
            "packages": [package(target, f"{target} package".encode()) for target in sorted(INPUTS["targets"])]}


def selection_value(manifest_bytes):
    return {"schema_version": 1, "kind": "forge-accepted-candidate", "repository": REPO, "run_id": RUN,
            "run_attempt": ATTEMPT, "manifest_artifact": {"id": 90, "digest": "8" * 64,
                                                          "manifest_sha256": sha256(manifest_bytes)}}


def a5_record(target=TARGET):
    """Connect's A5 owner join for this checkout's candidate inputs: no candidate block."""
    entry = INPUTS["targets"][target]
    files, programs, paths = entry["files"], entry["native_programs"], entry["paths"]
    assembly = json.loads((ROOT / files[paths["assembly"]]["path"]).read_bytes())
    archive = files[str(PurePosixPath(paths["assembly"]).parent / assembly["archive"])]
    executable, evidence = files[programs["provider"]["executable"]], files[programs["provider"]["evidence"]]

    def collection_asset(key):
        pin = files[key]
        if "member" in pin:
            return {"key": key, "url": pin["url"], "bytes": pin["archive_size"], "sha256": pin["archive_sha256"]}
        return {"key": key, "url": pin["url"], "bytes": pin["size"], "sha256": pin["sha256"]}

    return {"schema_version": 1, "kind": "connect_a5_owner_join", "disposition": "accepted",
            "owner": "connect-native-mac", "target": target, "scope": "fixture A5 owner join",
            "provider": {"executable": {key: executable[key] for key in ("url", "archive_size", "archive_sha256",
                                                                         "member", "size", "sha256")},
                         "evidence": {key: evidence[key] for key in ("url", "size", "sha256")}},
            "a1_gateway": {"archive": {"url": archive["url"], "size_bytes": archive["size"], "sha256": archive["sha256"]},
                           "member": {"path": f"meerkat-connect-{target}/bin/toolkit_gateway",
                                      "sha256": assembly["binary_sha256"], "size": assembly["binary_bytes"]}},
            "resource_manifest_sha256": assembly["resources_sha256"],
            "collection_public_assets": [collection_asset(key) for key in programs["collection"].values()]}


def candidate_join(kind, selection, manifest, target=TARGET):
    packages = forge.manifest_packages(manifest)
    return {"schema_version": 1, "kind": kind, "disposition": "accepted", "owner": "connect-native-mac",
            "target": target,
            "candidate": {"run_id": selection["run_id"], "run_attempt": selection["run_attempt"],
                          "manifest_sha256": selection["manifest_artifact"]["manifest_sha256"]},
            "subjects": {"package_sha256": packages[target], "release_inputs_sha256": manifest["inputs_sha256"]},
            "evidence": [{"role": "observation", "path": "fixture/observation.json", "sha256": "5d" * 32, "size_bytes": 10}],
            "scope": "fixture candidate acceptance", "limitations": []}


def projection_value(selection, manifest, joins):
    return {"schema": forge.REVIEW_SCHEMA,
            "candidate": {"run_id": selection["run_id"], "run_attempt": selection["run_attempt"],
                          "manifest_sha256": selection["manifest_artifact"]["manifest_sha256"],
                          "packages": forge.manifest_packages(manifest)},
            "joins": {slot: sha256(data) for slot, data in joins.items()},
            "outcomes": {slot: "accepted" for slot in joins}, "verdict": "accepted"}


@contextlib.contextmanager
def fixture_directory():
    directory = ROOT / f".forge-dispatch-fixture-{uuid.uuid4().hex}"
    directory.mkdir()
    previous = Path.cwd()
    os.chdir(directory)
    try:
        yield directory
    finally:
        os.chdir(previous)
        shutil.rmtree(directory)


class DispatchTests(unittest.TestCase):
    def run_tool(self, *, join=None, projection=None, expected=None, main_head=MERGED, remove=None):
        """join(slot, record) and projection(value) change the records before they are written;
        expected(digests) changes the digests given to the tool. Returns (rc, stdout, stderr, files)."""
        manifest = manifest_value()
        manifest_bytes = forge.rc.json_bytes(manifest)
        selection = selection_value(manifest_bytes)
        records = {"a5_join": a5_record(),
                   "application_join": candidate_join("connect_application_candidate_join", selection, manifest),
                   "guide_join": candidate_join("connect_guide_candidate_join", selection, manifest)}
        if join is not None:
            for slot, record in records.items():
                join(slot, record)
        joins = {slot: json.dumps(record, indent=1).encode() for slot, record in records.items()}
        review = projection_value(selection, manifest, joins)
        if projection is not None:
            projection(review)
        review_bytes = json.dumps(review, indent=1).encode()
        with fixture_directory() as directory:
            files = {"projection": directory / "projection.json", **{slot: directory / f"{slot}.json" for slot in joins}}
            files["projection"].write_bytes(review_bytes)
            for slot, data in joins.items():
                files[slot].write_bytes(data)
            digests = {name: sha256(path.read_bytes()) for name, path in files.items()}
            if expected is not None:
                expected(digests)
            if remove is not None:
                files[remove].unlink()
            argv = ["--run-id", str(RUN), "--run-attempt", str(ATTEMPT), "--projection", str(files["projection"]),
                    "--projection-sha256", digests["projection"], "--merged-sha", MERGED, "--dry-run"]
            for slot in forge.REVIEW_JOINS:
                flag = slot.replace("_", "-")
                argv += [f"--{flag}", str(files[slot]), f"--{flag}-sha256", digests[slot]]

            def api(endpoint, *, pages=False):
                if endpoint.endswith(f"actions/runs/{RUN}"):
                    return {"head_sha": SHA}
                if endpoint.endswith("branches/main"):
                    return {"commit": {"sha": main_head}}
                raise AssertionError(endpoint)

            def download(identity, run_info, work):
                path = work / "manifest.zip"
                with zipfile.ZipFile(path, "w") as archive:
                    archive.writestr(forge.MANIFEST, manifest_bytes)
                    archive.writestr(forge.MANIFEST_PROVENANCE, b"bundle")
                return path

            def git_bytes(sha, path):
                self.assertEqual(sha, SHA)
                return (ROOT / path).read_bytes()

            def run(argv, **_):
                raise AssertionError(f"no command runs in these tests: {argv}")

            stdout, stderr = io.StringIO(), io.StringIO()
            with mock.patch.object(forge.rc, "api", api), \
                    mock.patch.object(forge.rc, "verify_run", lambda *a, **k: {"id": RUN, "head_sha": SHA}), \
                    mock.patch.object(forge, "candidate_artifacts",
                                      lambda run_id, names: {names[0]: {"id": 90, "digest": "sha256:" + "8" * 64}}), \
                    mock.patch.object(forge.rc, "download_artifact", download), \
                    mock.patch.object(tool, "git_bytes", git_bytes), mock.patch.object(forge.rc, "run", run), \
                    mock.patch.dict(os.environ, {"GITHUB_REPOSITORY": REPO}), \
                    contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
                code = tool.main(argv)
        return code, stdout.getvalue(), stderr.getvalue(), {"selection": selection, "review": review_bytes}

    def refused(self, message, **options):
        code, stdout, stderr, _ = self.run_tool(**options)
        self.assertEqual(code, 2, stderr)
        self.assertIn(message, stderr)
        self.assertEqual(stdout, "")

    def test_the_request_comes_only_from_the_checked_records(self):
        code, stdout, stderr, expected = self.run_tool()
        self.assertEqual((code, stderr), (0, ""))
        request = json.loads(stdout)
        self.assertEqual((request["workflow"], request["repository"], request["ref"]), ("forge-release.yml", REPO, "main"))
        inputs = request["inputs"]
        self.assertEqual(set(inputs), {"release_mode", "artifact_selection", "candidate_review", "candidate_review_sha256"})
        self.assertEqual(inputs["release_mode"], "publish")
        self.assertEqual(json.loads(inputs["artifact_selection"]), expected["selection"])
        self.assertEqual(base64.b64decode(inputs["candidate_review"]), expected["review"])
        self.assertEqual(inputs["candidate_review_sha256"], sha256(expected["review"]))
        # The private join records stay private: none of their contents reach the request.
        self.assertNotIn("fixture/observation.json", stdout)
        self.assertNotIn("connect_a5_owner_join", stdout)

    def test_consistently_replaced_join_digests_with_unchanged_records_refuse(self):
        # Connect's control: the records are unchanged, but the projection names another A5 digest
        # and the caller's expected digest says the same; no file with that digest passes.
        other = "9e" * 32

        def projection(value):
            value["joins"]["a5_join"] = other

        def expected(digests):
            digests["a5_join"] = other

        self.refused("a5_join record bytes differ from their expected SHA-256", projection=projection, expected=expected)
        self.refused("covers another a5_join record", projection=projection)

    def test_a_missing_join_refuses(self):
        self.refused("candidate review joins fields", projection=lambda value: value["joins"].pop("guide_join"))
        self.refused("guide_join record is not a regular file", remove="guide_join")

    def test_a_join_that_is_not_accepted_refuses(self):
        def failed(slot, record):
            if slot == "application_join":
                record["disposition"] = "failed"

        self.refused("application_join record is not accepted", join=failed)
        self.refused("a5_join outcome is not accepted",
                     projection=lambda value: value["outcomes"].__setitem__("a5_join", "unresolved"))

    def test_another_candidate_refuses(self):
        def other_run(slot, record):
            if slot == "guide_join":
                record["candidate"]["run_id"] = RUN + 7

        self.refused("guide_join accepts another candidate", join=other_run)
        self.refused("accepts another candidate than the selection",
                     projection=lambda value: value["candidate"].__setitem__("run_id", RUN + 7))

    def test_a_review_that_does_not_cover_a_join_refuses(self):
        self.refused("covers another guide_join record",
                     projection=lambda value: value["joins"].__setitem__("guide_join", value["joins"]["a5_join"]))

    def test_a_review_that_is_not_accepted_refuses(self):
        self.refused("verdict is not accepted", projection=lambda value: value.__setitem__("verdict", "unresolved"))

    def test_subjects_and_components_must_be_the_candidates(self):
        def package(slot, record):
            if slot == "application_join":
                record["subjects"]["package_sha256"] = "6c" * 32

        def provider(slot, record):
            if slot == "a5_join":
                record["provider"]["executable"]["archive_sha256"] = "6c" * 32

        def a5_candidate(slot, record):
            if slot == "a5_join":
                record["candidate"] = {"run_id": RUN}

        self.refused("accepts another linux-x86_64 package", join=package)
        self.refused("a5_join provider program differs from the candidate's inputs", join=provider)
        self.refused("carries a candidate block", join=a5_candidate)

    def test_a_join_for_another_target_refuses_even_when_consistent(self):
        # Connect's C1 control, through the record-reading dispatcher: either slot's record names
        # macos-arm64 with that target's actual package digest. Every file digest and the
        # projection's join digests are recomputed from the changed records, so only the
        # agreed target can refuse it.
        # Mac in either slot alone, and in both; the Linux A5 owner join is kept every time.
        for slots in (("application_join",), ("guide_join",), ("application_join", "guide_join")):
            def mac(name, record, slots=slots):
                if name in slots:
                    record["target"] = "macos-arm64"
                    record["subjects"]["package_sha256"] = forge.manifest_packages(manifest_value())["macos-arm64"]

            with self.subTest(slots=slots):
                self.refused(f"{slots[0]} record must cover linux-x86_64", join=mac)

    def test_the_a5_owner_join_must_cover_the_same_linux_target(self):
        def mac(name, record):
            if name == "a5_join":
                record["target"] = "macos-arm64"

        self.refused("a5_join record must cover linux-x86_64", join=mac)

    def test_every_join_must_cover_the_same_target(self):
        # Were a kind ever allowed for another target, the joins must still cover one target.
        def mac(name, record):
            if name == "guide_join":
                record["target"] = "macos-arm64"
                record["subjects"]["package_sha256"] = forge.manifest_packages(manifest_value())["macos-arm64"]

        kinds = copy.deepcopy(tool.JOIN_KINDS)
        kinds["guide_join"]["connect_guide_candidate_join"]["target"] = "macos-arm64"
        with mock.patch.object(tool, "JOIN_KINDS", kinds):
            self.refused("the joins cover different targets", join=mac)

    def test_an_unknown_kind_refuses(self):
        def unknown(slot, record):
            if slot == "guide_join":
                record["kind"] = "connect_guide_join_draft"

        self.refused("is not one this tool checks", join=unknown)

    def test_main_must_be_the_merged_commit(self):
        self.refused("main's head is", main_head="d" * 40)


if __name__ == "__main__":
    unittest.main()
