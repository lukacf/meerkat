#!/usr/bin/env python3
"""Unit tests for scripts/published_crates_standalone.py (no cargo, no network).

The census and the guard run cargo; what they decide from cargo's output is
pure and pinned here: comment stripping, the guard's findings, the test-name
listing parser, the manifest's workspace-only selection and name parity.
"""

from __future__ import annotations

import contextlib
import hashlib
import importlib.util
import io
import json
import os
import pathlib
import shutil
import sys
import tarfile
import tempfile
import unittest
import unittest.mock

SCRIPT = pathlib.Path(__file__).resolve().parent / "published_crates_standalone.py"
spec = importlib.util.spec_from_file_location("published_crates_standalone", SCRIPT)
assert spec is not None and spec.loader is not None
pcs = importlib.util.module_from_spec(spec)
sys.modules["published_crates_standalone"] = pcs
spec.loader.exec_module(pcs)


def kinds(findings):
    return [kind for kind, _line, _excerpt in findings]


class FloorNormalization(unittest.TestCase):
    """The published-floor check's manifests: what `cargo package` would
    write for a product crate built against crates.io."""

    def setUp(self):
        self.root = pathlib.Path("/repo")
        self.crate = self.root / "connect" / "crates" / "app"
        self.kept = {(self.root / "connect" / "crates" / "lib").resolve(): "../lib"}

    def normalize(self, name, spec, workspace_deps=None):
        return pcs.normalized_dependency(name, spec, workspace_deps or {}, self.kept, self.crate, self.root)

    def test_workspace_inheritance_merges_features_and_drops_paths_outside_the_product(self):
        spec = self.normalize("meerkat", {"workspace": True, "features": ["b"]},
                              {"meerkat": {"version": "=0.9.6", "path": "crates/meerkat", "features": ["a"]}})
        self.assertEqual(spec, {"version": "=0.9.6", "features": ["a", "b"]})

    def test_a_product_crate_keeps_its_scratch_path(self):
        spec = self.normalize("lib", {"version": "=0.9.4", "path": "../lib"})
        self.assertEqual(spec, {"version": "=0.9.4", "path": "../lib"})

    def test_a_path_dependency_without_a_version_cannot_be_published(self):
        with self.assertRaises(pcs.UsageError):
            self.normalize("tool", {"path": "../../../crates/tool"})

    def test_an_undeclared_workspace_dependency_is_refused(self):
        with self.assertRaises(pcs.UsageError):
            self.normalize("missing", {"workspace": True})


class FloorSurface(unittest.TestCase):
    def metadata(self, *packages):
        return {"packages": [{"name": name, "targets": [{"kind": [kind]} for kind in kinds]} for name, kinds in packages]}

    def test_libraries_and_binaries_of_the_members_only(self):
        metadata = self.metadata(("app", ["bin", "test"]), ("lib", ["lib", "test"]), ("dep", ["lib"]))
        self.assertEqual(pcs.surface_flags(metadata, {"app", "lib"}), ["--lib", "--bins"])

    def test_no_bins_flag_without_a_binary(self):
        metadata = self.metadata(("lib", ["lib", "example"]))
        self.assertEqual(pcs.surface_flags(metadata, {"lib"}), ["--lib"])

    def test_nothing_to_check_is_a_usage_error(self):
        with self.assertRaises(pcs.UsageError):
            pcs.surface_flags(self.metadata(("only-tests", ["test"])), {"only-tests"})


class FloorResolveProblem(unittest.TestCase):
    def test_an_unmet_requirement_is_named(self):
        stderr = "error: failed to select a version for the requirement `meerkat = \"=0.9.6\"`\ncandidate versions found which didn't match: 0.9.5\n"
        self.assertEqual(pcs.resolve_problem(stderr), 'meerkat = "=0.9.6"')

    def test_an_unpublished_crate_is_named(self):
        stderr = ("    Updating crates.io index\n"
                  "error: no matching package named `meerkat-machine-codegen` found\n"
                  "location searched: crates.io index\n"
                  "required by package `toolkit-catalog v0.9.4 (/ws/toolkit-catalog)`\n")
        self.assertEqual(pcs.resolve_problem(stderr), "meerkat-machine-codegen is not on crates.io")

    def test_any_other_error_reports_cargos_first_error_line(self):
        stderr = "warning: x\nerror: failed to parse manifest\n\nCaused by:\n  bad\n"
        self.assertEqual(pcs.resolve_problem(stderr), "failed to parse manifest")


# A cargo stand-in for the floor stages: `metadata --no-deps` lists a Meta
# product (meta-extension-state, meerkat-meta) and a Core crate; `package`
# writes each selected crate's archive into CARGO_TARGET_DIR/package;
# `metadata` in the scratch workspace rewrites its lock and lists meerkat-core
# from crates.io (or fails as FLOOR_STUB_RESOLVE_ERROR says); `build` and
# `check` succeed. Every call is logged with its working directory.
# FLOOR_STUB_PAD (`crate=bytes`) adds that many incompressible bytes under the
# crate's tests/fixtures, so its archive is at least that large.
FLOOR_CARGO_STUB = r'''
import io, json, os, pathlib, sys, tarfile
args, cwd = sys.argv[1:], pathlib.Path.cwd()
with open(os.environ["FLOOR_STUB_LOG"], "a") as log:
    log.write(json.dumps({"args": args, "cwd": str(cwd)}) + "\n")
root = pathlib.Path(os.environ["FLOOR_STUB_ROOT"])
pad = dict(item.split("=") for item in os.environ.get("FLOOR_STUB_PAD", "").split(",") if item)
crates = {"meta-extension-state": root / "meta/crates/meta-extension-state",
          "meerkat-meta": root / "meta/crates/meerkat-meta", "meerkat-core": root / "crates/meerkat-core"}
def package(name, path, source=None, kinds=("lib",)):
    return {"id": f"{name} 0.8.31", "name": name, "version": "0.8.31", "source": source,
            "manifest_path": str(path / "Cargo.toml"), "targets": [{"kind": [kind]} for kind in kinds]}
if args[:2] == ["metadata", "--no-deps"]:
    packages = [package(name, path) for name, path in crates.items()]
    print(json.dumps({"packages": packages, "workspace_members": [p["id"] for p in packages]}))
elif args[:1] == ["package"]:
    target = pathlib.Path(os.environ["CARGO_TARGET_DIR"]) / "package"
    target.mkdir(parents=True, exist_ok=True)
    for index, word in enumerate(args):
        if args[index - 1] == "-p":
            body = f"[package]\nname = \"{word}\"\nversion = \"0.8.31\"\n".encode()
            with tarfile.open(target / f"{word}-0.8.31.crate", "w:gz") as archive:
                info = tarfile.TarInfo(f"{word}-0.8.31/Cargo.toml")
                info.size = len(body)
                archive.addfile(info, io.BytesIO(body))
                if word in pad:
                    filler = os.urandom(int(pad[word]))
                    info = tarfile.TarInfo(f"{word}-0.8.31/tests/fixtures/recorded.bin")
                    info.size = len(filler)
                    archive.addfile(info, io.BytesIO(filler))
elif args[:1] == ["metadata"]:
    error = os.environ.get("FLOOR_STUB_RESOLVE_ERROR")
    if error:
        print(error, file=sys.stderr)
        sys.exit(101)
    (cwd / "Cargo.lock").write_text("# resolved by the stub\nmeerkat-core 0.8.31 registry\n")
    registry = "registry+https://github.com/rust-lang/crates.io-index"
    print(json.dumps({"packages": [
        package("meta-extension-state", cwd / "meta-extension-state-0.8.31"),
        package("meerkat-meta", cwd / "meerkat-meta-0.8.31", kinds=("lib", "bin")),
        package("meerkat-core", root / "registry/meerkat-core", source=registry)]}))
elif args[:1] in (["build"], ["check"]):
    sys.exit(0)
else:
    print(f"floor cargo stub: unexpected command {args}", file=sys.stderr)
    sys.exit(2)
'''


