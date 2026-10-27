"""Finite transfer-reader controls, with no cloud or service operations.

FORGE_REMOTE_TAR_SOURCE_ROOT selects the exact predecessor or candidate tree.
When copied to installer/tests, the default selects the maintained installer.
Transfer byte expressions containing 1024 are reduced by the same factor
256 in isolated compiled ASTs. The fixed 16 KiB owner JSON limit is retained.
The actual 64/65/80 MiB source/compressed/TAR
relationship becomes 256/260/320 KiB; the existing file and manifest limits
are reduced identically. A 384 KiB global PAX comment exercises expansion.
Only the extracted Python bootstrap runs, with a hash-pinned inert helper.
No shell script, cloud client, process supervisor or application is started.
"""
import ast
import base64
import gzip
import hashlib
import io
import json
import os
from pathlib import Path
import sys
import tarfile
import types

import pytest


SCALE = 256
GUIDE_ID = "11111111-1111-4111-8111-111111111111"
BUILD_ID = "22222222-2222-4222-8222-222222222222"
OWNER_ID = "3" * 32
ACCEPTED_DIGEST = "a" * 64
LONG_PATH = "assets/" + "long-path-" * 16 + "/accepted.txt"
INERT_HELPER = b'''import json
from pathlib import Path
home = Path.home()
with (home / "qualified-helper-executed.txt").open("a") as output:
    output.write("exact helper imported\\n")
def receive_archive(archive, root, identity, digest):
    with (home / "qualified-helper-received.jsonl").open("a") as output:
        output.write(json.dumps({"archive": str(archive), "root": str(root),
            "identity": identity, "digest": digest}) + "\\n")
'''


def source_root():
    return Path(os.environ.get("FORGE_REMOTE_TAR_SOURCE_ROOT",
        str(Path(__file__).resolve().parents[1]))).resolve()


def maintained_installer():
    for parent in Path(__file__).resolve().parents:
        if (parent / "launchkit/deployment_deadlines.py").is_file():
            return parent
    raise AssertionError("The existing deployment deadline renderer is required.")


def numeric_product(node):
    if isinstance(node, ast.Constant) and type(node.value) is int:
        return node.value, node.value == 1024
    if isinstance(node, ast.BinOp) and isinstance(node.op, ast.Mult):
        left, right = numeric_product(node.left), numeric_product(node.right)
        if left is not None and right is not None:
            return left[0] * right[0], left[1] or right[1]
    return None


