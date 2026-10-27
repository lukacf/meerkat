#!/usr/bin/env python3
"""Contracts for the warm-deps artifacts (scripts/ci-warm-deps.py): what the
warm workflow may do, which CI jobs extract them with which environment, and
what an extraction accepts."""

import importlib.util
import io
import json
import os
import re
import tarfile
import tempfile
import unittest
import zipfile
from pathlib import Path
from unittest import mock

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("ci_warm_deps", ROOT / "scripts/ci-warm-deps.py")
warm = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(warm)
CI = (ROOT / ".github/workflows/ci.yml").read_text()
WARM = (ROOT / ".github/workflows/warm-deps.yml").read_text()
ACTION = (ROOT / ".github/actions/fetch-warm-deps/action.yml").read_text()
FETCH = "      - uses: ./.github/actions/fetch-warm-deps\n"
PR = {"GITHUB_EVENT_NAME": "pull_request", "GITHUB_REPOSITORY": "o/meta", "GITHUB_HEAD_REF": "rel/x",
      "GITHUB_JOB": "test", "HEAD_REPOSITORY": "o/meta", "PR_HEAD_SHA": "f" * 40}


def jobs(text: str) -> dict[str, str]:
    return dict(re.findall(r"(?m)^  ([a-z][\w-]*):\n((?:(?:    .*)?\n)*)", text.split("\njobs:\n", 1)[1]))


def artifact(members: list[tuple[str, bytes | None, str | None]]) -> Path:
    """A warm-deps artifact zip of (name, file bytes or None for a directory, symlink target)."""
    data = io.BytesIO()
    with tarfile.open(fileobj=data, mode="w", format=tarfile.PAX_FORMAT) as tar:
        for name, content, link in members:
            info = tarfile.TarInfo(name)
            info.mtime = 1_700_000_000.25
            if link:
                info.type, info.linkname = tarfile.SYMTYPE, link
            elif content is None:
                info.type = tarfile.DIRTYPE
            else:
                info.size = len(content)
            tar.addfile(info, io.BytesIO(content) if content else None)
    path = Path(tempfile.mkdtemp()) / "artifact.zip"
    with zipfile.ZipFile(path, "w") as bundle:
        bundle.writestr(warm.TARBALL, data.getvalue())
    return path