class FloorFixture:
    """A repository with a Meta product and the cargo stand-in above (no
    network)."""

    def setUp(self):
        self.tmp = pathlib.Path(tempfile.mkdtemp(prefix="floor-no-build-"))
        self.addCleanup(shutil.rmtree, self.tmp)
        self.root = self.tmp / "repo"
        for relative, text in {
            "ci/products.json": json.dumps({"products": [
                {"name": "core", "root": ""},
                {"name": "meta", "root": "meta", "publish_order": ["meta-extension-state", "meerkat-meta"]}]}),
            ".cargo/config.toml": '[env]\nLIBSQLITE3_FLAGS = "-DSQLITE_ENABLE_FTS5"\n',
            "Cargo.lock": "# the repository lock, the seed\n",
            "meta/crates/meta-extension-state/Cargo.toml": "[package]\nname = \"meta-extension-state\"\n",
            "meta/crates/meerkat-meta/Cargo.toml": "[package]\nname = \"meerkat-meta\"\n",
            "crates/meerkat-core/Cargo.toml": "[package]\nname = \"meerkat-core\"\n",
        }.items():
            (self.root / relative).parent.mkdir(parents=True, exist_ok=True)
            (self.root / relative).write_text(text)
        stub = self.tmp / "cargo"
        stub.write_text(f"#!{sys.executable}\n" + FLOOR_CARGO_STUB)
        stub.chmod(0o755)
        self.log = self.tmp / "calls.jsonl"
        self.env = {"CARGO_BIN": str(stub), "FLOOR_STUB_LOG": str(self.log), "FLOOR_STUB_ROOT": str(self.root)}

    def files(self):
        return {path.relative_to(self.tmp): (path.stat().st_mtime_ns, path.read_bytes())
                for path in sorted(self.tmp.rglob("*")) if path.is_file() and path != self.log}

    def run_floor(self, name, *, resolve_only, resolve_error=None, pad=""):
        out = self.tmp / name
        env = dict(self.env, FLOOR_STUB_RESOLVE_ERROR=resolve_error or "", FLOOR_STUB_PAD=pad)
        with unittest.mock.patch.dict(os.environ, env), \
                contextlib.redirect_stdout(io.StringIO()) as stdout:
            status = pcs.floor(self.root, "meta", out, "release", False, resolve_only=resolve_only)
        calls = [json.loads(line) for line in self.log.read_text().splitlines()]
        self.log.write_text("")
        return status, json.loads((out / "floor.json").read_text()), calls, stdout.getvalue(), out


class FloorNoBuild(FloorFixture, unittest.TestCase):
    """`floor --no-build` packages, unpacks and resolves, records the
    resolved lock's digest, builds nothing, publishes nothing and writes only
    under --out (a cargo stand-in, no network)."""

    def test_no_build_resolves_records_the_lock_and_builds_nothing(self):
        status, report, calls, stdout, out = self.run_floor("out", resolve_only=True)
        self.assertEqual(status, 0)
        self.assertEqual((report["stage"], report["problems"]), ("resolve", []))
        self.assertEqual(report["registry_dependencies"], ["meerkat-core 0.8.31"])
        lock = (out / "ws" / "Cargo.lock").read_bytes()
        self.assertTrue(lock.startswith(b"# resolved by the stub"))
        self.assertEqual(report["lock_sha256"], hashlib.sha256(lock).hexdigest())
        self.assertEqual([call["args"][0] for call in calls], ["metadata", "package", "metadata"])
        self.assertIn("--no-verify", calls[1]["args"])
        self.assertFalse([call for call in calls
                          if {"build", "check", "publish", "install", "login", "yank", "owner"} & set(call["args"])])
        self.assertIn("resolution only, nothing built", stdout)

    def test_no_build_writes_nothing_outside_its_output_dir(self):
        before = self.files()
        self.run_floor("out", resolve_only=True)
        after = self.files()
        written = {path for path in after if path.parts[0] == "out"}
        self.assertTrue(written)
        self.assertEqual({path: value for path, value in after.items() if path not in written}, before)

    def test_a_full_floor_resolves_to_the_same_lock_and_then_builds(self):
        _, resolved, _, _, _ = self.run_floor("resolve", resolve_only=True)
        status, built, calls, stdout, _ = self.run_floor("build", resolve_only=False)
        self.assertEqual(status, 0)
        self.assertEqual((built["stage"], built["problems"]), ("build", []))
        self.assertEqual(built["lock_sha256"], resolved["lock_sha256"])
        self.assertEqual(calls[-1]["args"], ["build", "--workspace", "--lib", "--bins"])
        self.assertIn("releasable against published dependencies", stdout)

    def test_no_build_names_a_declaration_crates_io_cannot_resolve(self):
        error = "error: failed to select a version for the requirement `meerkat-core = \"=0.8.31\"`"
        status, report, calls, _, _ = self.run_floor("out", resolve_only=True, resolve_error=error)
        self.assertEqual(status, 1)
        self.assertEqual(report["stage"], "resolve")
        self.assertIn('meerkat-core = "=0.8.31"', report["problems"][0])
        self.assertNotIn("lock_sha256", report)
        self.assertEqual([call["args"][0] for call in calls], ["metadata", "package", "metadata"])


