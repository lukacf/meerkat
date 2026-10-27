"""Trusted stopped product Install adapter for the original remote controller.

The origin explicitly selects the complete control dependency graph for its
observed CPython/Linux ABI. This module adds no native setup/start authority,
public path selector, dependency resolver or deployment state machine. The
original remote operation owns detached process custody; the actual canonical
DeploymentService/factory own application installation and cleanup.
"""
from __future__ import annotations

from contextlib import contextmanager
from copy import deepcopy
import hashlib
import inspect
import io
import json
import os
from pathlib import Path
import platform
import re
import stat
import struct
import sys
import tempfile
import zipfile

FORMAT = "forge-product-control-source-v1"
MAX_BUNDLE = 128 * 1024 * 1024
MAX_SOURCE = 16 * 1024 * 1024
MAX_FILES = 512


def _require(value, message):
    if not value:
        raise ValueError(message)


def _json(value):
    return (json.dumps(value, ensure_ascii=True, sort_keys=True, allow_nan=False,
        separators=(",", ":")) + "\n").encode("ascii")


def _decode(content):
    def unique(pairs):
        result = {}
        for key, value in pairs:
            _require(key not in result, "Duplicate control input field.")
            result[key] = value
        return result
    return json.loads(content, object_pairs_hook=unique)


def _read(path, maximum=MAX_BUNDLE):
    path = Path(path)
    _require(not any(item.is_symlink() for item in (path, *path.parents)),
        "The original control input contains a link.")
    before = path.lstat()
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(fd, "rb") as stream:
        info = os.fstat(stream.fileno())
        def identity(value):
            return (value.st_dev, value.st_ino, value.st_mode, value.st_uid,
                value.st_nlink, value.st_size, value.st_mtime_ns, value.st_ctime_ns)
        _require(stat.S_ISREG(info.st_mode) and info.st_uid == os.getuid()
            and info.st_nlink == 1 and 0 <= info.st_size <= maximum
            and identity(before) == identity(info), "The control input is not an owned bounded file.")
        content = stream.read(maximum + 1)
        _require(len(content) == info.st_size and identity(info)
            == identity(os.fstat(stream.fileno())) == identity(path.lstat()),
            "The original control input changed during reading.")
    return content


class ProductControlSelection:
    """Immutable private launcher selection, never reconstructed from HTTP data.

    The supplying owner accepts provenance and the complete control graph.
    Pin checking and wheel tags do not themselves establish that acceptance.
    The declared graph is checked and imported before an application Install;
    an unavailable or partial graph leaves that original operation uncertain.
    """

    def __init__(self, *, runtime_target, requirements, wheels, python_tag, abi_tag,
            glibc_minimum, require_current):
        from .runtime_candidates import ArtifactFile, MetaToolkitRuntimeTarget
        _require(type(runtime_target) is MetaToolkitRuntimeTarget
            and runtime_target.target == "linux-x86_64" and type(requirements) is ArtifactFile
            and type(wheels) is tuple and wheels and all(type(item) is ArtifactFile for item in wheels)
            and callable(require_current), "Select actual product and control dependency owners.")
        _require(type(python_tag) is str and re.fullmatch(r"cp3[0-9]{2}", python_tag)
            and int(python_tag[3:]) >= 11 and abi_tag == python_tag
            and type(glibc_minimum) is tuple and len(glibc_minimum) == 2
            and all(type(item) is int and item >= 0 for item in glibc_minimum)
            and glibc_minimum >= (2, 31), "Select the actual supported Linux CPython ABI.")
        self._selection = (runtime_target, requirements, wheels, python_tag, abi_tag,
            glibc_minimum, require_current)
        self._original = self._selection

    @property
    def runtime_target(self):
        return self._selection[0]

    def require_current(self):
        _require(self._selection == self._original, "The original control selection changed.")
        result = self._selection[-1]()
        if inspect.iscoroutine(result):
            result.close()
        _require(result is not False and not inspect.isawaitable(result)
            and not inspect.isasyncgen(result), "The original control selection owner changed.")
    def read_dependencies(self):
        """Joined file lane; the caller fences its actual owner on the calling task."""
        _require(self._selection == self._original, "The original control selection changed.")
        from .templates.install_candidate import _read_selected_dependency_wheels
        from .runtime_selection import selected_runtime_artifacts
        # The guest checks its glibc against this control's minimum only, so
        # a minimum below the product's declared floor is refused here.
        floor, minimum = selected_runtime_artifacts(self.runtime_target).glibc_floor, self._selection[5]
        if floor is not None and minimum < floor:
            from .projects import DraftConflict
            needed = f"glibc {floor[0]}.{floor[1]}"
            raise DraftConflict(f"This product's native programs need {needed} or newer, but the selected Linux "
                f"control admits computers with glibc {minimum[0]}.{minimum[1]}. Raise the control selection's glibc "
                f"minimum to {floor[0]}.{floor[1]} and install on a Linux computer with {needed} or newer.")
        pending = _read_selected_dependency_wheels(self._selection[1], self._selection[2])
        names = {row[0] for row in pending}
        _require({"pydantic", "pydantic-core", "httpx"} <= names,
            "The owner-selected control dependency batch lacks its actual host imports.")
        interpreter = {"python_tag": self._selection[3], "abi_tag": self._selection[4],
            "glibc_minimum": list(self._selection[5]), "machine": "x86_64", "implementation": "cpython"}
        for _, filename, _, _ in pending:
            _require(_wheel_compatible(filename, interpreter),
                "The selected control wheel differs from its actual guest interpreter/platform.")
        return pending, interpreter


def _wheel_compatible(filename, interpreter):
    parts = filename.removesuffix(".whl").split("-")
    if not filename.endswith(".whl") or len(parts) not in {5, 6}:
        return False
    python_tags, abi_tags, platforms = (part.split(".") for part in parts[-3:])
    selected = interpreter["python_tag"]
    minor = int(selected[3:])
    # As pip does, a pure py3N tag fits CPython 3.M for every N <= M.
    python_ok = any(tag in {selected, "py3"}
        or re.fullmatch(r"py3[0-9]{1,2}", tag) and int(tag[3:]) <= minor
        or re.fullmatch(r"cp3[0-9]{1,2}", tag) and int(tag[3:]) <= minor
            and "abi3" in abi_tags for tag in python_tags)
    abi_ok = any(tag in {interpreter["abi_tag"], "none", "abi3"} for tag in abi_tags)
    def platform_ok(tag):
        if tag in {"any", "linux_x86_64"}:
            return True
        versions = {"manylinux1_x86_64": (2, 5), "manylinux2010_x86_64": (2, 12),
            "manylinux2014_x86_64": (2, 17)}
        found = re.fullmatch(r"manylinux_([0-9]+)_([0-9]+)_x86_64", tag)
        version = tuple(map(int, found.groups())) if found else versions.get(tag)
        return version is not None and version <= tuple(interpreter["glibc_minimum"])
    return python_ok and abi_ok and any(platform_ok(tag) for tag in platforms)


class ProductControlBundle:
    def __init__(self, selection, content, manifest, source_inputs):
        self.selection, self.content, self.manifest = selection, content, deepcopy(manifest)
        self._selection = selection
        self._observed = (content, _json(manifest), tuple(source_inputs))

    def require_current(self):
        _require(type(self.selection) is ProductControlSelection and self.selection is self._selection and self.content == self._observed[0]
            and _json(self.manifest) == self._observed[1], "The original frozen control bundle changed.")
        self.selection.require_current()
        self.require_files_current()

    def require_files_current(self):
        for path, expected, observed in self._observed[2]:
            info = path.lstat()
            current = (info.st_dev, info.st_ino, info.st_mode, info.st_uid, info.st_nlink,
                info.st_size, info.st_mtime_ns, info.st_ctime_ns)
            _require(not path.is_symlink() and current == observed,
                "The original trusted control source changed after freezing.")


