"""Package a selected Meta/Connect product without substituting native bytes."""
from __future__ import annotations

from dataclasses import replace
import hashlib
import json
import os
from pathlib import Path
import re
import stat
import tempfile
import zipfile

from . import distribution as legacy
from . import guide_sdk
from .packaged_runtime import FORMAT, relocate_selection
from .runtime_candidates import (ArtifactFile, MetaToolkitRuntimeTarget,
    MAX_PRODUCT_ARCHIVE_BYTES)
from .runtime_selection import candidate_selection, selected_runtime_artifacts


PUBLIC_EXTRA = {"LICENSE-MIT", "LICENSE-APACHE", "THIRD_PARTY_NOTICES.md"}
# The application's ordinary Meta RPC gateway, paired with the selected Meta SDK.
ORDINARY_GATEWAY = "vendor/ordinary/rpc_gateway"
MAX_TOTAL = 4 * 1024 * 1024 * 1024
MAX_FILES = 8192


def candidate_readme(guide, target, origin, *, ordinary=False, glibc_floor=None):
    # A Linux product's glibc floor is the one its resource build declares.
    linux = "Linux x86_64" + (f" with glibc {glibc_floor[0]}.{glibc_floor[1]} or newer" if glibc_floor else "")
    host = (linux if target.target == "linux-x86_64"
        else "Apple Silicon macOS 13.5 or newer")
    ordinary_note = (f"\nApplications without a connection setup helper use the included Meta "
        f"{target.meta.version} RPC gateway with that Meta SDK.\n" if ordinary else
        "\nThis archive does not include the ordinary Meta RPC gateway. New applications\n"
        "without a connection setup helper need a package containing that gateway for\n"
        "the selected application Meta SDK. Ask the package owner for that package\n"
        "before building.\n")
    # Applications install into Forge's own Python: name the one CPython the
    # selected compiled (non-abi3) dependency wheels are built for.
    cpythons = sorted({int(tag[3:]) for wheel in target.dependency_wheels
        for py_tags, abi_tags, _ in [Path(wheel.path).name.removesuffix(".whl").split("-")[-3:]]
        if "abi3" not in abi_tags.split(".") for tag in py_tags.split(".") if re.fullmatch(r"cp3[0-9]+", tag)})
    python = f"CPython 3.{cpythons[0]}" if len(cpythons) == 1 else "Python 3.11+"
    python_note = (f"\nApplications install into Forge's own Python. If your `python3` is another version,\n"
        f"start Forge with `PYTHON_BIN` set to a CPython 3.{cpythons[0]} interpreter; GETTING_STARTED.md\n"
        "shows how.\n" if len(cpythons) == 1 else "")
    return f"""# Meta Forge local candidate

The Forge guide uses Core {guide.core_version} and Meta {guide.version}. The
application uses the separately selected Meta {target.meta.version} and Toolkit
{target.toolkit.version} SDK pair. Its native product origin is `{origin}`.{ordinary_note}
Artifact integrity and source checks do not establish that a
particular installed application has completed its useful task.

Supported package destination: **{host}**. Other platforms and cloud deployment
are not qualified by this package.

## Open Forge

You need a complete {python} installation with `venv` and `ensurepip`.
Initial installation of the exactly pinned Forge Python dependencies needs
internet access. The package supplies its exact Meta and Toolkit SDK wheels,
native guide gateway and selected Connect product. Node.js and a source checkout
are not needed to open the wizard.
{python_note}
Extract the archive into a new folder you will keep. Open a terminal there:

```sh
bash start.sh
```

Follow the printed local address. Keep the terminal open while using Forge.
Press Ctrl+C to stop it. Your conversation and installed applications use their
own retained state; keep their folders when returning or restarting.

For dependency and artifact preparation without starting Forge or native agents:

```sh
bash start.sh --prepare-only
```

The normal entry selects the supplied trusted runtime. You do not need to set
a factory environment variable or assemble SDK paths. Source-only trees lack
those package inputs and stop with preparation guidance.

## Create and use an application

Describe the useful job, answer the guide's choices, and review the evolving
agent design and animated proposal. Build the application, review its checks,
then use its offered installation and protected settings controls. Enter API
keys or Console credentials only in their private forms.

Complete one real task in the installed application. Verify its result, correct
an actual requirement, then restart the same installation and check its history.
Build acceptance, installation, capability setup and useful execution are
separate outcomes. The package does not invent missing accounts or permissions.

Application checks may require Forge's private reviewed Node checker and the
host isolation prerequisites described in GETTING_STARTED.md. Linux checks
need working unprivileged user namespaces, bubblewrap, libseccomp, libatomic and
libstdc++ support. The wizard offers its normal preparation control before a
build. A missing prerequisite keeps checks unavailable.

See GETTING_STARTED.md for the complete guided journey and recovery controls.
Project licenses and third-party notices are included. Private settings,
credentials, saved conversations and coordination evidence are not application
configuration files to upload or publish.

## Exact selected source

- Guide Core: `{guide.core_source}`
- Guide Meta: `{guide.meta_source}`
- Guide client SDK: `{guide_sdk.SELECTED_VERSION}` (source `{guide_sdk.SOURCE}`)
- Guide client SDK wheel: `{guide_sdk.SHA256}`
- Application Meta SDK wheel: `{target.meta_wheel.sha256}`
- Toolkit SDK wheel: `{target.toolkit_wheel.sha256}`
- Native product descriptor: `{target.assembly.sha256}`

The archive's standalone-manifest.json pins its actual source/assets, SDKs,
native product, checker dependencies and guide gateway. A development descriptor
retains actual staging/build/resource/catalog bytes without claiming a final
installed verification report. Consult the companion journey evidence for the
tested outcome on these exact bytes.
"""