def crate_archive(directory, crate, files, version="0.9.12", compresslevel=9):
    """A crate file as `cargo package` writes it: gzip, every member under
    `<crate>-<version>/`."""
    path = directory / f"{crate}-{version}.crate"
    with tarfile.open(path, "w:gz", compresslevel=compresslevel) as archive:
        for name, body in files.items():
            info = tarfile.TarInfo(f"{crate}-{version}/{name}")
            info.size = len(body)
            archive.addfile(info, io.BytesIO(body))
    return path


class PackageSize(unittest.TestCase):
    """A crate file crates.io would refuse for its size fails before any
    publication, and the failure says what weighs most."""

    def setUp(self):
        self.tmp = pathlib.Path(tempfile.mkdtemp(prefix="package-size-"))
        self.addCleanup(shutil.rmtree, self.tmp)

    def test_the_limit_is_the_registrys_and_the_allowed_maximum_leaves_room(self):
        self.assertEqual(pcs.REGISTRY_MAX_CRATE_BYTES, 10_485_760)
        self.assertEqual(pcs.REGISTRY_MAX_CRATE_BYTES, 10 * 1024 * 1024)
        self.assertEqual(pcs.ALLOWED_MAX_CRATE_BYTES, 9_961_472)
        self.assertEqual(pcs.ALLOWED_MAX_CRATE_BYTES, 19 * 512 * 1024)
        self.assertLess(pcs.ALLOWED_MAX_CRATE_BYTES, pcs.REGISTRY_MAX_CRATE_BYTES)

    def test_a_crate_file_at_the_allowed_maximum_passes_and_one_byte_above_fails(self):
        archive = crate_archive(self.tmp, "demo", {"src/lib.rs": os.urandom(2048)})
        size = archive.stat().st_size
        with unittest.mock.patch.object(pcs, "ALLOWED_MAX_CRATE_BYTES", size):
            self.assertEqual(pcs.package_size_problems({"demo": archive}), [])
        with unittest.mock.patch.object(pcs, "ALLOWED_MAX_CRATE_BYTES", size - 1):
            problems = pcs.package_size_problems({"demo": archive})
        self.assertTrue(problems[0].startswith(f"demo: demo-0.9.12.crate is {size} bytes, 1 above the allowed maximum of {size - 1} bytes"),
                        problems[0])

    def test_the_real_limits_pass_a_crate_file_of_the_allowed_size_and_fail_a_larger_one(self):
        at_most = self.tmp / "fits-0.9.12.crate"
        with at_most.open("wb") as handle:
            handle.truncate(pcs.ALLOWED_MAX_CRATE_BYTES)
        self.assertEqual(pcs.package_size_problems({"fits": at_most}), [])
        # 10 MiB of incompressible fixture: the archive is larger than that.
        heavy = crate_archive(self.tmp, "heavy", {"tests/fixtures/recorded.bin": os.urandom(10 * 1024 * 1024),
                                                 "src/lib.rs": b"pub fn f() {}\n"}, compresslevel=1)
        size = heavy.stat().st_size
        self.assertGreater(size, pcs.REGISTRY_MAX_CRATE_BYTES)
        problems = pcs.package_size_problems({"fits": at_most, "heavy": heavy})
        self.assertIn(f"heavy: heavy-0.9.12.crate is {size} bytes", problems[0])
        self.assertIn("above the allowed maximum of 9961472 bytes (9.5 MiB)", problems[0])
        self.assertIn("crates.io refuses a crate file above 10485760 bytes (10 MiB)", problems[0])
        self.assertEqual(problems[1].split()[0], "tests/")
        self.assertFalse([line for line in problems if line.startswith("fits")])

    def test_the_failure_names_the_heaviest_top_level_entries_first(self):
        files = {
            "Cargo.toml": b"[package]\nname = \"demo\"\n",
            "src/lib.rs": os.urandom(3000),
            "src/bin/tool.rs": os.urandom(500),
            "tests/fixtures/recorded.bin": os.urandom(6000),
            "tests/suite.rs": b"x" * 100,
            # Large on disk, small in the archive: ranked by what it costs there.
            "console-dist/app.js": b"a" * 50_000,
        }
        archive = crate_archive(self.tmp, "demo", files)
        with unittest.mock.patch.object(pcs, "ALLOWED_MAX_CRATE_BYTES", 1):
            headline, *entries = pcs.package_size_problems({"demo": archive})
        self.assertIn("demo: demo-0.9.12.crate", headline)
        self.assertEqual([line.split()[0] for line in entries], ["tests/", "src/", "console-dist/", "Cargo.toml"])
        self.assertIn("6100 unpacked", entries[0])
        self.assertIn("3500 unpacked", entries[1])
        self.assertIn("50000 unpacked", entries[2])
        self.assertEqual(pcs.largest_entries(archive)[0][:2], ("tests/", 6100))

    def test_only_the_heaviest_entries_are_listed(self):
        files = {f"dir{index:02}/file.bin": os.urandom(100 + index) for index in range(pcs.LARGEST_ENTRIES_SHOWN + 4)}
        entries = pcs.largest_entries(crate_archive(self.tmp, "demo", files))
        self.assertEqual(len(entries), pcs.LARGEST_ENTRIES_SHOWN)
        self.assertEqual(entries[0][0], f"dir{pcs.LARGEST_ENTRIES_SHOWN + 3:02}/")

    def test_each_crate_has_exactly_one_archive(self):
        crate_archive(self.tmp, "demo", {"Cargo.toml": b""})
        crate_archive(self.tmp, "demo-extra", {"Cargo.toml": b""})
        self.assertEqual(pcs.packaged_archives(self.tmp, ["demo"]), {"demo": self.tmp / "demo-0.9.12.crate"})
        with self.assertRaises(pcs.UsageError):
            pcs.packaged_archives(self.tmp, ["demo", "missing"])
        crate_archive(self.tmp, "demo", {"Cargo.toml": b""}, version="0.9.13")
        with self.assertRaises(pcs.UsageError):
            pcs.packaged_archives(self.tmp, ["demo"])

    def test_a_census_that_packages_an_oversized_crate_fails(self):
        too_large = pcs.OversizedPackage(["demo: demo-0.9.12.crate is 2 bytes, 1 above the allowed maximum"])
        with unittest.mock.patch.object(pcs, "release_crates", return_value=["demo"]), \
                unittest.mock.patch.object(pcs, "census", side_effect=too_large), \
                contextlib.redirect_stderr(io.StringIO()) as stderr:
            self.assertEqual(pcs.main(["census", "--out", str(self.tmp / "census")]), 1)
        self.assertIn("FAIL demo: demo-0.9.12.crate is 2 bytes", stderr.getvalue())