def freeze_product_control(selection):
    from .deployment_service import supervisor_source_files
    from .remote_interface_bundle import product_destination_owner_bytes
    from .runtime_selection import candidate_selection
    _require(type(selection) is ProductControlSelection, "Select the original private control owner.")
    pending, interpreter = selection.read_dependencies()
    source = Path(__file__).parent
    host = supervisor_source_files()
    files = {name: content for name, content in host.items() if name.startswith("launchkit/")}
    _require("launchkit/remote_product_control.py" in files,
        "The canonical product control adapter is absent from its trusted source closure.")
    originals = []
    for name, content in sorted(files.items()):
        path = source / name.removeprefix("launchkit/")
        _require(_read(path, MAX_SOURCE) == content, "The selected trusted control source changed during freezing.")
        info = path.lstat()
        observed = (info.st_dev, info.st_ino, info.st_mode, info.st_uid, info.st_nlink,
            info.st_size, info.st_mtime_ns, info.st_ctime_ns)
        originals.append((path, hashlib.sha256(content).hexdigest(), observed))
    requirements_pin = selection._selection[1]
    requirements = _read(requirements_pin.path)
    _require(len(requirements) == requirements_pin.size
        and hashlib.sha256(requirements).hexdigest() == requirements_pin.sha256,
        "The original selected control requirements changed during freezing.")
    files["dependencies/requirements.txt"] = requirements
    wheels = []
    for _, filename, content, _ in pending:
        name = "dependencies/" + filename
        _require(name not in files, "The original control dependency filename is duplicated.")
        files[name] = content
        wheels.append(name)
    _require(len(files) <= MAX_FILES and sum(map(len, files.values())) <= MAX_BUNDLE,
        "The original control closure exceeds its bound.")
    stream = io.BytesIO()
    with zipfile.ZipFile(stream, "w", compression=zipfile.ZIP_STORED) as archive:
        for name, content in sorted(files.items()):
            info = zipfile.ZipInfo(name, (1980, 1, 1, 0, 0, 0))
            info.external_attr = (stat.S_IFREG | 0o600) << 16
            archive.writestr(info, content)
    content = stream.getvalue()
    _require(len(content) <= MAX_BUNDLE, "The frozen control bundle exceeds its bound.")
    manifest = {"format": FORMAT, "interpreter": interpreter,
        "runtime_selection": candidate_selection(selection.runtime_target, deployment_target="linux"),
        "source_owner_sha256": hashlib.sha256(product_destination_owner_bytes()).hexdigest(),
        "bundle": {"size": len(content), "sha256": hashlib.sha256(content).hexdigest()},
        "files": {name: {"size": len(data), "sha256": hashlib.sha256(data).hexdigest()}
            for name, data in sorted(files.items())},
        "dependencies": {"requirements": "dependencies/requirements.txt", "wheels": wheels}}
    result = ProductControlBundle(selection, content, manifest, originals)
    result.require_files_current()
    return result


def guest_interpreter(manifest):
    value = manifest["interpreter"]
    actual = "cp" + str(sys.version_info.major) + str(sys.version_info.minor)
    libc, version = platform.libc_ver()
    try:
        glibc = tuple(int(part) for part in version.split(".")[:2])
    except ValueError:
        glibc = ()
    _require(manifest.get("format") == FORMAT and sys.implementation.name == value["implementation"]
        and platform.system() == "Linux" and platform.machine() == value["machine"]
        and actual == value["python_tag"] == value["abi_tag"]
        and libc == "glibc" and glibc >= tuple(value["glibc_minimum"]),
        "The original selected control interpreter/platform differs from this guest.")
    return {"python_tag": actual, "abi_tag": actual, "machine": platform.machine(),
        "implementation": sys.implementation.name, "glibc": list(glibc)}


def read_request(stream, maximum):
    def exact(size):
        chunks = []
        pending = size
        while pending:
            value = stream.read(pending)
            _require(value, "The original control stream header is incomplete.")
            chunks.append(value)
            pending -= len(value)
        return b"".join(chunks)
    first = stream.read(1)
    if first == b"{":
        body = first + stream.read(maximum)
        _require(len(body) <= maximum, "The original control request exceeds its bound.")
        return _decode(body), None
    header = first + exact(3)
    _require(len(header) == 4, "The original control stream header is incomplete.")
    size = struct.unpack("!I", header)[0]
    _require(0 < size <= maximum, "The original control header exceeds its bound.")
    content = exact(size)
    return _decode(content), stream


def source_context(ops, root, owner, manifest):
    path = Path(owner["host"]["home"]) / ".local/state/meta-forge/product-sources" / owner["deployment_id"]
    context = _decode(_read(path / "owner.json", 1024 * 1024))
    _require(context.get("deployment_id") == owner["deployment_id"]
        and context.get("guide_id") == owner["binding"]["guide_id"]
        and context.get("build_id") == owner["binding"]["build_id"]
        and context.get("source_digest") == owner["binding"]["accepted_source_digest"]
        and context.get("package_sha256") == owner["binding"]["package_sha256"]
        and context.get("host") == owner["host"]
        and context.get("owner_sha256") == manifest["source_owner_sha256"],
        "The control operation differs from its actual staged source intake.")
    source = _read(path / "source-owner.py", 4 * 1024 * 1024)
    _require(hashlib.sha256(source).hexdigest() == manifest["source_owner_sha256"],
        "The original source intake owner changed.")
    staged = _decode(_read(path / "staged.json", 1024 * 1024))
    _require(staged.get("status") == "product-source-staged"
        and staged.get("deployment_id") == owner["deployment_id"],
        "The original source intake is not confirmed staged.")
    selected = _decode(_read(root / ".mobkit/forge-product-source/owner.json", 1024 * 1024))
    _require(selected.get("context") == context and selected.get("origin") == "runtime-candidate.json"
        and _decode(_read(root / "runtime-candidate.json", 1024 * 1024)) == manifest["runtime_selection"],
        "The original staged product selection differs from this trusted control closure.")
    return context, source


def verify_package_source(root, context):
    intake = Path(context["host"]["home"]) / ".local/state/meta-forge/product-sources" / context["deployment_id"]
    data = _read(intake / "package.zip", 64 * 1024 * 1024)
    _require(len(data) == context["package_size"] and hashlib.sha256(data).hexdigest() == context["package_sha256"],
        "The original accepted product package changed.")
    with zipfile.ZipFile(io.BytesIO(data)) as archive:
        _require(len(archive.infolist()) <= 2048 and len(archive.namelist()) == len(set(archive.namelist())),
            "The original application inventory changed.")
        total = 0
        for info in archive.infolist():
            total += info.file_size
            _require(total <= 64 * 1024 * 1024 and not info.is_dir()
                and not info.filename.startswith("/") and "\\" not in info.filename
                and not any(part in {"", ".", ".."} for part in info.filename.split("/")),
                "The original application source inventory is unsafe.")
            _require(_read(root / info.filename, 64 * 1024 * 1024) == archive.read(info),
                "The original accepted application source changed.")


def receive_bundle(ops, root, identity, binding, host, source, manifest, request, stream):
    guest_interpreter(manifest)
    owner = {"protocol": 1, "deployment_id": identity, "binding": binding,
        "host": {key: host[key] for key in ("uid", "home", "destination")}, "product_control": True}
    context, _ = source_context(ops, root, owner, manifest)
    verify_package_source(root, context)
    folder = ops["_directory"](root)
    ops["_private_directory"](folder)
    _require(stream is not None and request.get("transfer_file") == manifest["bundle"],
        "The control stream differs from its original selected bundle.")
    target = folder / "control-bundle.zip"
    descriptor, name = tempfile.mkstemp(prefix=".control-transfer-", dir=folder)
    temporary = Path(name)
    try:
        hashed, count = hashlib.sha256(), 0
        with os.fdopen(descriptor, "wb") as output:
            os.fchmod(output.fileno(), 0o600)
            while count < manifest["bundle"]["size"]:
                chunk = stream.read(min(1024 * 1024, manifest["bundle"]["size"] - count))
                _require(chunk, "The original control stream is incomplete.")
                output.write(chunk)
                count += len(chunk)
                hashed.update(chunk)
            output.flush()
            os.fsync(output.fileno())
        _require(not stream.read(1) and hashed.hexdigest() == manifest["bundle"]["sha256"],
            "The original control stream byte binding changed.")
        _bundle_files(_read(temporary), manifest)
        if target.exists() or target.is_symlink():
            _require(_read(target) == _read(temporary), "The original control bundle changed.")
        else:
            os.link(temporary, target, follow_symlinks=False)
    finally:
        temporary.unlink(missing_ok=True)
    snapshots = {"controller.py": source, "control-selection.json": _json(manifest)}
    for name, content in snapshots.items():
        path = folder / name
        if path.exists() or path.is_symlink():
            _require(_read(path) == content, "The original control snapshot changed.")
        else:
            ops["_write"](path, content, exclusive=True)
    owner["snapshots"] = {**{name: hashlib.sha256(content).hexdigest() for name, content in snapshots.items()},
        "control-bundle.zip": manifest["bundle"]["sha256"]}
    old = ops["_owner"](root, identity, binding, host, required=False)
    if old is None:
        ops["_json_write"](folder / "owner.json", owner, exclusive=True)
    else:
        _require(old == owner, "The original controller owner changed.")
    return {"status": "not-installed", "control_source_retained": True, "ready": False,
        "admitted": False, "worker_live": False, "source_current": True,
        "service": {"status": "not-installed", "ready": False}, "service_owned": False,
        "service_observed": False, "retryable": False, "private_settings_supported": False}