def _ordinary(path, limit, *, allow_empty=False):
    path = Path(path)
    if any(parent.is_symlink() for parent in (path, *path.parents)):
        raise legacy.DistributionError("Product distribution inputs cannot contain links.")
    before = path.lstat()
    if (not stat.S_ISREG(before.st_mode) or before.st_uid != os.getuid()
            or before.st_nlink != 1 or not (0 if allow_empty else 1) <= before.st_size <= limit):
        raise legacy.DistributionError("Product distribution inputs must be owned, ordinary and bounded.")
    hashed, read_size = hashlib.sha256(), 0
    with path.open("rb") as handle:
        opened = os.fstat(handle.fileno())
        if (opened.st_dev, opened.st_ino) != (before.st_dev, before.st_ino):
            raise legacy.DistributionError("A product input changed before observation.")
        while chunk := handle.read(min(1024 * 1024, before.st_size - read_size + 1)):
            read_size += len(chunk)
            if read_size > before.st_size:
                raise legacy.DistributionError("A product input changed during observation.")
            hashed.update(chunk)
        if read_size != before.st_size:
            raise legacy.DistributionError("A product input changed during observation.")
        after = os.fstat(handle.fileno())
    fields = ("st_dev", "st_ino", "st_mode", "st_uid", "st_nlink", "st_size", "st_mtime_ns", "st_ctime_ns")
    observed = tuple(getattr(before, name) for name in fields)
    if observed != tuple(getattr(after, name) for name in fields) or observed != tuple(
            getattr(path.lstat(), name) for name in fields):
        raise legacy.DistributionError("A product input changed during observation.")
    return hashed.hexdigest(), before.st_size


def _public_path(name):
    if name in PUBLIC_EXTRA:
        return True
    if name.startswith("vendor/product/") or name in {"vendor/guide/rpc_gateway", ORDINARY_GATEWAY, guide_sdk.WHEEL}:
        from pathlib import PurePosixPath

        path = PurePosixPath(name)
        return (not path.is_absolute() and path.as_posix() == name
            and not any(part in {"", ".", ".."} or part.startswith(".") for part in path.parts))
    return legacy._public_path(name)