class PackageSizeCommand(unittest.TestCase):
    """`package-size` weighs the archives another step packaged: every crate
    the product publishes, and nothing for a product that publishes none."""

    def setUp(self):
        self.tmp = pathlib.Path(tempfile.mkdtemp(prefix="package-size-command-"))
        self.addCleanup(shutil.rmtree, self.tmp)
        self.root = self.tmp / "repo"
        (self.root / "ci").mkdir(parents=True)
        (self.root / "ci" / "products.json").write_text(json.dumps({"products": [
            {"name": "core", "root": "", "publish_order": "scripts/release-rust-crates.sh"},
            {"name": "meta", "root": "meta", "publish_order": ["meta-extension-state", "meerkat-meta"]},
            {"name": "connect", "root": "connect", "publish_order": []}]}))
        self.packages = self.tmp / "target" / "package"
        self.packages.mkdir(parents=True)

    def run_command(self, product, directory=None):
        with contextlib.redirect_stdout(io.StringIO()) as stdout, contextlib.redirect_stderr(io.StringIO()) as stderr:
            status = pcs.package_size(self.root, product, directory or self.packages)
        return status, stdout.getvalue(), stderr.getvalue()

    def test_every_published_crate_within_the_allowed_size_passes(self):
        small = crate_archive(self.packages, "meta-extension-state", {"src/lib.rs": b"pub fn f() {}\n"})
        large = crate_archive(self.packages, "meerkat-meta", {"src/lib.rs": os.urandom(4096)})
        status, stdout, stderr = self.run_command("meta")
        self.assertEqual((status, stderr), (0, ""))
        self.assertIn(f"{small.stat().st_size} bytes", stdout)
        self.assertIn(f"{large.stat().st_size} bytes", stdout)

    def test_an_oversized_crate_fails_and_is_named(self):
        crate_archive(self.packages, "meta-extension-state", {"src/lib.rs": b"pub fn f() {}\n"})
        large = crate_archive(self.packages, "meerkat-meta", {"src/lib.rs": b"pub fn f() {}\n",
                                                             "tests/fixtures/recorded.bin": os.urandom(4096)})
        with unittest.mock.patch.object(pcs, "ALLOWED_MAX_CRATE_BYTES", 2000):
            status, _, stderr = self.run_command("meta")
        self.assertEqual(status, 1)
        self.assertIn(f"FAIL meerkat-meta: meerkat-meta-0.9.12.crate is {large.stat().st_size} bytes", stderr)
        self.assertIn("tests/", stderr)
        self.assertNotIn("FAIL meta-extension-state", stderr)

    def test_a_missing_archive_is_not_a_pass(self):
        crate_archive(self.packages, "meta-extension-state", {"src/lib.rs": b""})
        with self.assertRaises(pcs.UsageError):
            self.run_command("meta")
        with unittest.mock.patch.object(pcs, "ROOT", self.root), contextlib.redirect_stderr(io.StringIO()) as stderr:
            status = pcs.main(["package-size", "--product", "meta", "--package-dir", str(self.packages)])
        self.assertEqual(status, 2)
        self.assertIn("expected one archive for meerkat-meta", stderr.getvalue())

    def test_a_product_that_publishes_no_crate_has_nothing_to_weigh(self):
        status, stdout, _ = self.run_command("connect", self.tmp / "never-packaged")
        self.assertEqual(status, 0)
        self.assertIn("connect publishes no crate", stdout)


class FloorPackageSize(FloorFixture, unittest.TestCase):
    """The floor check weighs the crates it packages, before it unpacks,
    resolves or builds anything."""

    def test_an_oversized_package_fails_the_floor_before_resolution(self):
        with unittest.mock.patch.object(pcs, "ALLOWED_MAX_CRATE_BYTES", 2000):
            status, report, calls, stdout, out = self.run_floor("out", resolve_only=False, pad="meerkat-meta=4096")
        self.assertEqual(status, 1)
        self.assertEqual(report["stage"], "package")
        size = (out / "target" / "package" / "meerkat-meta-0.8.31.crate").stat().st_size
        self.assertTrue(report["problems"][0].startswith(f"meerkat-meta: meerkat-meta-0.8.31.crate is {size} bytes"),
                        report["problems"][0])
        self.assertEqual(report["problems"][1].split()[0], "tests/")
        self.assertEqual([call["args"][0] for call in calls], ["metadata", "package"])
        self.assertFalse((out / "ws").exists())
        self.assertIn("not releasable (package)", stdout)
        self.assertIn("meerkat-meta: meerkat-meta-0.8.31.crate", stdout)

    def test_the_floor_report_records_the_packaged_sizes(self):
        status, report, _, _, out = self.run_floor("out", resolve_only=True, pad="meerkat-meta=4096")
        self.assertEqual(status, 0)
        packages = out / "target" / "package"
        self.assertEqual(report["package_bytes"], {
            "meta-extension-state": (packages / "meta-extension-state-0.8.31.crate").stat().st_size,
            "meerkat-meta": (packages / "meerkat-meta-0.8.31.crate").stat().st_size})
        self.assertGreater(report["package_bytes"]["meerkat-meta"], 4096)


class PullRequestLaneWeighsThePackages(unittest.TestCase):
    """ci.yml's published-floor job: its floor step is advisory on a pull
    request, so the size check is a step of its own that is not, over the
    archives the floor step packaged."""

    def job(self):
        text = (SCRIPT.parent.parent / ".github" / "workflows" / "ci.yml").read_text()
        body = text.split("\n  published-floor:\n", 1)[1]
        end = [index for index, line in enumerate(body.splitlines()) if line.startswith("  ") and not line.startswith("   ")]
        return "\n".join(body.splitlines()[: end[0]] if end else body.splitlines())

    def steps(self):
        return ("\n" + self.job()).split("\n      - ")[1:]

    def test_a_required_step_weighs_what_the_floor_step_packaged(self):
        steps = self.steps()
        floor = [index for index, step in enumerate(steps) if "published_crates_standalone.py floor " in step]
        size = [index for index, step in enumerate(steps) if "published_crates_standalone.py package-size " in step]
        self.assertEqual((len(floor), len(size)), (1, 1))
        self.assertGreater(size[0], floor[0])
        self.assertIn('--out "${RUNNER_TEMP}/floor-${PRODUCT}"', steps[floor[0]])
        self.assertIn('package-size --product "${PRODUCT}" --package-dir "${RUNNER_TEMP}/floor-${PRODUCT}/target/package"',
                      steps[size[0]])
        self.assertIn("continue-on-error:", steps[floor[0]])
        self.assertNotIn("continue-on-error", steps[size[0]])
        self.assertNotIn("\n        if:", steps[size[0]])
        self.assertEqual(self.job().count("cargo package"), 0)