def _bundle_files(content, manifest):
    _require(len(content) == manifest["bundle"]["size"]
        and hashlib.sha256(content).hexdigest() == manifest["bundle"]["sha256"],
        "The selected control bundle changed.")
    rows = manifest["files"]
    _require(type(rows) is dict and 0 < len(rows) <= MAX_FILES, "The selected control inventory exceeds its bound.")
    with zipfile.ZipFile(io.BytesIO(content)) as archive:
        names = archive.namelist()
        _require(len(names) == len(set(names)) and set(names) == set(rows),
            "The complete control source inventory changed.")
        total, files = 0, {}
        for info in archive.infolist():
            name, row = info.filename, rows[info.filename]
            parts = name.split("/")
            _require(not info.is_dir() and not name.startswith("/") and "\\" not in name
                and not any(part in {"", ".", ".."} for part in parts)
                and parts[0] in {"launchkit", "dependencies"}
                and stat.S_IFMT(info.external_attr >> 16) == stat.S_IFREG
                and info.file_size == row["size"] and 0 <= info.file_size <= MAX_BUNDLE,
                "The selected control entry is unsafe or changed.")
            total += info.file_size
            _require(total <= MAX_BUNDLE, "The selected control closure exceeds its bound.")
            data = archive.read(info)
            _require(hashlib.sha256(data).hexdigest() == row["sha256"],
                "The selected control file changed.")
            files[name] = data
    return files


def _source_current(ops, root, owner, manifest):
    fresh = ops["_owner"](root, owner["deployment_id"], owner["binding"], owner["host"])
    _require(fresh == owner, "The original product controller owner changed.")
    source_context(ops, root, owner, manifest)


# The original command receives only public source/binding facts in argv. Site
# startup is disabled until the exact selected dependency payload is checked.
CONTROL_CHILD_ENTRY = r'''import hashlib,json,os,stat,sys,sysconfig
from pathlib import Path
root=Path(sys.argv[1])
expected=json.loads(sys.argv[2])
operation_id,mode=sys.argv[3:5]
p=root/'.mobkit/launch-remote/controller.py'
if any(item.is_symlink() for item in (p,*p.parents)):
    raise RuntimeError('The original child source contains a link.')
fd=os.open(p,os.O_RDONLY|os.O_NOFOLLOW|os.O_NONBLOCK)
with os.fdopen(fd,'rb') as channel:
    before=os.fstat(channel.fileno()); source=channel.read(16777217); after=os.fstat(channel.fileno())
identity=lambda row:(row.st_dev,row.st_ino,row.st_mode,row.st_uid,row.st_nlink,row.st_size,row.st_mtime_ns,row.st_ctime_ns)
if (not stat.S_ISREG(before.st_mode) or before.st_uid!=os.getuid() or before.st_nlink!=1
        or before.st_mode&63 or len(source)!=before.st_size or len(source)>16777216
        or identity(before)!=identity(after) or identity(after)!=identity(p.lstat())
        or hashlib.sha256(source).hexdigest()!=expected['owner']['binding']['controller_sha256']):
    raise RuntimeError('The original child controller changed before imports.')
ops={'__name__':'forge_verified_product_child','__file__':str(p),'CONTROLLER_SOURCE':source}
exec(compile(source,str(p),'exec'),ops)
adapter,manifest=ops['_product_adapter']()
if manifest!=expected['manifest']:
    raise RuntimeError('The original child selection changed before imports.')
control=adapter['_preimport_control'](ops,root,expected['owner'],manifest,operation_id,expected['worker'])
adapter['_install_verified_control_imports'](ops,root,manifest)
venv=control/'.venv'
paths=sysconfig.get_paths(scheme='posix_venv' if 'posix_venv' in sysconfig.get_scheme_names() else 'posix_prefix',
    vars={'base':str(venv),'platbase':str(venv)})
if paths['purelib']!=paths['platlib']:
    raise RuntimeError('The original control import scheme differs.')
site=Path(paths['purelib'])
sys.prefix=sys.exec_prefix=str(venv)
if mode=='dependencies':
    from launchkit.remote_product_control import install_control_dependencies
    dependencies=manifest['dependencies']
    refs={name:{'path':str(control/name),**manifest['files'][name]}
        for name in (dependencies['requirements'],*dependencies['wheels'])}
    # Debian/Ubuntu permits canonical ensurepip inside this actual created
    # venv. The system worker must not capture it before venv creation.
    bootstrap=adapter['_stdlib_pip_bootstrap']()
    install_control_dependencies(control,refs,site_packages=site,pip_bootstrap=bootstrap)
elif mode=='owner':
    adapter['_verify_installed_control_dependencies'](control,manifest,site)
    sys.path.insert(0,str(site))
    import asyncio
    from launchkit.remote_product_control import run_initial_install
    asyncio.run(run_initial_install(root,operation_id,int(sys.argv[5]),retain_control=True))
else:
    raise RuntimeError('The original child mode is unavailable.')
'''


def _control_materialized_files(root, manifest):
    folder = root / ".mobkit/launch-remote"
    control = folder / "control-host"
    files = _bundle_files(_read(folder / "control-bundle.zip"), manifest)
    for name, expected in files.items():
        _require(_read(control / name) == expected,
            "The original trusted control input changed before imports.")
    return control


def _preimport_control(ops, root, owner, manifest, operation_id, worker):
    """Reuse original frozen controller checks before importing its host closure."""
    import time
    _source_current(ops, root, owner, manifest)
    control = _control_materialized_files(root, manifest)
    guest_interpreter(manifest)
    context, _ = source_context(ops, root, owner, manifest)
    verify_package_source(root, context)
    # Popen returns before the original command birth can be published. Wait
    # only for that same original worker/UUID, never for a replacement owner.
    deadline = time.monotonic() + 5
    while True:
        job = ops["_job"](root)
        _require(job and job.get("operation_id") == operation_id and job.get("action") == "install"
            and job.get("worker") == worker and ops["_live"](worker),
            "The original child operation owner changed before imports.")
        command = job.get("command")
        if type(command) is dict and command.get("pid") == os.getpid() and ops["_live"](command):
            break
        _require(time.monotonic() < deadline, "The original child command birth was not published.")
        time.sleep(0.01)
    _source_current(ops, root, owner, manifest)
    _control_materialized_files(root, manifest)
    return control


def _install_verified_control_imports(ops, root, manifest):
    # Reuse the original accepted-source loader, with only this verified finite
    # control source closure. It cannot use a changed source or cached bytecode.
    import ast
    import importlib.abc
    import importlib.util
    folder = root / ".mobkit/launch-remote"
    files = {name: data for name, data in _bundle_files(_read(folder / "control-bundle.zip"), manifest).items()
        if name.startswith("launchkit/") and name.endswith(".py")}
    original = ops["ACCEPTED_PYTHON"]
    tree = ast.parse(original)
    classes = [ast.get_source_segment(original, node) for node in tree.body
        if isinstance(node, ast.ClassDef) and node.name in {"AcceptedLoader", "AcceptedFinder"}]
    _require(len(classes) == 2, "The original accepted-source loader changed.")
    namespace = {"__name__": "forge_verified_control_source", "root": folder / "control-host",
        "files": files, "importlib": importlib}
    exec(compile("\n".join(classes), "/__forge_frozen__/control-source-loader.py", "exec"), namespace)
    sys.meta_path.insert(0, namespace["AcceptedFinder"]())