class FiniteBytes(ast.NodeTransformer):
    def visit_BinOp(self, node):
        product = numeric_product(node)
        if (product is not None and product[1] and product[0] >= SCALE
                and product[0] != 16 * 1024):
            assert product[0] % SCALE == 0
            return ast.copy_location(ast.Constant(product[0] // SCALE), node)
        return self.generic_visit(node)


def compile_finite(text, filename):
    tree = FiniteBytes().visit(ast.parse(text, filename=filename))
    return compile(ast.fix_missing_locations(tree), filename, "exec")


def load_module(name):
    path = source_root() / "launchkit" / name
    content = path.read_text()
    if name == "templates_v2/remote_operations.py":
        renderer_path = maintained_installer() / "launchkit/deployment_deadlines.py"
        renderer = {"__name__": "finite_deadline_renderer", "__file__": str(renderer_path)}
        exec(compile(renderer_path.read_text(), str(renderer_path), "exec"), renderer)
        content = renderer["render_destination_shutdown_budget"](content)
    module = types.ModuleType("finite_" + path.stem)
    module.__file__ = str(path)
    exec(compile_finite(content, str(path)), module.__dict__)
    return module


def bootstrap_code():
    path = source_root() / "launchkit/templates/deploy-gcp.sh"
    marker = 'REMOTE_INSTALL="$(python3 - "$INSTANCE" "$DEPLOYMENT_ID" "$CONFIGURATION" <<\'PY\'\n'
    text = path.read_text().split(marker, 1)[1].split("\nPY\n", 1)[0]
    tree = ast.parse(text, filename=str(path))
    assignments = [node for node in tree.body if isinstance(node, ast.Assign)
        and any(isinstance(target, ast.Name) and target.id == "code" for target in node.targets)]
    assert len(assignments) == 1
    return ast.literal_eval(assignments[0].value), str(path) + ":remote-bootstrap"


def public_files(app=b"# inert accepted application\n"):
    return {
        "deployment.json": json.dumps({"schemaVersion": 2, "target": "gcp", "slug": "finite-fixture"}).encode(),
        "implementation-report.json": json.dumps({"guide_version": 1, "build_id": BUILD_ID,
            "verification": {"digest": ACCEPTED_DIGEST}}).encode(),
        "app.py": app, "install.sh": b"#!/bin/sh\n# never executed\n",
        "install_gateway.py": b"# never executed\n", "requirements.txt": b"# no dependencies installed\n",
        "application_configuration.py": b"# never executed\n", "remote_operations.py": INERT_HELPER,
        LONG_PATH: b"accepted ordinary PAX path\n",
    }


def transfer(files, *, comment="small legitimate global PAX comment"):
    manifest = "".join(hashlib.sha256(content).hexdigest() + "  " + name + "\n"
        for name, content in sorted(files.items())).encode()
    raw = io.BytesIO()
    with tarfile.open(fileobj=raw, mode="w:", format=tarfile.PAX_FORMAT,
            pax_headers={"comment": comment}) as stream:
        for name, content in sorted({**files, "SHA256SUMS": manifest}.items()):
            entry = tarfile.TarInfo(name)
            entry.size, entry.mode, entry.mtime = len(content), 0o755 if name == "install.sh" else 0o644, 0
            stream.addfile(entry, io.BytesIO(content))
    digest = hashlib.sha256()
    for name, content in sorted(files.items()):
        digest.update(name.encode() + b"\0" + hashlib.sha256(content).digest())
    return gzip.compress(raw.getvalue(), mtime=0), digest.hexdigest(), raw.getvalue()


def binding(digest):
    return {"guide_id": GUIDE_ID, "guide_version": 1, "build_id": BUILD_ID,
        "accepted_source_digest": ACCEPTED_DIGEST, "package_sha256": "b" * 64,
        "controller_sha256": "c" * 64, "transferred_source_digest": digest}


def parser_spy(monkeypatch):
    original, calls = tarfile.open, []
    def open_actual(*args, **kwargs):
        calls.append((args, kwargs))
        return original(*args, **kwargs)
    monkeypatch.setattr(tarfile, "open", open_actual)
    return calls


def bootstrap_run(monkeypatch, home, expected, digest):
    monkeypatch.setenv("HOME", str(home))
    monkeypatch.setattr(sys, "argv", ["-c", "finite-fixture", OWNER_ID, digest, expected])
    code, filename = bootstrap_code()
    exec(compile_finite(code, filename), {"__name__": "finite_bootstrap", "__file__": filename})


def refuse_transfer(surface, archive, digest, tmp_path, monkeypatch):
    home = tmp_path.resolve() / "inert-home"
    home.mkdir(mode=0o700)
    path, destination = home / "mobkit-package.tar.gz", home / "src/finite-fixture"
    path.write_bytes(archive)
    path.chmod(0o600)
    if surface == "controller":
        module = load_module("remote_destination.py")
        invoke = lambda: module._archive(base64.b64encode(archive).decode(), binding(digest))
        refusal = module.ControllerError
    elif surface == "receiver":
        module = load_module("templates_v2/remote_operations.py")
        invoke = lambda: module.receive_archive(path, destination, OWNER_ID, digest)
        refusal = module.DestinationError
    else:
        invoke = lambda: bootstrap_run(monkeypatch, home, hashlib.sha256(INERT_HELPER).hexdigest(), digest)
        refusal = SystemExit
    calls = parser_spy(monkeypatch)
    refused = False
    try:
        invoke()
    except refusal:
        refused = True
    assert refused, "The unqualified archive was accepted."
    assert calls == [], "Unqualified expanded metadata reached the real TAR parser."
    assert not destination.exists(), "Archive refusal must precede destination publication."
    assert not (home / "qualified-helper-executed.txt").exists(), "Archive refusal must precede helper execution."
    assert not (home / "qualified-helper-received.jsonl").exists()
    assert path.read_bytes() == archive


@pytest.mark.parametrize("surface", ["controller", "receiver", "bootstrap"])
def test_compressible_global_pax_refused_before_parser_or_effects(surface, tmp_path, monkeypatch):
    files = public_files()
    archive, digest, expanded = transfer(files, comment="x" * (96 * 1024 * 1024 // SCALE))
    # Ordinary source and compressed transport remain legal. Only hidden
    # expanded global metadata exceeds the real, proportionally reduced cap.
    assert sum(map(len, files.values())) < 64 * 1024 * 1024 // SCALE
    assert len(archive) < (65 * 1024 * 1024) // SCALE
    assert len(expanded) > (80 * 1024 * 1024) // SCALE
    refuse_transfer(surface, archive, digest, tmp_path, monkeypatch)


@pytest.mark.parametrize("surface", ["controller", "receiver", "bootstrap"])
@pytest.mark.parametrize("damage", ["crc", "truncated-trailer"])
def test_corrupt_or_truncated_gzip_refused_before_parser_or_effects(surface, damage, tmp_path, monkeypatch):
    archive, digest, _ = transfer(public_files())
    if damage == "crc":
        corrupt = bytearray(archive)
        corrupt[-8] ^= 1
        archive = bytes(corrupt)
    else:
        archive = archive[:-8]
    refuse_transfer(surface, archive, digest, tmp_path, monkeypatch)


def test_valid_controller_global_pax_and_extended_path_keep_exact_transfer(monkeypatch):
    module = load_module("remote_destination.py")
    files = public_files()
    archive, digest, _ = transfer(files)
    calls = parser_spy(monkeypatch)
    first = module._archive(base64.b64encode(archive).decode(), binding(digest))
    second = module._archive(base64.b64encode(archive).decode(), binding(digest))
    assert first == second
    assert first[0] == archive and first[1] == files
    assert first[2] == json.loads(files["deployment.json"])
    assert len(calls) == 2, "The actual TAR parser must handle legitimate PAX records."


@pytest.fixture
def receiver_umask(request):
    previous = os.umask(request.param)
    try:
        yield request.param
    finally:
        os.umask(previous)


@pytest.mark.parametrize("receiver_umask", [0o022, 0o077], indirect=True)
def test_valid_receiver_global_pax_exact_retry_preserves_private_state(tmp_path, receiver_umask):
    module = load_module("templates_v2/remote_operations.py")
    files = public_files()
    archive, digest, _ = transfer(files)
    path, destination = tmp_path.resolve() / "transfer.tar.gz", tmp_path.resolve() / "destination"
    path.write_bytes(archive)
    selected = module.receive_archive(path, destination, OWNER_ID, digest)
    for name, content in files.items():
        assert (destination / name).read_bytes() == content
    assert (destination / "install.sh").stat().st_mode & 0o777 == 0o755
    assert (destination / LONG_PATH).stat().st_mode & 0o777 == 0o644
    (destination / ".env").write_bytes(b"inert private settings survive\n")
    (destination / "data").mkdir()
    (destination / "data/history").write_bytes(b"inert retained history\n")
    before = (destination / "app.py").stat()
    (destination / LONG_PATH).unlink()
    assert module.receive_archive(path, destination, OWNER_ID, digest) == selected
    after = (destination / "app.py").stat()
    assert (after.st_ino, after.st_mtime_ns) == (before.st_ino, before.st_mtime_ns)
    assert (destination / LONG_PATH).read_bytes() == files[LONG_PATH]
    assert (destination / ".env").read_bytes() == b"inert private settings survive\n"
    assert (destination / "data/history").read_bytes() == b"inert retained history\n"
    assert module.source_digest(destination) == digest


def test_receiver_parses_the_same_compressed_snapshot_it_admitted(tmp_path, monkeypatch):
    module = load_module("templates_v2/remote_operations.py")
    files = public_files()
    original, digest, _ = transfer(files)
    replacement, _, _ = transfer(public_files(app=b"# different later source\n"))
    path, destination = tmp_path.resolve() / "transfer.tar.gz", tmp_path.resolve() / "destination"
    path.write_bytes(original)
    ordinary, observations = module.ordinary, []
    def observe_then_replace(selected, **kwargs):
        content = ordinary(selected, **kwargs)
        if Path(selected) == path:
            observations.append(content)
            path.write_bytes(replacement)
        return content
    monkeypatch.setattr(module, "ordinary", observe_then_replace)
    module.receive_archive(path, destination, OWNER_ID, digest)
    assert observations == [original]
    assert path.read_bytes() == replacement
    for name, content in files.items():
        assert (destination / name).read_bytes() == content
    assert module.source_digest(destination) == digest


def test_bootstrap_executes_only_the_exact_qualified_inert_helper(tmp_path, monkeypatch):
    home = tmp_path.resolve() / "inert-home"
    home.mkdir(mode=0o700)
    archive, digest, _ = transfer(public_files())
    (home / "mobkit-package.tar.gz").write_bytes(archive)
    calls = parser_spy(monkeypatch)
    expected = hashlib.sha256(INERT_HELPER).hexdigest()
    bootstrap_run(monkeypatch, home, expected, digest)
    bootstrap_run(monkeypatch, home, expected, digest)
    assert len(calls) == 2
    assert (home / "qualified-helper-executed.txt").read_text() == "exact helper imported\n" * 2
    rows = [json.loads(row) for row in (home / "qualified-helper-received.jsonl").read_text().splitlines()]
    assert rows == [{"archive": str(home / "mobkit-package.tar.gz"), "root": str(home / "src/finite-fixture"),
        "identity": OWNER_ID, "digest": digest}] * 2
    assert not (home / "src").exists(), "The helper fixture does not install or run an application."


def test_bootstrap_never_executes_an_unqualified_helper(tmp_path, monkeypatch):
    home = tmp_path.resolve() / "inert-home"
    home.mkdir(mode=0o700)
    archive, digest, _ = transfer(public_files())
    (home / "mobkit-package.tar.gz").write_bytes(archive)
    with pytest.raises(SystemExit, match="helper differs"):
        bootstrap_run(monkeypatch, home, "f" * 64, digest)
    assert not (home / "qualified-helper-executed.txt").exists()
    assert not (home / "qualified-helper-received.jsonl").exists()
    assert not (home / "src").exists()