class StripComments(unittest.TestCase):
    def test_comments_blanked_strings_kept_and_lines_preserved(self):
        src = (
            '//! #[path = "a.rs"] mod x;\n'
            'let s = "// not a comment"; /* block\n'
            ' #[path = "b.rs"] */ let c = \'"\';\n'
            'let r = r#"x // y "#; #[path = "c.rs"] mod y;\n'
        )
        out = pcs.strip_comments(src)
        self.assertEqual(out.count("\n"), src.count("\n"))
        self.assertNotIn('#[path = "a.rs"]', out)
        self.assertNotIn('#[path = "b.rs"]', out)
        self.assertIn('"// not a comment"', out)
        self.assertIn('r#"x // y "#', out)
        self.assertIn('#[path = "c.rs"]', out)

    def test_nested_block_comment(self):
        out = pcs.strip_comments("a /* x /* y */ z */ b")
        self.assertEqual(out.split(), ["a", "b"])


class ScanSource(unittest.TestCase):
    PACKAGED = {"src/lib.rs", "src/help.rs", "embedded/skill.md", "tests/support/mod.rs", "tests/a.rs"}

    def scan(self, relative, text):
        return pcs.scan_source(self.PACKAGED, relative, pcs.strip_comments(text))

    def test_include_of_a_packaged_file_is_clean(self):
        self.assertEqual(self.scan("src/help.rs", 'include_str!("../embedded/skill.md")'), [])

    def test_include_leaving_the_package_is_an_escape(self):
        self.assertEqual(kinds(self.scan("src/help.rs", 'include_str!("../../docs/x.md")')), ["include-escape"])

    def test_include_of_an_unpackaged_file_inside_the_root_is_an_escape(self):
        self.assertEqual(kinds(self.scan("src/help.rs", 'include_bytes!("../tests/fixtures/x.bin")')), ["include-escape"])

    def test_include_from_manifest_dir(self):
        ok = 'include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/embedded/skill.md"))'
        bad = 'include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../x.md"))'
        self.assertEqual(kinds(self.scan("src/lib.rs", ok)), [])
        self.assertEqual(kinds(self.scan("src/lib.rs", bad)), ["include-escape"])

    def test_path_attribute(self):
        self.assertEqual(self.scan("tests/a.rs", '#[path = "support/mod.rs"] mod support;'), [])
        self.assertEqual(
            kinds(self.scan("tests/a.rs", '#[path = "../../../tests/fixtures/live/support.rs"] mod s;')),
            ["path-escape"],
        )

    def test_path_attribute_in_a_doc_comment_is_not_a_finding(self):
        self.assertEqual(self.scan("tests/a.rs", '//! declare `#[path = "../../x.rs"] mod x;`\n'), [])

    def test_manifest_dir_walked_up(self):
        parent = 'let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();'
        dotdot = 'let f = Path::new(env!("CARGO_MANIFEST_DIR")).join("../other/src/x.rs");'
        own = 'let f = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/handles");'
        self.assertEqual(kinds(self.scan("tests/a.rs", parent)), ["manifest-dir-walk"])
        self.assertEqual(kinds(self.scan("tests/a.rs", dotdot)), ["manifest-dir-walk"])
        self.assertEqual(self.scan("tests/a.rs", own), [])

    def test_manifest_dir_statement_ends_at_semicolon(self):
        text = 'let own = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");\nlet up = own.parent();'
        self.assertEqual(self.scan("tests/a.rs", text), [])

    def test_manifest_dir_joined_with_a_constant_walks_where_the_constant_does(self):
        use = 'fn t() { let f = Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURE); }\n'
        up = 'const FIXTURE: &str =\n    "../../crates/other/tests/fixtures/x.json";\n' + use
        own = 'const FIXTURE: &str = "tests/fixtures/x.json";\n' + use
        unused = 'const OTHER: &str = "../../x.json";\n' + use
        self.assertEqual(kinds(self.scan("src/lib.rs", up)), ["manifest-dir-walk"])
        self.assertEqual(self.scan("src/lib.rs", own), [])
        self.assertEqual(self.scan("src/lib.rs", unused), [])

    def test_root_helper_and_skip_when_missing(self):
        self.assertEqual(kinds(self.scan("tests/a.rs", "fn workspace_root() -> PathBuf { todo!() }")), ["root-helper"])
        self.assertEqual(
            kinds(self.scan("tests/a.rs", 'eprintln!("Skipping: rkat binary not found (build it)");')),
            ["skip-when-missing"],
        )
        self.assertEqual(self.scan("tests/a.rs", 'eprintln!("Skipping: Nushell not installed");'), [])


    def test_code_the_package_compiles_out_is_not_scanned(self):
        walk = 'let r = Path::new(env!("CARGO_MANIFEST_DIR")).join("../other/fixtures");'
        text = (
            '#[cfg(not(target_arch = "wasm32"))]\n'
            '#[cfg(feature = "workspace-tests")]\n'
            "fn stage() -> Result<PathBuf, Box<dyn Error>> {\n"
            "    let s = \"}\"; let c = '}'; let raw = r#\"}\"#;\n"
            f"    {walk}\n"
            "}\n"
            '#[cfg(all(test, feature = "workspace-tests"))]\n'
            "#[tokio::test]\n"
            f"async fn gated_test() {{ {walk} }}\n"
            '#[cfg(any(feature = "workspace-tests", all(unix, feature = "workspace-tests")))]\n'
            f"mod gated {{ fn up() {{ {walk} }} }}\n"
            '#[cfg(feature = "workspace-tests")]\n'
            "use other::{a, b};\n"
            f"fn kept() {{ {walk} }}\n"
        )
        findings = self.scan("src/lib.rs", text)
        self.assertEqual([(kind, line) for kind, line, _ in findings], [("manifest-dir-walk", 14)])

    def test_a_cfg_that_does_not_need_the_feature_keeps_the_code(self):
        walk = 'let r = Path::new(env!("CARGO_MANIFEST_DIR")).parent();'
        for cfg in (
            'not(feature = "workspace-tests")',
            'any(test, feature = "workspace-tests")',
            'feature = "workspace-tests-extra"',
            "test",
            'all(test, not(feature = "workspace-tests"))',
        ):
            with self.subTest(cfg=cfg):
                text = f"#[cfg({cfg})]\nfn f() {{ {walk} }}\n"
                self.assertEqual(kinds(self.scan("src/lib.rs", text)), ["manifest-dir-walk"])
        statement = f'fn f() {{\n    #[cfg(feature = "workspace-tests")]\n    {walk}\n}}\n'
        self.assertEqual(kinds(self.scan("src/lib.rs", statement)), ["manifest-dir-walk"])

    def test_a_file_its_leading_inner_attribute_compiles_out(self):
        walk = 'fn f() { let r = Path::new(env!("CARGO_MANIFEST_DIR")).parent(); }\n'
        gated = '#![allow(dead_code)]\n#![cfg(all(feature = "workspace-tests", unix))]\n' + walk
        self.assertEqual(self.scan("tests/a.rs", gated), [])
        late = walk + 'mod m { #![cfg(feature = "workspace-tests")] }\n'
        self.assertEqual(kinds(self.scan("tests/a.rs", late)), ["manifest-dir-walk"])

    def test_cfg_requires(self):
        feature = "workspace-tests"
        for predicate, needs in (
            ('feature = "workspace-tests"', True),
            ('all(test, feature = "workspace-tests")', True),
            ('any(feature = "workspace-tests", all(feature = "workspace-tests", unix))', True),
            ('any(test, feature = "workspace-tests")', False),
            ('not(feature = "workspace-tests")', False),
            ("any()", False),
            ('feature = "workspace-tests", test', False),
            ('feature = workspace-tests', False),
        ):
            with self.subTest(predicate=predicate):
                self.assertEqual(pcs.cfg_requires(predicate, feature), needs)