def _verify_installed_control_dependencies(control, manifest, site):
    """Compare importable installed bytes with the original selected wheel batch.

    Pip-generated RECORD/INSTALLER/REQUESTED data is not executable payload.
    This readback does not confer native product or application readiness.
    """
    _require(site.is_dir() and not any(path.is_symlink() for path in (site, *site.parents))
        and site.stat().st_uid == os.getuid(), "The original control import directory changed.")
    destinations = set()
    for name in manifest["dependencies"]["wheels"]:
        row = manifest["files"][name]
        content = _read(control / name)
        _require(len(content) == row["size"] and hashlib.sha256(content).hexdigest() == row["sha256"],
            "The original selected control wheel changed before imports.")
        with zipfile.ZipFile(io.BytesIO(content)) as archive:
            for entry in archive.infolist():
                if entry.is_dir():
                    continue
                relative = entry.filename
                _require(not relative.startswith("/") and "\\" not in relative
                    and not any(part in {"", ".", ".."} for part in relative.split("/")),
                    "The selected control dependency import path is unsafe.")
                if ".data/" in relative:
                    prefix, relative = relative.split(".data/", 1)
                    kind, relative = relative.split("/", 1)
                    if kind in {"scripts", "headers", "data"}:
                        continue
                    _require(kind in {"purelib", "platlib"},
                        "The selected control wheel import scheme is unsupported.")
                _require(not relative.endswith(".pth"),
                    "The selected control graph cannot inject site startup code.")
                if relative.endswith(".dist-info/RECORD"):
                    continue
                _require(relative not in destinations, "The selected control import payload overlaps.")
                destinations.add(relative)
                _require(_read(site / relative, 64 * 1024 * 1024) == archive.read(entry),
                    "The installed control dependency payload changed before imports.")
    # No unselected executable file can become a dependency or shadow stdlib.
    # The finite launchkit loader bypasses caches. Selected dependency caches
    # are refused below; -B only prevents new bytecode writes. The pip child
    # separately imports the interpreter's exact sealed bootstrap wheel.
    selected_roots = {name.split("/", 1)[0] for name in destinations}
    for name in selected_roots:
        selected = site / name
        for path in ([selected] if selected.is_file() else selected.rglob("*")):
            _require(not path.is_symlink() and path.lstat().st_uid == os.getuid(),
                "The installed control import payload is not owned.")
            if path.is_dir():
                continue
            relative = path.relative_to(site).as_posix()
            generated = ".dist-info/" in relative and relative.rsplit("/", 1)[1] in {
                "RECORD", "INSTALLER", "REQUESTED", "direct_url.json"}
            _require(relative in destinations or generated,
                "The installed control dependency contains unselected executable bytes.")
    for path in site.iterdir():
        if path.name in selected_roots:
            continue
        _require(path.name in {"pip", "setuptools", "pkg_resources", "_distutils_hack", "distutils-precedence.pth"}
            or re.fullmatch(r"(?:pip|setuptools)-[^/]+\.dist-info", path.name),
            "The original control import directory contains an unselected payload.")
    return destinations


# Only the interpreter's own accepted ensurepip bytes enter this nested child.
# Mutable site packages and .pth/sitecustomize are never placed on its path.
CONTROL_PIP_ENTRY = r'''import hashlib,json,os,stat,sys
from pathlib import Path
control=Path(sys.argv[1]); descriptor=int(sys.argv[2]); pin=json.loads(sys.argv[3])
with os.fdopen(os.dup(descriptor),'rb') as channel:
    channel.seek(0); info=os.fstat(channel.fileno()); data=channel.read(33554433)
if (not stat.S_ISREG(info.st_mode) or info.st_uid!=os.getuid()
        or len(data)!=pin['size'] or len(data)>33554432
        or hashlib.sha256(data).hexdigest()!=pin['sha256']):
    raise RuntimeError('The original interpreter pip bootstrap changed.')
sys.prefix=sys.exec_prefix=str(control/'.venv')
location=('/proc/self/fd/' if Path('/proc/self/fd').is_dir() else '/dev/fd/')+str(descriptor)
sys.path.insert(0,location)
from pip._internal.cli.main import main
raise SystemExit(main(sys.argv[4:]))
'''


def _stdlib_pip_bootstrap():
    """Retain the exact wheel chosen by this trusted interpreter's ensurepip.

    Public bootstrap uses its own bundled/distro selection. Its existing child
    seam is intercepted only to retain bytes, never to execute or install pip.
    This runs in the verified no-site child after actual control venv creation.
    Unsupported interpreters refuse before selected dependency installation.
    """
    import ensurepip
    _require(callable(getattr(ensurepip, "_run_pip", None)),
        "This interpreter cannot retain its canonical pip bootstrap without site startup.")
    original, environment = ensurepip._run_pip, dict(os.environ)
    selected = []

    def retain(arguments, additional_paths=None):
        _require(not selected and type(additional_paths) in {list, tuple}
            and 0 < len(additional_paths) <= 8,
            "This interpreter's canonical pip bootstrap selection is unsupported.")
        wheels = [Path(name) for name in additional_paths
            if re.fullmatch(r"pip-[A-Za-z0-9_.+!-]+\.whl", Path(name).name)]
        _require(len(wheels) == 1, "This interpreter's canonical pip wheel is unavailable or ambiguous.")
        content = _read(wheels[0].resolve(), 32 * 1024 * 1024)
        with zipfile.ZipFile(io.BytesIO(content)) as archive:
            _require({"pip/__init__.py", "pip/_internal/cli/main.py"} <= set(archive.namelist()),
                "This interpreter's canonical wheel lacks its original pip entry.")
        selected.append((content, {"name": wheels[0].name, "size": len(content),
            "sha256": hashlib.sha256(content).hexdigest()}))
        return 0

    ensurepip._run_pip = retain
    try:
        ensurepip.bootstrap()
    except (TypeError, AttributeError) as error:
        raise ValueError("This interpreter's canonical pip bootstrap seam is unsupported.") from error
    finally:
        ensurepip._run_pip = original
        os.environ.clear()
        os.environ.update(environment)
    _require(len(selected) == 1, "This interpreter did not retain its canonical pip bootstrap.")
    return selected[0]


def _control_dependency_executor(control, pip_bootstrap, current):
    """Use the existing installer executor seam without changing ordinary pip."""
    import subprocess
    from . import remote_destination as ops_module
    _require(type(pip_bootstrap) is tuple and len(pip_bootstrap) == 2,
        "Control installation requires its original interpreter pip bootstrap bytes.")
    data, pin = pip_bootstrap
    _require(type(data) is bytes and type(pin) is dict and set(pin) == {"name", "size", "sha256"}
        and len(data) == pin["size"] and hashlib.sha256(data).hexdigest() == pin["sha256"],
        "The original interpreter pip bootstrap bytes changed.")
    root = control.parents[2]
    _require(control == ops_module._directory(root) / "control-host",
        "Control dependency installation belongs to this original staged host.")

    def execute(arguments, *, input=None, pip=False):
        current()
        _require(input is None and pip is True and arguments[:4] == [str(control / ".venv/bin/python"), "-I", "-m", "pip"],
            "The control executor cannot replace another original installer command.")
        with ops_module._input_descriptor(data, root) as descriptor:
            result = subprocess.run([arguments[0], "-I", "-S", "-B", "-c", CONTROL_PIP_ENTRY,
                str(control), str(descriptor), _json(pin).decode(), *arguments[4:]],
                check=False, capture_output=True, close_fds=True, pass_fds=(descriptor,),
                env=dict(os.environ, PIP_CONFIG_FILE=os.devnull))
        current()
        return {"returncode": result.returncode, "stdout": result.stdout, "stderr": result.stderr}

    return execute


def worker_install(ops, root, owner, operation_id, settings_fd, manifest):
    _require(type(settings_fd) is int and settings_fd >= 0, "The original sealed settings input is absent.")
    with os.fdopen(settings_fd, "rb") as stream:
        settings = _decode(stream.read(ops["MAX_PRIVATE"] + 1))
    _require(type(settings) is dict and len(_json(settings)) <= ops["MAX_PRIVATE"],
        "The original private settings input exceeds its bound.")
    _source_current(ops, root, owner, manifest)
    guest_interpreter(manifest)
    folder = ops["_directory"](root)
    files = _bundle_files(_read(folder / "control-bundle.zip"), manifest)
    control = folder / "control-host"
    _require(not control.exists() and not control.is_symlink(),
        "Preserve the original entered control bootstrap; no retry was admitted.")
    control.mkdir(mode=0o700)
    for name, data in sorted(files.items()):
        path = control / name
        path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
        ops["_write"](path, data, exclusive=True)
    def current():
        _source_current(ops, root, owner, manifest)
        job = ops["_job"](root)
        _require(job and job["operation_id"] == operation_id and job["action"] == "install"
            and ops["_live"](job.get("worker")), "The original product Install operation changed.")
        for name, expected in files.items():
            _require(_read(control / name) == expected, "The original trusted host source changed.")
    current()
    ops["_command"]([sys.executable, "-I", "-m", "venv", str(control / ".venv")],
        root, owner, operation_id, "python-environment")
    current()
    python = control / ".venv/bin/python"
    job = ops["_job"](root)
    expected = _json({"owner": owner, "manifest": manifest, "worker": job["worker"]}).decode()
    ops["_command"]([str(python), "-I", "-S", "-B", "-c", CONTROL_CHILD_ENTRY,
        str(root), expected, operation_id, "dependencies"], root, owner, operation_id, "dependencies")
    current()
    # The original sealed descriptor is not recreated as a path or journal input.
    with ops["_input_descriptor"](_json(settings), root) as private:
        settings.clear()
        ops["_command"]([str(python), "-I", "-S", "-B", "-c", CONTROL_CHILD_ENTRY,
            str(root), expected, operation_id, "owner", str(private)],
            root, owner, operation_id, "application-source", timeout=None, pass_fds=(private,))
    current()
    outcome = _decode(_read(folder / "product-install-completed.json", 1024 * 1024))
    _require(outcome.get("operation_id") == operation_id, "The original actual Install completion changed.")