class WarmDepsTests(unittest.TestCase):
    def test_every_rust_cache_job_extracts_its_graph_built_with_its_env(self):
        blocks = jobs(CI)
        consumers = {name for name, block in blocks.items() if "Swatinem/rust-cache" in block}
        self.assertEqual(consumers, set(warm.JOB_GRAPHS))
        self.assertEqual(set(warm.JOB_GRAPHS.values()), set(warm.GRAPHS))
        self.assertEqual(CI.count(FETCH), len(consumers))
        for name in sorted(consumers):
            with self.subTest(job=name):
                block = blocks[name]
                # The job token, which can now read actions, stays out of .git/config.
                self.assertRegex(block, r"      - uses: actions/checkout@[0-9a-f]{40} # v4\n"
                                        r"        with:\n          persist-credentials: false\n")
                builds = re.search(r"repo-cargo (build|test|nextest|clippy|check)|make test|npm --prefix console run "
                                   r"fixtures|topology_examples", block)
                # After the cache restore, whose files the artifact overrides, and before any build.
                self.assertLess(block.index("Swatinem/rust-cache"), block.index(FETCH))
                self.assertLess(block.index(FETCH), builds.start())
                env = dict(re.findall(r"(?m)^      (CARGO_PROFILE_\w+|RUSTFLAGS): \"?([^\"\n]*)\"?$",
                                      block.split("    steps:", 1)[0]))
                self.assertEqual(env, warm.GRAPHS[warm.JOB_GRAPHS[name]][0])

    def test_only_the_consumer_jobs_gain_actions_read(self):
        grants = {name: re.search(r"(?m)^    permissions:\n((?:      .*\n)+)", block) for name, block in jobs(CI).items()}
        expected = {name: "      actions: read\n      contents: read\n" for name in warm.JOB_GRAPHS}
        expected["drop-superseded-caches"] = "      actions: write\n      contents: read\n"
        self.assertEqual({name: grant.group(1) for name, grant in grants.items() if grant}, expected)
        self.assertIn("\npermissions:\n  contents: read\n\nenv:\n", CI)

    def test_the_warm_workflow_is_a_read_only_dispatch_that_saves_no_cache(self):
        self.assertIn("\non:\n  workflow_dispatch:\n\npermissions:\n  contents: read\n\n", WARM)
        self.assertEqual(WARM.count("permissions:"), 1)
        for forbidden in ("secrets.", "rust-cache", "actions/cache", "inputs:", "ref:", "pull_request"):
            self.assertNotIn(forbidden, WARM)
        graphs = re.search(r"graph: \[([^\]]*)\]", WARM).group(1)
        self.assertEqual([graph.strip() for graph in graphs.split(",")], list(warm.GRAPHS))
        self.assertIn("          path: warm-deps.tar\n          if-no-files-found: error\n"
                      "          retention-days: 3\n", WARM)
        for workflow in (WARM, CI):
            self.assertIn("\n  CARGO_TARGET_DIR: ${{ github.workspace }}/target\n", workflow)

    def test_the_fetch_runs_on_pull_requests_only_bounded_and_never_fails(self):
        self.assertIn("    - if: github.event_name == 'pull_request'\n", ACTION)
        self.assertIn("        PR_HEAD_SHA: ${{ github.event.pull_request.head.sha }}\n", ACTION)
        self.assertIn("timeout -k 10 300 python3 scripts/ci-warm-deps.py fetch\n"
                      "        || echo \"warm dependencies: none", ACTION)
        with mock.patch.object(warm, "gh", side_effect=AssertionError("no API call")):
            for env, outcome in (({"GITHUB_EVENT_NAME": "push"}, "not a pull request"),
                                 ({**PR, "HEAD_REPOSITORY": "fork/meta"}, "a fork's pull request"),
                                 ({**PR, "GITHUB_JOB": "fmt-lint"}, "job fmt-lint has no graph")):
                with self.subTest(outcome=outcome), mock.patch.dict(os.environ, env, clear=True):
                    self.assertEqual(warm.fetch(), outcome)

    def test_fetch_takes_only_this_repositorys_dispatches_on_the_head_branch(self):
        name, calls = "warm-deps-test-x", []
        good = {"id": 5, "event": "workflow_dispatch", "head_branch": "rel/x", "path": warm.WORKFLOW,
                "head_repository": {"full_name": "o/meta"}, "head_sha": "a" * 40}
        runs = [{**good, "id": 1, "event": "push"}, {**good, "id": 2, "head_branch": "main"},
                {**good, "id": 3, "path": ".github/workflows/ci.yml"},
                {**good, "id": 4, "head_repository": {"full_name": "fork/meta"}}, good]

        def gh(path, stdout=None):
            calls.append(path)
            if "/runs?" in path:
                return json.dumps({"workflow_runs": runs}).encode()
            if path.endswith(f"/artifacts?name={name}"):
                return json.dumps({"artifacts": [{"id": 9, "name": name, "expired": False}]}).encode()
            if "/compare/" in path:
                return json.dumps({"status": "ahead", "files": [{"filename": "src/lib.rs"}]}).encode()
            stdout.write(artifact([("deps/a.rlib", b"a", None)]).read_bytes())
            return b""

        with tempfile.TemporaryDirectory() as target, mock.patch.dict(os.environ, {**PR, "CARGO_TARGET_DIR": target}), \
                mock.patch.object(warm, "artifact_name", return_value=name), mock.patch.object(warm, "gh", gh):
            self.assertEqual(warm.fetch(), f"extracted {name} from run 5")
            self.assertTrue((Path(target) / "debug/deps/a.rlib").is_file())
        self.assertIn("branch=rel%2Fx", calls[0])
        self.assertIn(f"repos/o/meta/compare/{'a' * 40}...{'f' * 40}", calls)
        self.assertEqual([path for path in calls if "/runs/" in path], [f"repos/o/meta/actions/runs/5/artifacts?name={name}"])

    def test_a_run_warms_only_its_own_commit_or_a_descendant_with_the_same_builders(self):
        def compare(status, *files):
            reply = json.dumps({"status": status, "files": [dict(f) for f in files]}).encode()
            return mock.patch.object(warm, "gh", return_value=reply)

        source = {"filename": "crates/meerkat-meta/src/lib.rs"}
        for status, files, accepted in (("identical", [], True), ("ahead", [source], True),
                                        ("behind", [], False), ("diverged", [source], False),
                                        ("ahead", [source, {"filename": "scripts/ci-warm-deps.py"}], False),
                                        ("ahead", [{"filename": "x.yml", "previous_filename": warm.WORKFLOW}], False),
                                        ("ahead", [source] * 300, False)):
            with self.subTest(status=status, files=len(files)), compare(status, *files):
                self.assertEqual(warm.lineage_refusal("o/meta", "a" * 40, "f" * 40) is None, accepted)
        # A refused run is skipped, and the job builds cold when none is left.
        run = {"id": 5, "event": "workflow_dispatch", "head_branch": "rel/x", "path": warm.WORKFLOW,
               "head_repository": {"full_name": "o/meta"}, "head_sha": "a" * 40}
        replies = {"runs?": {"workflow_runs": [run]}, "artifacts?": {"artifacts": [{"id": 9, "name": "n", "expired": False}]},
                   "compare/": {"status": "diverged", "files": []}}
        gh = lambda path, stdout=None: json.dumps(next(v for k, v in replies.items() if k in path)).encode()
        with mock.patch.dict(os.environ, PR), mock.patch.object(warm, "artifact_name", return_value="n"), \
                mock.patch.object(warm, "gh", gh):
            self.assertRegex(warm.fetch(), r"^no n from .*; skipped run 5: its commit a{12} is not the head")

    def test_extraction_keeps_mtimes_and_refuses_entries_outside_the_outputs(self):
        with tempfile.TemporaryDirectory() as target, mock.patch.dict(os.environ, {"CARGO_TARGET_DIR": target}):
            self.assertIsNone(warm.extract(artifact([("deps", None, None), ("deps/liba-0123456789abcdef.rlib", b"a", None),
                                                     (".fingerprint/a-0123456789abcdef/lib-a", b"f", None)])))
            self.assertEqual(Path(target, "debug/deps/liba-0123456789abcdef.rlib").stat().st_mtime, 1_700_000_000.25)
            for member, why in ((("../escape", b"e", None), "leaves"), (("/etc/x", b"e", None), "leaves"),
                                (("src/lib.rs", b"e", None), "leaves"), (("deps/../../x", b"e", None), "leaves"),
                                (("deps/link", None, "/etc/passwd"), "not a plain file")):
                with self.subTest(member=member[0]):
                    self.assertIn(why, warm.extract(artifact([member])))
            self.assertFalse(Path(target, "escape").exists() or Path(target, "debug/deps/link").exists())

    def test_build_refuses_dependencies_from_outside_crates_io(self):
        workspace = [{"name": "meerkat-meta", "manifest_path": "Cargo.toml"}]
        for source, manifest, refused in ((f'source = "{warm.CRATES_IO}"', "", False),
                                          ('source = "git+https://example.invalid/x"', "", True),
                                          ("", "", True), (f'source = "{warm.CRATES_IO}"', "[patch.crates-io]\n", True)):
            with self.subTest(source=source, manifest=manifest), tempfile.TemporaryDirectory() as tmp:
                Path(tmp, "Cargo.lock").write_text(f'version = 4\n\n[[package]]\nname = "x"\nversion = "1.0.0"\n{source}\n')
                Path(tmp, "Cargo.toml").write_text(f"[workspace]\n{manifest}")
                if not refused:
                    warm.refuse_foreign_sources(workspace, Path(tmp))
                    continue
                with self.assertRaisesRegex(SystemExit, "refused"):
                    warm.refuse_foreign_sources(workspace, Path(tmp))

    def test_hard_linked_build_scripts_are_packed_and_extracted_under_every_name(self):
        packages = [{"name": "meerkat-meta", "targets": [{"name": "meerkat_meta"}]}]
        with tempfile.TemporaryDirectory() as tmp:
            debug = Path(tmp, "debug")
            script = debug / "build/serde-0123456789abcdef/build_script_build-0123456789abcdef"
            script.parent.mkdir(parents=True)
            script.write_bytes(b"\x7fELF")
            os.link(script, script.with_name("build-script-build"))
            Path(debug, "build/serde-0123456789abcdef/link").symlink_to(script)
            (debug / "deps").mkdir()
            (debug / ".fingerprint").mkdir()
            Path(debug, "deps/libmeerkat_meta-0123456789abcdef.rlib").write_bytes(b"own")
            left_out = warm.pack(debug, packages, Path(tmp, "warm.tar"))
            with tarfile.open(Path(tmp, "warm.tar")) as tar:
                members = {member.name: member for member in tar}
            self.assertEqual(left_out, ["build/serde-0123456789abcdef/link"])
            for name in ("build_script_build-0123456789abcdef", "build-script-build"):
                member = members[f"build/serde-0123456789abcdef/{name}"]
                self.assertTrue(member.isfile() and member.size == 4, name)
            self.assertNotIn("deps/libmeerkat_meta-0123456789abcdef.rlib", members)

    def test_only_dependency_outputs_are_packed(self):
        packages = [{"name": "meerkat-meta", "targets": [{"name": "rpc_gateway"}, {"name": "meerkat_meta"}]}]
        for name, own in (("libmeerkat_meta-0123456789abcdef.rlib", True), ("meerkat-meta-0123456789abcdef", True),
                          ("rpc_gateway-0123456789abcdef", True), ("libtokio-0123456789abcdef.rlib", False),
                          ("tokio-0123456789abcdef", False)):
            with self.subTest(name=name):
                self.assertEqual(warm.workspace_output(name, packages), own)


if __name__ == "__main__":
    unittest.main()