class ParseListing(unittest.TestCase):
    OUTPUT = (
        "   Compiling demo v0.9.4\n"
        "     Running unittests src/lib.rs (target/debug/deps/demo-0a1b)\n"
        "tests::alpha: test\n"
        "tests::beta: test\n"
        "\n"
        "     Running tests/flows.rs (target/debug/deps/flows-2c3d)\n"
        "flows_run: test\n"
        "bench_like: benchmark\n"
        "     Running unittests src/main.rs (target/debug/deps/demo_bin-4e5f)\n"
        "   Doc-tests demo\n"
        "src/lib.rs - Thing (line 12): test\n"
    )

    def test_names_per_target(self):
        names = pcs.parse_listing(self.OUTPUT)
        self.assertEqual(names["src/lib.rs"], {"tests::alpha", "tests::beta"})
        self.assertEqual(names["tests/flows.rs"], {"flows_run"})
        self.assertEqual(names["src/main.rs"], set())
        self.assertEqual(names[pcs.DOC_TARGET], {"src/lib.rs - Thing (line 12)"})


class DocTestNames(unittest.TestCase):
    def test_package_prefix_is_stripped_on_both_sides(self):
        workspace = pcs.parse_listing(
            "   Doc-tests demo\ncrates/demo/src/lib.rs - Thing (line 3): test\n", "crates/demo"
        )
        standalone = pcs.parse_listing("   Doc-tests demo\ndemo-0.9.4/src/lib.rs - Thing (line 3): test\n", "demo-0.9.4")
        self.assertEqual(workspace, standalone)
        self.assertEqual(workspace[pcs.DOC_TARGET], {"src/lib.rs - Thing (line 3)"})


class Parity(unittest.TestCase):
    ENTRY = {
        "excluded": [{"path": "tests/repo_gate.rs", "reason": "reads the repository"}],
        "workspace_only": [
            {"target": "src/lib.rs", "modules": ["resolver::workspace_tests"], "reason": "needs meerkat-runtime"},
            {"target": "src/lib.rs", "tests": ["tests::reads_sibling_fixture"], "reason": "sibling fixture"},
        ],
    }
    WORKSPACE = {
        "src/lib.rs": {
            "tests::a",
            "resolver::workspace_tests::x",
            "resolver::workspace_tests::y",
            "tests::reads_sibling_fixture",
        },
        "tests/repo_gate.rs": {"gate"},
        "tests/kept.rs": {"kept"},
    }

    def test_declared_differences_are_parity(self):
        standalone = {"src/lib.rs": {"tests::a"}, "tests/kept.rs": {"kept"}}
        self.assertEqual(pcs.parity(self.ENTRY, self.WORKSPACE, standalone), [])

    def test_a_lost_test_fails(self):
        standalone = {"src/lib.rs": set(), "tests/kept.rs": {"kept"}}
        self.assertEqual(
            pcs.parity(self.ENTRY, self.WORKSPACE, standalone),
            ["src/lib.rs: tests::a runs in the workspace but not standalone"],
        )

    def test_a_vacuous_target_fails(self):
        # A file gated on a feature only the stripped self dev-dep enabled.
        standalone = {"src/lib.rs": {"tests::a"}, "tests/kept.rs": set()}
        self.assertEqual(
            pcs.parity(self.ENTRY, self.WORKSPACE, standalone),
            ["tests/kept.rs: kept runs in the workspace but not standalone"],
        )

    def test_a_test_only_standalone_runs_fails(self):
        standalone = {"src/lib.rs": {"tests::a", "tests::extra"}, "tests/kept.rs": {"kept"}}
        self.assertEqual(
            pcs.parity(self.ENTRY, self.WORKSPACE, standalone),
            ["src/lib.rs: tests::extra runs standalone but not in the workspace"],
        )

    def test_equal_counts_with_swapped_names_fail(self):
        standalone = {"src/lib.rs": {"tests::b"}, "tests/kept.rs": {"kept"}}
        self.assertEqual(len(pcs.parity(self.ENTRY, self.WORKSPACE, standalone)), 2)

    def test_module_prefix_is_a_path_not_a_string_prefix(self):
        names = {"resolver::workspace_tests_extra::z", "resolver::workspace_tests::x"}
        self.assertEqual(
            pcs.workspace_only_names(self.ENTRY, "src/lib.rs", names), {"resolver::workspace_tests::x"}
        )

    def test_a_stale_declaration_fails(self):
        # The declared test was renamed and the declared module emptied.
        workspace = {"src/lib.rs": {"tests::a", "tests::reads_sibling_fixture_renamed"}, "tests/kept.rs": {"kept"}}
        standalone = {"src/lib.rs": {"tests::a"}, "tests/kept.rs": {"kept"}}
        self.assertEqual(
            pcs.parity(self.ENTRY, workspace, standalone),
            [
                "src/lib.rs: declared workspace-only module resolver::workspace_tests holds no workspace test",
                "src/lib.rs: declared workspace-only test tests::reads_sibling_fixture is not a workspace test",
                "src/lib.rs: tests::reads_sibling_fixture_renamed runs in the workspace but not standalone",
            ],
        )


class LoadManifest(unittest.TestCase):
    def load(self, text):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            (root / "ci").mkdir()
            (root / "ci" / "standalone-workspace-only.toml").write_text(text)
            return pcs.load_manifest(root)

    def test_grouped_workspace_only_loads(self):
        crates = self.load(
            '[crates.demo]\n'
            '[[crates.demo.workspace_only]]\n'
            'target = "src/lib.rs"\nreason = "needs a cyclic dev-dependency"\n'
            'modules = ["a::tests"]\ntests = ["b::tests::one"]\n'
        )
        self.assertEqual(crates["demo"]["workspace_only"][0]["modules"], ["a::tests"])

    def test_a_workspace_only_entry_needs_a_reason_and_a_name(self):
        with self.assertRaises(pcs.UsageError):
            self.load('[[crates.demo.workspace_only]]\ntarget = "src/lib.rs"\nmodules = ["a"]\n')
        with self.assertRaises(pcs.UsageError):
            self.load('[[crates.demo.workspace_only]]\ntarget = "src/lib.rs"\nreason = "x"\n')