def install_control_dependencies(control, refs, *, site_packages, pip_bootstrap=None):
    from .runtime_candidates import ArtifactFile
    from .templates.install_candidate import install_selected_dependency_wheels
    # The interpreter's default scheme can name the system site-packages, so
    # the caller names the created control venv's own site, the only target.
    venv = Path(control) / ".venv"
    _require(isinstance(site_packages, Path) and ".." not in site_packages.parts
        and site_packages != venv and site_packages.is_relative_to(venv),
        "The control dependencies install only into the created control environment.")
    requirements = ArtifactFile(**{**refs["dependencies/requirements.txt"],
        "path": Path(refs["dependencies/requirements.txt"]["path"])})
    wheels = tuple(ArtifactFile(**{**row, "path": Path(row["path"])})
        for name, row in sorted(refs.items()) if name != "dependencies/requirements.txt")
    def current():
        for row in refs.values():
            data = _read(row["path"])
            _require(len(data) == row["size"] and hashlib.sha256(data).hexdigest() == row["sha256"],
                "The original owner-selected control dependency changed.")
    install_selected_dependency_wheels(requirements, wheels, deployment_root=control,
        site_packages=site_packages, sdk_python=control / ".venv/bin/python",
        require_current=current, execute_command=_control_dependency_executor(control, pip_bootstrap, current))
    import ast
    folder = control.parent
    values = [ast.literal_eval(node.value) for node in ast.parse(_read(folder / "controller.py", MAX_SOURCE)).body
        if isinstance(node, ast.Assign) and any(isinstance(target, ast.Name)
            and target.id == "PRODUCT_CONTROL_MANIFEST" for target in node.targets)]
    _require(len(values) == 1, "The original selected control manifest is absent.")
    _verify_installed_control_dependencies(control, _decode(values[0]), site_packages)
    if str(site_packages) not in sys.path:
        sys.path.insert(0, str(site_packages))
    # These actual imports happen before any application installer is admitted.
    import httpx
    from .blueprint import Blueprint
    _require(httpx.AsyncClient is not None and Blueprint is not None, "The actual control imports are unavailable.")


async def run_initial_install(root, operation_id, settings_fd, *, retain_control=False):
    import asyncio
    folder = root / ".mobkit/launch-remote"
    raw = _decode(_read(folder / "owner.json", 1024 * 1024))
    source_bytes = _read(folder / "controller.py", 4 * 1024 * 1024)
    _require(hashlib.sha256(source_bytes).hexdigest() == raw["binding"]["controller_sha256"],
        "The original trusted controller changed before actual Install.")
    # Source is verified data. Reading this literal never executes a controller.
    import ast
    tree = ast.parse(source_bytes)
    values = [ast.literal_eval(node.value) for node in tree.body if isinstance(node, ast.Assign)
        and any(isinstance(target, ast.Name) and target.id == "PRODUCT_CONTROL_MANIFEST" for target in node.targets)]
    _require(len(values) == 1, "The trusted product control selection is absent.")
    manifest = _decode(values[0])
    _control_materialized_files(root, manifest)
    # The production child has already checked the original frozen controller,
    # source intake, operation birth and dependency payload before this import.
    from . import remote_destination as ops_module
    ops = vars(ops_module)
    ops["PRODUCT_CONTROL_MANIFEST"] = values[0]
    ops["CONTROLLER_SOURCE"] = source_bytes
    adapters = [ast.literal_eval(node.value) for node in tree.body if isinstance(node, ast.Assign)
        and any(isinstance(target, ast.Name) and target.id == "PRODUCT_CONTROL_ADAPTER_SOURCE" for target in node.targets)]
    adapter_pin = manifest["files"]["launchkit/remote_product_control.py"]
    _require(len(adapters) == 1 and type(adapters[0]) is bytes and len(adapters[0]) == adapter_pin["size"]
        and hashlib.sha256(adapters[0]).hexdigest() == adapter_pin["sha256"],
        "The authenticated controller adapter differs from its frozen source selection.")
    ops["PRODUCT_CONTROL_ADAPTER_SOURCE"] = adapters[0]
    owner = ops["_owner"](root, raw["deployment_id"], raw["binding"], raw["host"])
    context, source = source_context(ops, root, owner, manifest)
    with os.fdopen(settings_fd, "rb") as stream:
        settings = _decode(stream.read(ops["MAX_PRIVATE"] + 1))
    _require(type(settings) is dict and len(_json(settings)) <= ops["MAX_PRIVATE"],
        "The original sealed settings input changed.")
    task, loop = asyncio.current_task(), asyncio.get_running_loop()
    control = folder / "control-host"
    observed = []
    def stamp(path):
        info = path.lstat()
        _require(not path.is_symlink() and stat.S_ISREG(info.st_mode)
            and info.st_uid == os.getuid() and info.st_nlink == 1,
            "The original control observation is not an owned file.")
        return (info.st_dev, info.st_ino, info.st_mode, info.st_uid, info.st_nlink,
            info.st_size, info.st_mtime_ns, info.st_ctime_ns)
    # These bytes were authenticated before imports. Repeated owner callbacks
    # fence original file observations without hashing a large closure on the loop.
    for name, row in manifest["files"].items():
        path = control / name
        data = _read(path)
        _require(len(data) == row["size"] and hashlib.sha256(data).hexdigest() == row["sha256"],
            "The original trusted control input changed before actual Install.")
        observed.append((path, stamp(path)))
    for path in (folder / "owner.json", folder / "controller.py", folder / "control-selection.json",
            folder / "control-bundle.zip"):
        observed.append((path, stamp(path)))
    def current():
        _require(asyncio.current_task() is task and asyncio.get_running_loop() is loop,
            "The original destination Install task changed.")
        _require(_decode(_read(folder / "owner.json", 1024 * 1024)) == owner
            and all(stamp(path) == expected for path, expected in observed),
            "The original product control input owner changed.")
        job = ops["_job"](root)
        command = job.get("command") if job else None
        _require(job and job["operation_id"] == operation_id and job["action"] == "install"
            and ops["_live"](job.get("worker")) and type(command) is dict
            and command.get("pid") == os.getpid() and ops["_live"](command),
            "The actual original remote command owner changed.")
    current()
    from .deployment import DeploymentService, read_receipt
    from .destination_application_inputs import create_destination_application_inputs
    from .remote_product_destination_owner import open_stopped_product_source
    from .templates.storage_setup import project_lock
    current()
    service = DeploymentService(Path(context["host"]["home"]) / ".local/state/meta-forge", None, None)
    inputs = None
    initial_completed = False
    try:
        async with service.lock:
            with project_lock(root) as lease:
                with open_stopped_product_source(context, project_lease=lease,
                        require_current=current, owner_source=source) as selected:
                    inputs = await create_destination_application_inputs(selected,
                        deployment=service, require_current=current)
        await inputs.factory.install(settings=settings)
        current()
        receipt = read_receipt(inputs.factory.directory)
        _require(receipt.get("status") == "stopped" and receipt.get("phase") == "configure"
            and receipt.get("installed_candidate_runtime_proof"), "The real byte Install has not completed stopped.")
        content = _read(inputs.factory.directory / "receipt.json", 1024 * 1024)
        result = {"operation_id": operation_id, "context": context,
            "installation_receipt_sha256": hashlib.sha256(content).hexdigest(),
            "canonical_directory": str(inputs.factory.directory), "scope": "byte-install-stopped"}
        ops["_json_write"](folder / "product-install-completed.json", result, exclusive=True)
        initial_completed = True
        if retain_control:
            settings.clear()
            await _retain_control_owner(ops, root, owner, manifest, operation_id,
                service, inputs, context, current)
    except BaseException:
        if not initial_completed and inputs is not None and inputs.factory._pending_scope is not None:
            await inputs.factory.cleanup_installation()
        raise
    finally:
        settings.clear()
        # Only the original retained owner may admit runtime work. Its joined
        # shutdown or failed Install must preserve any unresolved actual owner.
        _require(not service.tasks and not service.processes and not service._application_preparations,
            "The original stopped installer still owns live work.")