def _product_inputs(target, artifacts):
    selected = {pin.path: pin for pin in artifacts.evidence}
    selected[artifacts.archive.path] = artifacts.archive
    for name in ("assembly", "meta_wheel", "toolkit_wheel", "dependency_requirements",
            "model_catalog", "source_manifest"):
        pin = getattr(target, name)
        if pin is not None:
            selected[pin.path] = pin
    for pin in target.dependency_wheels:
        selected[pin.path] = pin
    return selected


def _pinned_guide_sdk():
    try:
        guide_sdk.require_pinned()
    except ValueError as exc:
        raise legacy.DistributionError(str(exc)) from exc


def package(root, target, guide, output, *, ordinary_gateway=None):
    """Select public source and the actual owner's complete product closure.

    The target and guide selection come from the original trusted packaging
    owner. The existing native reader still validates the producer records and
    exact wheel/source/dependency bytes. This is not installed useful-work proof.
    An optional ordinary gateway is the owner's Meta RPC gateway release for
    the target's Meta SDK version, for applications without a setup helper.
    """
    from .bootstrap import SelectedGuideRuntime
    from .connect_installer_caller import ExactFile
    from .dom_check import prepare_dependencies

    root, output = Path(root).resolve(strict=True), Path(output).absolute()
    if (type(target) is not MetaToolkitRuntimeTarget or type(guide) is not SelectedGuideRuntime
            or ordinary_gateway is not None and type(ordinary_gateway) is not ExactFile
            or output.exists() or output.is_symlink()):
        raise legacy.DistributionError("Select exact product inputs and a fresh archive output.")
    _pinned_guide_sdk()
    guide.recheck()
    artifacts = selected_runtime_artifacts(target)
    selection = candidate_selection(target, verified_artifacts=artifacts)
    # The source manifest is copied explicitly so historical producer locators
    # remain provenance rather than paths opened on an extracted consumer host.
    if target.source_manifest is None:
        build = json.loads((target.assembly.path.parent / "build-record.json").read_bytes())
        original = Path(build["source_manifest"]["path"])
        pin = ArtifactFile(original, original.stat().st_size, build["source_manifest"]["sha256"])
        target = replace(target, source_manifest=pin)
        artifacts = selected_runtime_artifacts(target)
        selection = candidate_selection(target, verified_artifacts=artifacts)
    inputs = _product_inputs(target, artifacts)
    source_files = {}
    for name in legacy.TOP_LEVEL | PUBLIC_EXTRA:
        path = root / name
        if name in PUBLIC_EXTRA and not path.exists():
            if name != "THIRD_PARTY_NOTICES.md":
                raise legacy.DistributionError("The product distribution requires its project licenses.")
            continue
        source_files[name] = path
    for folder in ("launchkit", "assets", "web/dist"):
        for path in sorted((root / folder).rglob("*")):
            name = path.relative_to(root).as_posix()
            if path.is_symlink():
                raise legacy.DistributionError("Public distribution source cannot contain links.")
            if path.is_file() and legacy._public_path(name):
                source_files[name] = path
    source_files["web/freeform-animation.mjs"] = root / "web/freeform-animation.mjs"
    paths = {"artifact_root": "vendor/product", "dependency_wheels": []}
    for path, pin in inputs.items():
        name = "vendor/product/" + path.relative_to(target.artifact_root).as_posix()
        if not _public_path(name):
            raise legacy.DistributionError("A selected product input has an unsupported public locator.")
        hashed, size = _ordinary(path, MAX_PRODUCT_ARCHIVE_BYTES)
        if (hashed, size) != (pin.sha256, pin.size):
            raise legacy.DistributionError("A selected product input changed before packaging.")
        source_files[name] = path
    for name in ("assembly", "meta_wheel", "toolkit_wheel", "dependency_requirements",
            "model_catalog", "source_manifest"):
        paths[name] = "vendor/product/" + getattr(target, name).path.relative_to(target.artifact_root).as_posix()
    paths["dependency_wheels"] = ["vendor/product/" + pin.path.relative_to(target.artifact_root).as_posix()
        for pin in target.dependency_wheels]
    source_files["vendor/guide/rpc_gateway"] = guide.gateway.path
    source_files[guide_sdk.WHEEL] = root / guide_sdk.WHEEL
    if ordinary_gateway is not None:
        source_files[ORDINARY_GATEWAY] = ordinary_gateway.path
    with tempfile.TemporaryDirectory(prefix="forge-product-dom-") as temporary:
        candidate_document = Path(temporary).resolve(strict=True) / "README.md"
        candidate_document.write_text(candidate_readme(guide, target, artifacts.origin,
            ordinary=ordinary_gateway is not None, glibc_floor=artifacts.glibc_floor))
        candidate_document.chmod(0o600)
        source_files["README.md"] = candidate_document
        dependencies, dom_hash = prepare_dependencies(Path(temporary).resolve(), source=root.parent / "console")
        dom_files = {}
        for path in sorted(dependencies.rglob("*")):
            if path.is_file():
                name = "checker-dependencies/" + path.relative_to(dependencies).as_posix()
                source_files[name] = path
                original = root.parent / "console" / path.relative_to(dependencies)
                mode = original.stat().st_mode & 0o777
                if mode not in {0o600, 0o700} or original.read_bytes() != path.read_bytes():
                    raise legacy.DistributionError("The authenticated checker dependencies changed before packaging.")
                dom_files[name] = mode
        if not legacy.REQUIRED <= source_files.keys():
            raise legacy.DistributionError("The product distribution is missing public Forge files.")
        file_records, modes, total = {}, {}, 0
        for name, path in sorted(source_files.items()):
            if not _public_path(name):
                raise legacy.DistributionError("The product distribution contains an unsupported public path.")
            limit = MAX_PRODUCT_ARCHIVE_BYTES if name.startswith("vendor/") else legacy.MAX_FILE
            hashed, size = _ordinary(path, limit, allow_empty=not name.startswith("vendor/"))
            file_records[name] = {"sha256": hashed, "bytes": size}
            modes[name] = dom_files.get(name, 0o700 if name in {"vendor/guide/rpc_gateway", ORDINARY_GATEWAY} else
                0o755 if name == "start.sh" else 0o600 if name.startswith("vendor/product/") else 0o644)
            total += size
        if len(source_files) > MAX_FILES or total > MAX_TOTAL:
            raise legacy.DistributionError("The product distribution exceeds its bounds.")
        # Bind the bytes observed for this archive, not an earlier SDK read.
        # The application's wheel remains selected separately for its own env.
        if file_records[guide_sdk.WHEEL] != {"sha256": guide_sdk.SHA256, "bytes": guide_sdk.SIZE}:
            raise legacy.DistributionError("The Guide client SDK differs from its reviewed wheel.")
        guide_hash, guide_size = _ordinary(guide.gateway.path, MAX_PRODUCT_ARCHIVE_BYTES)
        ordinary = None
        if ordinary_gateway is not None:
            if file_records[ORDINARY_GATEWAY]["sha256"] != ordinary_gateway.sha256:
                raise legacy.DistributionError("The ordinary Meta gateway differs from its selected release bytes.")
            ordinary = {"version": target.meta.version,
                "gateway": {"path": ORDINARY_GATEWAY, **file_records[ORDINARY_GATEWAY]}}
        data = {"format": FORMAT, "product": {"selection": selection, "paths": paths},
            "guide": {"version": guide.version, "core_version": guide.core_version,
                "meta_source": guide.meta_source, "core_source": guide.core_source,
                "sdk": guide_sdk.selection(),
                "gateway": {"path": "vendor/guide/rpc_gateway", "sha256": guide_hash, "bytes": guide_size}},
            "frontend_source_sha256": legacy._frontend_inputs(root),
            "dom_dependencies_sha256": dom_hash, "files": file_records, "file_modes": modes,
            "support": {"native_target": target.target, "native_origin": artifacts.origin,
                "installed_use_accepted": False}}
        if ordinary is not None:
            data["ordinary"] = ordinary
        content = (json.dumps(data, sort_keys=True, indent=2, ensure_ascii=True) + "\n").encode()
        output.parent.mkdir(parents=True, exist_ok=True)
        with output.open("xb") as stream, zipfile.ZipFile(stream, "w", compression=zipfile.ZIP_DEFLATED) as archive:
            for name, path in sorted(source_files.items()):
                info = zipfile.ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
                info.compress_type = zipfile.ZIP_DEFLATED
                info.external_attr = (stat.S_IFREG | modes[name]) << 16
                copied_hash, copied_size = hashlib.sha256(), 0
                with archive.open(info, "w", force_zip64=True) as destination, path.open("rb") as source:
                    while chunk := source.read(1024 * 1024):
                        copied_size += len(chunk)
                        if copied_size > file_records[name]["bytes"]:
                            raise legacy.DistributionError("A product input changed while writing the archive.")
                        copied_hash.update(chunk)
                        destination.write(chunk)
                # Bind the actual ZIP payload even if a changed input is
                # restored before the separate source recheck below.
                if file_records[name] != {"sha256": copied_hash.hexdigest(), "bytes": copied_size}:
                    raise legacy.DistributionError("A product input changed while writing the archive.")
                limit = MAX_PRODUCT_ARCHIVE_BYTES if name.startswith("vendor/") else legacy.MAX_FILE
                hashed, size = _ordinary(path, limit, allow_empty=not name.startswith("vendor/"))
                if file_records[name] != {"sha256": hashed, "bytes": size}:
                    raise legacy.DistributionError("A product input changed while writing the archive.")
            info = zipfile.ZipInfo(legacy.MANIFEST, date_time=(1980, 1, 1, 0, 0, 0))
            info.compress_type = zipfile.ZIP_DEFLATED
            info.external_attr = (stat.S_IFREG | 0o644) << 16
            archive.writestr(info, content)
        guide.recheck()
        selected_runtime_artifacts(target)
    return {"archive": str(output), "sha256": _ordinary(output, MAX_TOTAL)[0],
        "files": len(source_files), "native_target": target.target,
        "meta_version": target.meta.version, "native_executed": False,
        "installed_use_accepted": False}