class StandaloneFeatures(unittest.TestCase):
    def test_declared_features(self):
        manifest = {"package": {"metadata": {"meerkat": {"standalone-tests": {"features": ["test-support"]}}}}}
        self.assertEqual(pcs.standalone_features(manifest), ["test-support"])
        self.assertEqual(pcs.standalone_features({"package": {}}), [])


def crate_manifest(standalone=None, features=None, dev=None):
    package = {"name": "demo"}
    if standalone is not None:
        package["metadata"] = {"meerkat": {"standalone-tests": standalone}}
    manifest = {"package": package, "features": features or {}}
    if dev is not None:
        manifest["dev-dependencies"] = dev
    return manifest


class PublishedTestContract(unittest.TestCase):
    def failures(self, manifest, readme=None, packaged=("src/lib.rs",)):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            packaged = set(packaged)
            if readme is not None:
                (root / "README.md").write_text(readme)
                packaged.add("README.md")
            return pcs.published_test_contract_failures("demo", manifest, root, packaged)

    def test_the_command_carries_features_and_stack(self):
        manifest = crate_manifest({"features": ["a", "b"], "min-stack": 8388608}, {"a": [], "b": []})
        self.assertEqual(pcs.test_command(manifest), "RUST_MIN_STACK=8388608 cargo test --features a,b")
        self.assertEqual(pcs.test_command(crate_manifest({"features": ["a"]}, {"a": []})), "cargo test --features a")
        self.assertEqual(pcs.test_command(crate_manifest({"min-stack": 8388608})), "RUST_MIN_STACK=8388608 cargo test")
        self.assertIsNone(pcs.test_command(crate_manifest()))

    def test_a_plain_crate_needs_no_readme(self):
        self.assertEqual(self.failures(crate_manifest()), [])

    def test_the_readme_prints_the_exact_command(self):
        manifest = crate_manifest({"features": ["a"]}, {"a": []})
        self.assertEqual(self.failures(manifest, "Run `cargo test --features a`.\n"), [])
        self.assertEqual(len(self.failures(manifest, "Run `cargo test`.\n")), 1)
        self.assertEqual(len(self.failures(manifest)), 1)

    def test_declared_features_exist_and_never_enable_workspace_tests(self):
        manifest = crate_manifest({"features": ["missing", "workspace-tests"]}, {"workspace-tests": []},
                                  {"demo": {"path": ".", "features": ["workspace-tests"]}})
        readme = "cargo test --features missing,workspace-tests\n"
        self.assertEqual(len(self.failures(manifest, readme)), 2)

    def test_workspace_tests_needs_no_dev_dependency(self):
        # The workspace's test builds pass it (scripts/core-workspace-args.mjs --tests).
        declared = {"workspace-tests": []}
        self.assertEqual(self.failures(crate_manifest(None, declared)), [])


class CargoConfigAbove(unittest.TestCase):
    def test_a_configuration_above_the_directory_is_found(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp) / "repo"
            (root / ".cargo").mkdir(parents=True)
            (root / ".cargo" / "config.toml").write_text("[env]\n")
            self.assertEqual(pcs.cargo_config_above(root / "target" / "census"), root / ".cargo" / "config.toml")
            self.assertIsNone(pcs.cargo_config_above(pathlib.Path(tmp) / "elsewhere"))

    def test_the_users_own_cargo_home_does_not_count(self):
        with tempfile.TemporaryDirectory() as tmp:
            home = pathlib.Path(tmp) / ".cargo"
            home.mkdir()
            (home / "config.toml").write_text("[build]\n")
            previous = os.environ.get("CARGO_HOME")
            os.environ["CARGO_HOME"] = str(home)
            try:
                self.assertIsNone(pcs.cargo_config_above(pathlib.Path(tmp) / "census"))
            finally:
                if previous is None:
                    del os.environ["CARGO_HOME"]
                else:
                    os.environ["CARGO_HOME"] = previous


UNIT_TEST_RULE = """
rust_test(
    name = "demo_crate_unit_test",
    crate = ":demo_crate",
    crate_features = [
{features}
    ],
    deps = [],
)
"""


class BazelWorkspaceTests(unittest.TestCase):
    def failures(self, declared, build=None):
        manifest = crate_manifest(None, declared)
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            if build is not None:
                (root / "BUILD.bazel").write_text(build)
            return pcs.bazel_workspace_tests_failures("demo-crate", manifest, root)

    def rule(self, *features):
        return UNIT_TEST_RULE.format(features="\n".join(f'        "{f}",' for f in features))

    def test_a_crate_without_the_feature_needs_nothing(self):
        self.assertEqual(self.failures({"a": []}), [])

    def test_the_unit_test_target_turns_the_feature_on(self):
        declared = {"workspace-tests": []}
        self.assertEqual(self.failures(declared, self.rule("test-support", "workspace-tests")), [])
        self.assertEqual(len(self.failures(declared, self.rule("test-support"))), 1)
        self.assertEqual(len(self.failures(declared, "rust_library(name = \"demo_crate\")\n")), 1)
        self.assertEqual(len(self.failures(declared)), 1)

    def test_only_the_named_target_counts(self):
        other = self.rule("workspace-tests").replace("demo_crate_unit_test", "demo_crate_other_test")
        self.assertEqual(len(self.failures({"workspace-tests": []}, other + self.rule("test-support"))), 1)

    def test_every_declared_test_feature_is_on(self):
        manifest = crate_manifest({"features": ["test-support", "live"]}, {"test-support": [], "live": []})
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            (root / "BUILD.bazel").write_text(self.rule("live", "test-support"))
            self.assertEqual(pcs.bazel_workspace_tests_failures("demo-crate", manifest, root), [])
            (root / "BUILD.bazel").write_text(self.rule("test-support"))
            self.assertEqual(len(pcs.bazel_workspace_tests_failures("demo-crate", manifest, root)), 1)