def observe(ops, root, owner, manifest):
    _source_current(ops, root, owner, manifest)
    context, _ = source_context(ops, root, owner, manifest)
    verify_package_source(root, context)
    job = ops["_job"](root)
    live = bool(job and ops["_live"](job.get("worker")))
    endpoint = ops["_directory"](root) / "product-control-owner.json"
    if endpoint.exists() or endpoint.is_symlink():
        # The original completion witness remains history. Every later lifecycle
        # fact comes from the retained real factory, not its initial receipt hash.
        return request_control(ops, root, owner, manifest, {"action": "status",
            "deployment_id": owner["deployment_id"], "binding": owner["binding"],
            "destination": str(root), "expected_host": owner["host"]})
    state = "installing" if live else "not-installed" if job is None else "recovery-needed"
    complete = ops["_directory"](root) / "product-install-completed.json"
    if job and not live and job.get("status") == "completed" and job.get("cleanup_confirmed") is True:
        result = _decode(_read(complete, 1024 * 1024))
        context, _ = source_context(ops, root, owner, manifest)
        directory = Path(context["host"]["home"]) / ".local/state/meta-forge/deployments" / context["guide_id"] / context["deployment_id"]
        receipt_bytes = _read(directory / "receipt.json", 1024 * 1024)
        receipt = _decode(receipt_bytes)
        _require(result == {"operation_id": job["operation_id"], "context": context,
            "installation_receipt_sha256": hashlib.sha256(receipt_bytes).hexdigest(),
            "canonical_directory": str(directory), "scope": "byte-install-stopped"}
            and all(receipt.get(key) == context[key] for key in ("guide_id", "deployment_id", "build_id", "package_sha256"))
            and receipt.get("status") == "stopped" and receipt.get("phase") == "configure",
            "The completed actual byte Install changed during observation.")
        state = "stopped"
    return {"status": state, "ready": False, "retryable": False,
        "worker_pid": job.get("worker", {}).get("pid") if job else None, "worker_live": live,
        "source_current": True, "service": {"status": "not-installed", "ready": False},
        "service_owned": False, "service_observed": False, "admitted": job is not None,
        "cleanup_confirmed": bool(job and not live and job.get("cleanup_confirmed") is True),
        "private_settings_supported": False, "private_settings_configurable": False,
        "scope": "byte-install-stopped", "operation_id": job["operation_id"] if job else None,
        "phase": "configure" if state == "stopped" else job.get("phase") if job else None,
        "detail": "The product byte installer is stopped. Native setup and runtime actions remain separate."}


# This source is appended to the exact frozen trusted control module.
CONTROL_ACTIONS = frozenset({"prepare-setup", "finish-setup", "start-background", "stop", "restart"})
CONTROL_READS = frozenset({"status", "application-setup", "settings-options"})


def _control_peer(channel):
    import socket
    _require(hasattr(socket, "SO_PEERCRED"), "Product control requires its actual Linux peer credentials.")
    return struct.unpack("3i", channel.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, 12))


def _socket_identity(path):
    path = Path(path)
    info = path.lstat()
    parent = path.parent.lstat()
    _require(not path.is_symlink() and not path.parent.is_symlink()
        and stat.S_ISSOCK(info.st_mode) and info.st_uid == os.getuid()
        and info.st_mode & 0o077 == 0 and stat.S_ISDIR(parent.st_mode)
        and parent.st_uid == os.getuid() and parent.st_mode & 0o077 == 0,
        "The original product control endpoint lost private custody.")
    return [info.st_dev, info.st_ino, info.st_mode, info.st_uid]


def _control_record(ops, root, owner, manifest):
    """Immutable endpoint discovery, never a lifecycle journal or new admission."""
    folder = ops["_directory"](root)
    record = _decode(_read(folder / "product-control-owner.json", 16 * 1024))
    job = ops["_job"](root)
    complete = _read(folder / "product-install-completed.json", 1024 * 1024)
    _require(type(record) is dict and set(record) == {"protocol", "install_operation_id", "binding",
        "control_manifest_sha256", "initial_install_sha256", "worker", "command", "socket"}
        and record["protocol"] == 1 and record["binding"] == owner["binding"]
        and record["control_manifest_sha256"] == hashlib.sha256(_json(manifest)).hexdigest()
        and record["initial_install_sha256"] == hashlib.sha256(complete).hexdigest()
        and job and job["operation_id"] == record["install_operation_id"] and job["action"] == "install"
        and job.get("worker") == record["worker"] and job.get("command") == record["command"]
        and ops["_live"](record["worker"]) and ops["_live"](record["command"]),
        "The original retained product control process is unavailable or changed.")
    endpoint = record["socket"]
    _require(type(endpoint) is dict and set(endpoint) == {"path", "identity"}
        and type(endpoint["path"]) is str and len(os.fsencode(endpoint["path"])) < 104
        and _socket_identity(endpoint["path"]) == endpoint["identity"],
        "The original product control socket changed.")
    return record


def _public_control_binding(record):
    return {key: deepcopy(record[key]) for key in ("protocol", "install_operation_id", "binding",
        "control_manifest_sha256", "initial_install_sha256", "worker", "command")}


def request_control(ops, root, owner, manifest, request):
    """The verified SSH controller forwards to its exact retained local owner."""
    import socket
    _source_current(ops, root, owner, manifest)
    record = _control_record(ops, root, owner, manifest)
    payload = _json({"control": _public_control_binding(record), "request": request})
    _require(len(payload) <= ops["MAX_PRIVATE"], "The original product request exceeds its bound.")
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as channel:
        channel.settimeout(25)
        channel.connect(record["socket"]["path"])
        pid, uid, _ = _control_peer(channel)
        _require(pid == record["command"]["pid"] and uid == os.getuid(),
            "The product request reached another local process.")
        channel.sendall(struct.pack("!I", len(payload)) + payload)
        def exact(size):
            value = bytearray()
            while len(value) < size:
                part = channel.recv(size - len(value))
                _require(part, "The original product response was interrupted.")
                value.extend(part)
            return bytes(value)
        size = struct.unpack("!I", exact(4))[0]
        _require(0 < size <= ops["MAX_RESULT"], "The product response exceeds its bound.")
        answer = _decode(exact(size))
    _require(_control_record(ops, root, owner, manifest) == record
        and type(answer) is dict and answer.get("product_control", {}).get("binding") == _public_control_binding(record),
        "The product response lost its original retained process binding.")
    _source_current(ops, root, owner, manifest)
    return answer


def _public_factory_status(value):
    """Original observer facts only; private logs and arbitrary diagnostics stay local."""
    names = ("guide_id", "guide_version", "deployment_id", "build_id", "source_kind", "source_digest",
        "package_sha256", "destination", "target", "status", "phase", "execution_mode", "current",
        "current_guide_version", "source_verified", "process_live", "runtime_pid", "health", "health_checked_at",
        "console_url", "bootstrap", "startup_attempt_id", "startup_console_health", "runtime_candidate",
        "installed_candidate_runtime_proof", "interactive_lifecycle_id", "interactive_cleanup",
        "interactive_cleanup_attempt_id", "service", "service_observation", "service_cleanup_confirmed",
        "service_supervisor_pid", "service_instance_id", "updated_at", "update")
    return {key: deepcopy(value[key]) for key in names if key in value}


def _product_runtime_binding(receipt):
    """Canonical application identity, excluding changing read-only health times."""
    from .deployment_service import service_binding
    names = ('guide_id', 'guide_version', 'deployment_id', 'build_id', 'package_sha256',
        'destination', 'status', 'phase', 'execution_mode', 'runtime_pid', 'runtime_candidate',
        'startup_attempt_id', 'interactive_lifecycle_id', 'interactive_cleanup',
        'interactive_cleanup_attempt_id', 'service_supervisor_pid', 'service_instance_id',
        'service_cleanup_confirmed', 'bootstrap', 'console_url', 'update')
    return {**{key: deepcopy(receipt.get(key)) for key in names},
        'service': service_binding(receipt.get('service') or {})}