def verify(root):
    """Verify an extracted product without model or native process execution."""
    _pinned_guide_sdk()
    root = Path(root).resolve(strict=True)
    raw = legacy._read(root / legacy.MANIFEST)
    data = json.loads(raw, object_pairs_hook=legacy._unique_object)
    names = {"format", "product", "guide", "frontend_source_sha256", "dom_dependencies_sha256",
        "files", "file_modes", "support"}
    if (type(data) is not dict or data.keys() not in (names, names | {"ordinary"}) or data["format"] != FORMAT
            or type(data["files"]) is not dict or type(data["file_modes"]) is not dict
            or data["files"].keys() != data["file_modes"].keys()
            or not (legacy.REQUIRED | {"LICENSE-MIT", "LICENSE-APACHE", "vendor/guide/rpc_gateway"}) <= data["files"].keys()
            or not 0 < len(data["files"]) <= MAX_FILES):
        raise legacy.DistributionError("The extracted product distribution manifest is invalid.")
    product = data["product"]
    if (type(product) is not dict or not {"selection", "paths"} <= product.keys()
            or type(product["selection"]) is not dict or type(product["paths"]) is not dict):
        raise legacy.DistributionError("The product distribution needs its original selection and paths.")
    total = 0
    for name, record in data["files"].items():
        if (type(name) is not str or not _public_path(name) or type(record) is not dict
                or record.keys() != {"sha256", "bytes"} or type(record["bytes"]) is not int
                or record["bytes"] < 0 or type(record["sha256"]) is not str
                or not legacy.HEX.fullmatch(record["sha256"])
                or type(data["file_modes"][name]) is not int
                or data["file_modes"][name] not in {0o600, 0o644, 0o700, 0o755}):
            raise legacy.DistributionError("A product distribution file binding is invalid.")
        limit = MAX_PRODUCT_ARCHIVE_BYTES if name.startswith("vendor/") else legacy.MAX_FILE
        hashed, size = _ordinary(root / name, limit, allow_empty=not name.startswith("vendor/"))
        total += size
        if total > MAX_TOTAL or record != {"sha256": hashed, "bytes": size}:
            raise legacy.DistributionError("Product distribution files changed. Unpack a fresh copy.")
    # Restore only the already byte-verified payload modes. ZIP extraction does
    # not reliably preserve permissions required by exact SDK/native custody.
    for name, mode in data["file_modes"].items():
        path = root / name
        descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
        try:
            info = os.fstat(descriptor)
            record = data["files"][name]
            if (not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid()
                    or info.st_nlink != 1 or info.st_size != record["bytes"]):
                raise legacy.DistributionError("A product distribution file changed before mode restoration.")
            hashed, read_size = hashlib.sha256(), 0
            with os.fdopen(os.dup(descriptor), "rb") as handle:
                while chunk := handle.read(min(1024 * 1024, record["bytes"] - read_size + 1)):
                    read_size += len(chunk)
                    if read_size > record["bytes"]:
                        raise legacy.DistributionError("A product distribution file changed before mode restoration.")
                    hashed.update(chunk)
            if read_size != record["bytes"] or hashed.hexdigest() != record["sha256"]:
                raise legacy.DistributionError("A product distribution file changed before mode restoration.")
            if info.st_mode & 0o777 != mode:
                os.fchmod(descriptor, mode)
        finally:
            os.close(descriptor)
    ordinary = data.get("ordinary")
    if (ORDINARY_GATEWAY in data["files"]) != (ordinary is not None) or ordinary is not None and (
            type(ordinary) is not dict or ordinary.keys() != {"version", "gateway"}
            or data["file_modes"][ORDINARY_GATEWAY] != 0o700
            or ordinary["gateway"] != {"path": ORDINARY_GATEWAY, **data["files"][ORDINARY_GATEWAY]}):
        raise legacy.DistributionError("The ordinary Meta gateway differs from its selected bytes.")
    for parent in (root / "vendor/guide", root / "vendor/product",
            *((root / "vendor/ordinary",) if ordinary is not None else ())):
        if parent.is_symlink() or not parent.is_dir() or parent.stat().st_uid != os.getuid():
            raise legacy.DistributionError("The product runtime parent is not an owned ordinary directory.")
        parent.chmod(0o700)
    target = relocate_selection(product["selection"], product["paths"], root)
    selected_runtime_artifacts(target)
    if ordinary is not None and ordinary["version"] != target.meta.version:
        raise legacy.DistributionError("The ordinary Meta gateway is not the selected Meta SDK's release.")
    guide = data["guide"]
    if (type(guide) is not dict or guide.keys() != {"version", "core_version", "meta_source", "core_source", "gateway", "sdk"}
            or guide["version"] != guide_sdk.SELECTED_VERSION
            or guide["core_version"] != guide_sdk.SELECTED_VERSION
            or guide["sdk"] != guide_sdk.selection()
            or guide_sdk.WHEEL not in data["files"]
            or guide["gateway"] != {"path": "vendor/guide/rpc_gateway",
                **data["files"]["vendor/guide/rpc_gateway"]}
            or any(type(guide[name]) is not str or not re.fullmatch("[0-9a-f]{40}", guide[name])
                for name in ("meta_source", "core_source"))):
        raise legacy.DistributionError("The product guide differs from its selected native bytes.")
    try:
        guide_sdk.verify(legacy._read(root / guide_sdk.WHEEL))
    except ValueError as exc:
        raise legacy.DistributionError(str(exc)) from exc
    return data