def canary_source(predicate, message, used=True):
    return (
        "//! Demo.\n\n"
        "// Test-feature canary; .cargo/config.toml [env] sets MEERKAT_WORKSPACE_TEST_FEATURES.\n"
        f"#[cfg(all(test, not({predicate})))]\n"
        "mod test_features_canary {\n"
        "    fn require<T>() {\n"
        "        const {\n"
        "            let _ = size_of::<T>();\n"
        "            assert!(\n"
        '                option_env!("MEERKAT_WORKSPACE_TEST_FEATURES").is_none(),\n'
        f'                "{message}"\n'
        "            );\n"
        "        }\n"
        "    }\n"
        + ("\n    #[used]\n    static REQUIRE: fn() = require::<()>;\n" if used else "")
        + "}\n\npub fn f() {}\n"
    )


def const_canary_source(predicate, message):
    """The earlier shape: a `const _` assert, which `cargo check` and editors
    evaluate too."""
    return (
        "//! Demo.\n\n"
        f"#[cfg(all(test, not({predicate})))]\n"
        "const _: () = assert!(\n"
        '    option_env!("MEERKAT_WORKSPACE_TEST_FEATURES").is_none(),\n'
        f'    "{message}"\n'
        ");\n\npub fn f() {}\n"
    )


class TestFeatureCanary(unittest.TestCase):
    def failures(self, manifest, source):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            (root / "src").mkdir()
            (root / "src" / "lib.rs").write_text(source)
            return pcs.canary_failures("demo", manifest, root)

    def test_the_test_features_are_the_declared_ones_then_workspace_tests(self):
        manifest = crate_manifest({"features": ["a", "b"]}, {"a": [], "b": [], "workspace-tests": []})
        self.assertEqual(pcs.test_features(manifest), ["a", "b", "workspace-tests"])
        self.assertEqual(pcs.test_features(crate_manifest(None, {"a": []})), [])

    def test_one_canary_over_exactly_the_test_features(self):
        single = crate_manifest(None, {"workspace-tests": []})
        message = "demo: run `cargo test -p demo --features workspace-tests`"
        self.assertEqual(self.failures(single, canary_source('feature = "workspace-tests"', message)), [])
        several = crate_manifest({"features": ["a", "b"]}, {"a": [], "b": []})
        source = canary_source('all(feature = "a", feature = "b")', "demo: run `cargo test -p demo --features a,b`")
        self.assertEqual(self.failures(several, source), [])

    def test_a_canary_split_over_lines_counts(self):
        several = crate_manifest({"features": ["a", "b"]}, {"a": [], "b": []})
        message = "demo: needs its test features: \\\n                 `cargo test -p demo --features a,b` \\\n                 (more)"
        source = canary_source('all(feature = "a", feature = "b")', message)
        split = source.replace("#[cfg(all(test, not(all(", "#[cfg(all(\n    test,\n    not(all(\n        ")
        self.assertEqual(self.failures(several, split), [])
        broken = canary_source('all(feature = "a", feature = "b")', "demo: run `cargo test -p demo \\\n    --features a,b`")
        self.assertEqual(self.failures(several, broken), [])

    def test_a_missing_wrong_or_doubled_canary_fails(self):
        single = crate_manifest(None, {"workspace-tests": []})
        message = "demo: run `cargo test -p demo --features workspace-tests`"
        self.assertEqual(len(self.failures(single, "pub fn f() {}\n")), 1)
        self.assertEqual(len(self.failures(single, canary_source('feature = "other"', message))), 1)
        self.assertEqual(len(self.failures(single, canary_source('feature = "workspace-tests"', "demo: needs features"))), 1)
        doubled = canary_source('feature = "workspace-tests"', message) * 2
        self.assertEqual(len(self.failures(single, doubled)), 1)
        partial = crate_manifest({"features": ["a", "b"]}, {"a": [], "b": []})
        self.assertEqual(len(self.failures(partial, canary_source('feature = "a"', "demo: run `cargo test -p demo --features a,b`"))), 1)

    def test_the_message_names_the_whole_command(self):
        single = crate_manifest(None, {"workspace-tests": []})
        for message in (
            "demo: needs `--features workspace-tests`",
            "demo: run `cargo test -p other --features workspace-tests`",
            "demo: run cargo test -p demo --features workspace-tests",
        ):
            self.assertEqual(len(self.failures(single, canary_source('feature = "workspace-tests"', message))), 1, message)

    def test_only_a_canary_that_code_generation_alone_evaluates_counts(self):
        single = crate_manifest(None, {"workspace-tests": []})
        message = "demo: run `cargo test -p demo --features workspace-tests`"
        # A `const _` assert trips `cargo check`, clippy and editors too.
        self.assertEqual(len(self.failures(single, const_canary_source('feature = "workspace-tests"', message))), 1)
        # Without the static nothing instantiates the function: no build trips it.
        self.assertEqual(len(self.failures(single, canary_source('feature = "workspace-tests"', message, used=False))), 1)

    def test_a_crate_without_test_features_carries_none(self):
        plain = crate_manifest(None, {"a": []})
        self.assertEqual(self.failures(plain, "pub fn f() {}\n"), [])
        stray = canary_source('feature = "a"', "demo: run `cargo test -p demo --features a`")
        self.assertEqual(len(self.failures(plain, stray)), 1)


class CanaryFixture(unittest.TestCase):
    def test_the_selftest_fixture_carries_the_canary_shape(self):
        """scripts/test-feature-canary-selftest.sh builds this fixture to prove
        how the canary behaves on the pinned toolchain: it must have the shape
        the guard requires of the Core crates."""
        fixture = pcs.ROOT / "scripts" / "tests" / "fixtures" / "test-feature-canary"
        manifest = pcs.tomllib.loads((fixture / "Cargo.toml.txt").read_text())
        self.assertEqual(pcs.test_features(manifest), ["canary-test-feature"])
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            (root / "src").mkdir()
            (root / "src" / "lib.rs").write_text((fixture / "lib.rs.txt").read_text())
            self.assertEqual(pcs.canary_failures("test-feature-canary-fixture", manifest, root), [])


class SelfDevDependency(unittest.TestCase):
    def test_a_crate_never_dev_depends_on_itself(self):
        self.assertEqual(pcs.self_dev_dependency_failures("demo", crate_manifest(None, {}, {"other": "1"})), [])
        path_only = crate_manifest(None, {}, {"demo": {"path": ".", "features": ["workspace-tests"]}})
        self.assertEqual(len(pcs.self_dev_dependency_failures("demo", path_only)), 1)
        renamed = crate_manifest(None, {}, {"me": {"path": ".", "package": "demo"}})
        self.assertEqual(len(pcs.self_dev_dependency_failures("demo", renamed)), 1)
        targeted = crate_manifest()
        targeted["target"] = {"cfg(unix)": {"dev-dependencies": {"demo": {"path": "."}}}}
        self.assertEqual(len(pcs.self_dev_dependency_failures("demo", targeted)), 1)


if __name__ == "__main__":
    unittest.main()