async def _product_application_experiences(ops, service, factory, context, root, owner, actual, current):
    """Join declared links to the original factory and actual listener owner.

    No source URL, saved PID, live control worker or shared cgroup establishes
    application listener custody. This reader starts no application or service.
    """
    import asyncio
    import time
    from .application_experiences import declared_application_experiences, resolve_application_experiences
    from .deployment import (read_receipt, await_validation, _observe_status_package,
        _require_status_package_current, OWNER_FILE)
    from .deployment_evidence import project_deployment_evidence
    from .deployment_service import verify_service, service_binding
    from .deployment_updates import checked_package
    from .connect_application_update_startup import _file_identity
    from .installed_feedback import _mutating
    binding = owner['binding']
    # The imported canonical parser is in the verified control closure. The
    # child module does not inherit the standalone controller's parser literal.
    result = {'protocol': 1, 'supported': True,
        'status': 'unavailable', 'source_digest': binding['accepted_source_digest'],
        'installed_source_digest': binding['transferred_source_digest'], 'metadata_sha256': None, 'entries': []}
    if not result['supported'] or actual.get('source_verified') is not True:
        return result
    try:
        current()
        async with service.lock:
            factory._identity(service, context['guide_id'], context['deployment_id'])
            directory = factory.directory
            canonical = read_receipt(directory)
            selected = _product_runtime_binding(actual)
            _require(_product_runtime_binding(canonical) == selected,
                'The original application instance changed before interface observation.')
            # Retain these original application objects before the first await.
            # Equal saved PIDs cannot let a later replacement become the anchor.
            original_process = service.processes.get(context['deployment_id'])
            original_task = service.tasks.get(context['deployment_id'])
            original_admission = service.worker_admissions.get(context['deployment_id'])
            original_admission_state = deepcopy(original_admission)
            original_manager = service.service_manager
            def source_observation(directory, canonical, context):
                _, files = checked_package(directory, 'package.zip', canonical['package_sha256'])
                stamps = tuple((root / name, _file_identity(root / name)) for name in sorted(files))
                stamps += ((root / OWNER_FILE, _file_identity(root / OWNER_FILE)),)
                source = service._destination_control_package(directory, canonical, context)
                _require(all(_file_identity(path) == identity for path, identity in stamps),
                    'The installed source changed during its original package verification.')
                return source, stamps, files
            # Reuse the actual factory's installed package, not the legacy
            # service's archive/parser dependency snapshots. The canonical
            # parser above is part of the already verified control closure.
            (source, source_stamps, files), package_observation = await await_validation(_observe_status_package,
                source_observation, directory, deepcopy(canonical), deepcopy(context))
            current()
            factory._identity(service, context['guide_id'], context['deployment_id'])
            _require(source['source_digest'] == context['source_digest'],
                'The original application source changed before interface observation.')
            definition = json.loads(files['deployment.json'][0])
            declared = declared_application_experiences(files, definition)
            result['metadata_sha256'] = declared['metadata_sha256']
            keys = {entry['port_field'] for entry in declared['entries'] if 'port_field' in entry}
            port_stamp = _file_identity(root / '.env') if keys else None
            values = ops['_interface_settings'](root, keys)
            native = project_deployment_evidence({'id': context['guide_id']}, actual)
            resolved = resolve_application_experiences(files, definition,
                source_digest=binding['accepted_source_digest'], configuration_values=values,
                console_url=actual.get('console_url') if native['native_runtime_available'] else None)
            result['entries'] = [{key: entry[key] for key in ('id', 'label', 'path')}
                | {'destination_port': entry['destination_port'], 'listener_owned': False, 'listener_token': None,
                    'reason': entry['reason'] or ops['INTERFACE_UNAVAILABLE'], 'verified': False}
                for entry in resolved['entries']]
            result['status'] = 'resolved' if declared['entries'] else 'none'
            def source_current():
                current()
                factory._identity(service, context['guide_id'], context['deployment_id'])
                _require(service._directory(context['guide_id'], context['deployment_id']) == directory
                    and _product_runtime_binding(read_receipt(directory)) == selected,
                    'The original application source or instance changed during interface observation.')
                _require_status_package_current(directory, package_observation)
                _require(all(_file_identity(path) == identity for path, identity in source_stamps)
                    and (not keys or _file_identity(root / '.env') == port_stamp),
                    'The original installed source or declared port file changed during interface observation.')
            source_current()
            if not declared['entries'] or not native['native_runtime_available'] or _mutating(actual):
                return result
            deadline = time.monotonic() + ops['INTERFACE_SECONDS']
            mode = canonical.get('execution_mode')
            process = task = admission = manager = selected_service = None
            if mode == 'interactive':
                process, task, admission = original_process, original_task, original_admission
                _require(process is not None and type(process.pid) is int and process.pid > 0
                    and service.processes.get(context['deployment_id']) is process
                    and process.returncode is None and task is not None and not task.done() and not task.cancelling()
                    and service.tasks.get(context['deployment_id']) is task
                    and type(admission) is dict and admission.get('spawn_started') is True
                    and service.worker_admissions.get(context['deployment_id']) is admission
                    and admission == original_admission_state
                    and admission.get('mode') == 'start'
                    and admission.get('lifecycle_id') == canonical.get('interactive_lifecycle_id')
                    and not canonical.get('service') and canonical.get('interactive_cleanup') is None,
                    'The original interactive application handle is unavailable.')
                admission_state = original_admission_state
                anchor = {'kind': 'interactive-handle', 'pid': process.pid,
                    'lifecycle_id': canonical['interactive_lifecycle_id'], 'parent_pid': os.getpid()}
            elif mode == 'background':
                manager = original_manager
                selected_service = verify_service(directory, canonical)
                _require(service.service_manager is manager and selected_service.get('manager') == 'systemd-user'
                    and type(canonical.get('service_supervisor_pid')) is int
                    and canonical['service_supervisor_pid'] > 0 and canonical.get('service_instance_id'),
                    'The original background application manager is unavailable.')
                ops['_listener_budget'](deadline)
                async with asyncio.timeout(max(0.01, deadline - time.monotonic())):
                    manager_before = await manager.observe(selected_service)
                group = manager_before.get('control_group')
                _require(manager_before.get('loaded') is True and manager_before.get('state') == 'active'
                    and manager_before.get('substate') == 'running'
                    and manager_before.get('control_pid') in (None, 0)
                    and type(manager_before.get('pid')) is int
                    and manager_before['pid'] == canonical['service_supervisor_pid']
                    and isinstance(group, str) and Path(group).name == selected_service['label'] + '.service',
                    'The original background application supervisor changed.')
                anchor = {'kind': 'background-manager', 'pid': manager_before['pid'], 'control_group': group,
                    'instance_id': canonical['service_instance_id'], 'service': service_binding(selected_service)}
            else:
                raise ValueError('The original application lifecycle is unavailable.')
            def retained():
                source_current()
                ops['_listener_budget'](deadline)
                if process is not None:
                    _require(service.processes.get(context['deployment_id']) is process and process.returncode is None
                        and service.tasks.get(context['deployment_id']) is task and not task.done() and not task.cancelling()
                        and service.worker_admissions.get(context['deployment_id']) is admission
                        and admission == admission_state,
                        'The original application handle or admission changed during listener observation.')
                else:
                    _require(service.service_manager is manager
                        and service_binding(verify_service(directory, read_receipt(directory))) == service_binding(selected_service),
                        'The original background application definition changed during listener observation.')
            retained()
            tokens = {}
            runtime_binding = {**selected, 'source_digest': context['source_digest'],
                'metadata_sha256': declared['metadata_sha256']}
            for entry, resolved_entry in zip(result['entries'], resolved['entries']):
                port = entry['destination_port']
                if port is not None and not resolved_entry['reason']:
                    if port not in tokens:
                        try:
                            tokens[port] = ops['_product_listener_proof'](owner, runtime_binding, anchor, port,
                                deadline, retained)
                        except (ValueError, OSError):
                            tokens[port] = None
                        retained()
                    if tokens[port] is not None:
                        entry.update(listener_owned=True, listener_token=tokens[port], reason=None)
            if manager is not None:
                retained()
                async with asyncio.timeout(max(0.01, deadline - time.monotonic())):
                    manager_after = await manager.observe(selected_service)
                _require(manager_after == manager_before,
                    'The original service manager changed while reading the listener.')
                # An awaited manager query cannot retain an earlier kernel
                # process birth or socket association without observing again.
                for port, token in tokens.items():
                    if token is not None:
                        _require(ops['_product_listener_proof'](owner, runtime_binding, anchor, port,
                            deadline, retained) == token, 'The listener changed during manager observation.')
            retained()
            _require(ops['_interface_settings'](root, keys) == values,
                'The declared port changed during listener observation.')
            retained()
    except Exception:
        # Fixed public text cannot echo private settings or kernel diagnostics.
        # Missing listener custody affects only links, not canonical operations.
        for entry in result['entries']:
            entry.update(listener_owned=False, listener_token=None, reason=ops['INTERFACE_UNAVAILABLE'])
    current()
    return result


async def _dispatch_control_request(service, inputs, context, root, owner, operation_id,
        record, envelope, current, *, ops):
    """Only the original Install task calls the actual factory and canonical journal."""
    from .deployment_evidence import project_deployment_evidence
    current()
    _require(type(envelope) is dict and set(envelope) == {"control", "request"}
        and envelope["control"] == _public_control_binding(record), "The retained product request owner changed.")
    request = envelope["request"]
    _require(type(request) is dict and request.get("binding") == owner["binding"]
        and request.get("deployment_id") == context["deployment_id"] and request.get("destination") == str(root)
        and request.get("expected_host") == owner["host"], "The product request selects another original destination.")
    action = request.get("action")
    _require(action in CONTROL_ACTIONS | CONTROL_READS | {"configure"}, "This product control action is unavailable.")
    factory, gid, did = inputs.factory, context["guide_id"], context["deployment_id"]
    selected_request, result = request.get("operation_id"), None
    if action in CONTROL_ACTIONS | {"configure"}:
        _require(type(selected_request) is str, "Product actions require their exact canonical request UUID.")
        try:
            ops_id = str(__import__("uuid").UUID(selected_request))
        except (ValueError, TypeError, AttributeError):
            raise ValueError("Product actions require their exact canonical request UUID.") from None
        _require(selected_request == ops_id, "Product actions require their exact canonical request UUID.")
        if action == "configure":
            result = await factory.configure(service, gid, did, action_request_id=selected_request,
                version=context["guide_version"], build_id=context["build_id"],
                source_digest=context["source_digest"], package_sha256=context["package_sha256"],
                settings=request.get("settings", {}), remove_fields=request.get("remove_fields", []),
                use_host_provider_credentials=False)
        else:
            result = await factory.action(service, gid, did, action, action_request_id=selected_request)
    current()
    actual = await factory.status(service, gid, did)
    current()
    _require(all(actual.get(key) == context[key] for key in ("guide_id", "deployment_id", "build_id", "package_sha256"))
        and actual.get("source_digest") == context["source_digest"] and actual.get("destination") == str(root),
        "The actual factory observer changed its original installed source.")
    setup, options = None, None
    if actual.get("status") == "stopped" and actual.get("execution_mode", "interactive") == "interactive" and not actual.get("service"):
        if action in {"status", "application-setup", "configure", "prepare-setup", "stop"}:
            setup = await factory.setup_status(service, gid, did)
        if action == "settings-options":
            options = await factory.settings_options(service, gid, did)
    current()
    experiences = await _product_application_experiences(ops, service, factory, context, root, owner, actual, current)
    current()
    native = project_deployment_evidence({"id": gid}, actual)
    status = actual.get("status")
    if status in {"queued", "interrupted"}:
        status = "installing" if status == "queued" else "recovery-needed"
    response = {"status": status, "ready": native["native_runtime_available"], "retryable": False,
        "operation_id": operation_id, "phase": actual.get("phase"), "source_current": actual.get("source_verified") is True,
        "worker_pid": record["worker"]["pid"], "worker_live": True, "admitted": True,
        "cleanup_confirmed": False, "service": {}, "service_owned": False, "service_observed": False,
        "private_settings_supported": True, "private_settings_configurable": False,
        "application_experiences": experiences,
        "scope": "retained-original-product-owner", "product_control": {
            "binding": _public_control_binding(record), "owner_live": True,
            "initial_install_complete": True, "factory_status": _public_factory_status(actual),
            "setup_status": setup, "native_evidence": native},
        "detail": "The original product owner observed this application's saved source and runtime."}
    if options is not None:
        response.update(configured_keys=[field["key"] for field in options["fields"] if field["configured"]],
            console_port=options["console_port"], private_settings_configurable=True)
    if isinstance(result, dict) and isinstance(result.get("action_request"), dict):
        response["operation_id"] = selected_request
        response["product_control"]["action_request"] = deepcopy(result["action_request"])
        if action == "configure":
            response["configuration_saved"] = (result["action_request"].get("phase") == "settled"
                and result["action_request"].get("outcome") == "returned")
            response["private_settings_configurable"] = actual.get("status") == "stopped" and actual.get("source_verified") is True
    elif selected_request is not None and selected_request != operation_id:
        try:
            value = await service.action_request(gid, did, selected_request)
        except (LookupError, FileNotFoundError):
            pass
        else:
            response["operation_id"] = selected_request
            response["product_control"]["action_request"] = deepcopy(value["action_request"])
    return response


async def _retain_control_owner(ops, root, owner, manifest, operation_id, service, inputs, context, current):
    """Bounded private IPC; every effect stays serialized on this existing task."""
    import asyncio
    import signal
    queue = asyncio.Queue(maxsize=16)
    handlers = set()
    socket_directory = Path(tempfile.mkdtemp(prefix=".forge-product-", dir=Path("/tmp").resolve()))
    socket_path = socket_directory / "owner.sock"
    record = None
    server = None
    original_task = asyncio.current_task()
    loop = asyncio.get_running_loop()
    signal_installed = False

    async def receive(reader, writer):
        handler = asyncio.current_task()
        handlers.add(handler)
        queued = None
        try:
            _, uid, _ = _control_peer(writer.get_extra_info("socket"))
            _require(uid == os.getuid(), "The product request came from another local user.")
            async with asyncio.timeout(5):
                size = struct.unpack("!I", await reader.readexactly(4))[0]
                _require(0 < size <= ops["MAX_PRIVATE"], "The product request exceeds its bound.")
                request = _decode(await reader.readexactly(size))
            queued = loop.create_future()
            queue.put_nowait((request, queued))
            # A lost SSH reply cannot cancel the admitted canonical operation.
            answer = await asyncio.shield(queued)
            payload = _json(answer)
            _require(len(payload) <= ops["MAX_RESULT"], "The product response exceeds its bound.")
            writer.write(struct.pack("!I", len(payload)) + payload)
            async with asyncio.timeout(5):
                await writer.drain()
        except (OSError, ValueError, asyncio.IncompleteReadError, asyncio.QueueFull, TimeoutError):
            pass
        finally:
            writer.close()
            try:
                async with asyncio.timeout(5):
                    await writer.wait_closed()
            except (OSError, asyncio.CancelledError, TimeoutError):
                pass
            handlers.discard(handler)

    try:
        current()
        server = await asyncio.start_unix_server(receive, path=str(socket_path), limit=ops["MAX_PRIVATE"] + 4)
        socket_path.chmod(0o600)
        job = ops["_job"](root)
        record = {"protocol": 1, "install_operation_id": operation_id, "binding": deepcopy(owner["binding"]),
            "control_manifest_sha256": hashlib.sha256(_json(manifest)).hexdigest(),
            "initial_install_sha256": hashlib.sha256(_read(ops["_directory"](root) / "product-install-completed.json")).hexdigest(),
            "worker": deepcopy(job["worker"]), "command": deepcopy(job["command"]),
            "socket": {"path": str(socket_path), "identity": _socket_identity(socket_path)}}
        current()
        # Publish genuine completion before a controller can discover the socket
        # and hold its remote lock while waiting for this original owner Task.
        # These live processes still own control; whole-process cleanup is false.
        ops["_progress"](root, owner, operation_id, "complete", status="completed", completed_at=__import__("time").time(),
            cleanup_confirmed=False)
        current()
        ops["_json_write"](ops["_directory"](root) / "product-control-owner.json", record, exclusive=True)
        loop.add_signal_handler(signal.SIGTERM, original_task.cancel)
        signal_installed = True
        while True:
            envelope, waiter = await queue.get()
            try:
                answer = await _dispatch_control_request(service, inputs, context, root, owner,
                    operation_id, record, envelope, current, ops=ops)
            except asyncio.CancelledError:
                raise
            except (ValueError, RuntimeError, LookupError, OSError):
                answer = {"ready": False, "error": {"code": "recovery-needed", "message": "The original product operation needs an ownership check."},
                    "product_control": {"binding": _public_control_binding(record)}}
            finally:
                queue.task_done()
            if not waiter.done():
                waiter.set_result(answer)
    finally:
        if signal_installed:
            loop.remove_signal_handler(signal.SIGTERM)
        if server is not None:
            server.close()
            await server.wait_closed()
        for handler in tuple(handlers):
            handler.cancel()
        await asyncio.gather(*handlers, return_exceptions=True)
        # Shutdown stays on the original supplier task and drains actual handles.
        await service.shutdown()
        if inputs.factory._pending_scope is not None:
            inputs.factory._settled()
            await inputs.factory._release_pending()
        _require(not service.tasks and not service.processes and not service._application_preparations,
            "The original product owner has not confirmed application cleanup.")
        if record is not None and socket_path.exists() and _socket_identity(socket_path) != record["socket"]["identity"]:
            raise ValueError("The original control socket changed during shutdown; preserve its custody.")
        socket_path.unlink(missing_ok=True)
        socket_directory.rmdir()
