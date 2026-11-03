"""Package and verify the public standalone Forge distribution."""

from __future__ import annotations

import argparse
import ast
from email.parser import BytesParser
import hashlib
import io
import json
import os
from pathlib import Path, PurePosixPath
import re
import stat
import tempfile
import tomllib
import zipfile

MANIFEST = "standalone-manifest.json"
SDK_WHEEL = "vendor/meerkat_mobkit-0.8.46-py3-none-any.whl"
LOCAL_SDK_WHEEL = "vendor/meerkat_mobkit-0.8.42-py3-none-any.whl"
SDK_WHEEL_047 = "vendor/meerkat_mobkit-0.8.47-py3-none-any.whl"
SDK_WHEELS = {"0.8.42": LOCAL_SDK_WHEEL, "0.8.46": SDK_WHEEL, "0.8.47": SDK_WHEEL_047}
PUBLISHED_SDK = {
    "registry": "https://pypi.org/project/meerkat-mobkit/0.8.46/",
    "sha256": "e5c1c4cef9a19e945a723c5f9c9a55ff8f457d4c0e76c3ce93094b2646350cc2",
    "repository": "https://github.com/lukacf/meerkat-mobkit",
    "revision": "592c1297e3d4fc5b42db823023c1453cb0daa9f4",
    "release": "https://github.com/lukacf/meerkat-mobkit/releases/tag/v0.8.46",
}
PUBLISHED_NATIVE = {"version": "0.8.46", "core_version": "0.8.51",
    "release": "https://github.com/lukacf/meerkat-mobkit/releases/tag/v0.8.46"}
PUBLISHED_SDK_047 = {
    "registry": "https://pypi.org/project/meerkat-mobkit/0.8.47/",
    "sha256": "bca838cc3b956949cb13cbe64c91ce5c07738a38385bc791b325ec0a3bdcb85c",
    "repository": "https://github.com/lukacf/meerkat-mobkit",
    "revision": "324123936d5a98a14127fbb0c57d56f6a16f71a0",
    "release": "https://github.com/lukacf/meerkat-mobkit/releases/tag/v0.8.47",
}
PUBLISHED_NATIVE_047 = {"version": "0.8.47", "core_version": "0.8.51",
    "release": "https://github.com/lukacf/meerkat-mobkit/releases/tag/v0.8.47"}


def _published(version):
    if version == "0.8.46":
        return PUBLISHED_SDK, PUBLISHED_NATIVE
    if version == "0.8.47":
        return PUBLISHED_SDK_047, PUBLISHED_NATIVE_047
    raise DistributionError("The published runtime version is unsupported.")


TOP_LEVEL = {"start.sh", "README.md", "GETTING_STARTED.md", "FRONTEND_INTEGRATION.md",
    "pyproject.toml", "requirements-standalone.txt"}
REQUIRED = (TOP_LEVEL - {"GETTING_STARTED.md", "FRONTEND_INTEGRATION.md"}) | {"launchkit/distribution.py", "launchkit/server.py",
    "launchkit/bootstrap.py", "launchkit/generator.py",
    "web/freeform-animation.mjs", "web/dist/index.html", "web/dist/app.js", "web/dist/app.css"}
HEX = re.compile(r"[a-f0-9]{64}")
MAX_FILE = 32 * 1024 * 1024
MAX_FILES = 4096
MAX_TOTAL = 64 * 1024 * 1024


class DistributionError(ValueError):
    pass


def _public_path(name):
    path = PurePosixPath(name)
    if path.parts[:2] == ("checker-dependencies", "node_modules"):
        return path.as_posix() == name and not any(part in ("", ".", "..", "__pycache__", ".env", ".studio", ".tools", ".venv")
            for part in path.parts)
    if path.is_absolute() or path.as_posix() != name or any(
            part in ("", ".", "..", "__pycache__", "node_modules") or part.startswith(".") for part in path.parts):
        return False
    return name in TOP_LEVEL | set(SDK_WHEELS.values()) | {"web/freeform-animation.mjs"} or (
        len(path.parts) > 1 and (path.parts[0] in ("launchkit", "assets") or path.parts[:2] == ("web", "dist"))
        and path.suffix not in (".pyc", ".pyo"))


def _read(path):
    info = path.lstat()
    if not stat.S_ISREG(info.st_mode) or not 0 <= info.st_size <= MAX_FILE:
        raise DistributionError("Distribution inputs must be bounded ordinary files.")
    for parent in path.parents:
        if parent.is_symlink():
            raise DistributionError("Distribution inputs cannot contain symbolic links.")
    return path.read_bytes()


def _digest(content):
    return hashlib.sha256(content).hexdigest()


def _frontend_inputs(root):
    """Require the prebuilt assets to contain the current public source inputs."""
    inputs = {}
    repo = root.parent.resolve()
    for name in ("app.js.map", "app.css.map"):
        source_map = root / "web/dist" / name
        data = json.loads(_read(source_map))
        sources, contents = data.get("sources"), data.get("sourcesContent")
        if not isinstance(sources, list) or not sources or not isinstance(contents, list) or len(sources) != len(contents):
            raise DistributionError("The prebuilt frontend needs complete source maps.")
        for name, content in zip(sources, contents):
            if not isinstance(name, str) or not isinstance(content, str):
                raise DistributionError("The prebuilt frontend source map is invalid.")
            path = source_map.parent / name
            resolved = path.resolve()
            if not resolved.is_relative_to(repo):
                raise DistributionError("The prebuilt frontend is stale. Build it from the current checkout first.")
            located = resolved.relative_to(repo)
            # Installed packages are not public source inputs: package-lock.json
            # pins them, and their own source maps can name files the packages
            # do not ship (inline-style-parser's index.js, the MCP SDK's
            # TypeScript sources), which the bundler carries into app.js.map.
            # Where a name resolves in the checkout decides, not its spelling.
            if "node_modules" in located.parts:
                continue
            if not path.is_file() or _read(path) != content.encode():
                raise DistributionError("The prebuilt frontend is stale. Build it from the current checkout first.")
            inputs[located.as_posix()] = _digest(content.encode())
    if _read(root / "web/index.html") != _read(root / "web/dist/index.html"):
        raise DistributionError("The prebuilt frontend index is stale.")
    if _read(root / "assets/meerkat-transparent.png") != _read(root / "web/dist/assets/meerkat-transparent.png"):
        raise DistributionError("The prebuilt frontend asset is stale.")
    return inputs


def _sdk_files(content, version):
    with zipfile.ZipFile(io.BytesIO(content)) as archive:
        names = archive.namelist()
        prefix = f"meerkat_mobkit-{version}.dist-info/"
        metadata_names = {prefix + name for name in ("METADATA", "WHEEL", "RECORD", "top_level.txt")}
        if len(names) != len(set(names)) or len(names) > MAX_FILES or any(
                (name not in metadata_names and not name.startswith("meerkat_mobkit/")
                    and not (version in {"0.8.46", "0.8.47"} and name.startswith(prefix)))
                or PurePosixPath(name).is_absolute() or PurePosixPath(name).as_posix() != name
                or any(part in (".", "..", "") for part in PurePosixPath(name).parts)
                for name in names):
            raise DistributionError("The SDK wheel has unexpected files.")
        if sum(info.file_size for info in archive.infolist()) > MAX_TOTAL or any(info.file_size > MAX_FILE for info in archive.infolist()):
            raise DistributionError("The SDK wheel exceeds its bounds.")
        actual = {name: archive.read(name) for name in names if name.startswith("meerkat_mobkit/")}
        if prefix + "METADATA" not in names:
            raise DistributionError("The SDK wheel has an unexpected package identity.")
        metadata = BytesParser().parsebytes(archive.read(prefix + "METADATA"))
        if metadata.get_all("Name") != ["meerkat-mobkit"] or metadata.get_all("Version") != [version]:
            raise DistributionError("The SDK wheel has an unexpected package identity.")
    if b"def gateway_init_timeout(" not in actual.get("meerkat_mobkit/builder.py", b""):
        raise DistributionError("The bundled SDK must support gateway_init_timeout.")
    return actual


def _checked_sdk(content, root):
    actual = _sdk_files(content, "0.8.42")
    expected = {path.relative_to(root).as_posix(): _read(path) for path in root.rglob("*")
        if path.is_file() and "__pycache__" not in path.parts and path.suffix != ".pyc"}
    if actual != {"meerkat_mobkit/" + name: value for name, value in expected.items()}:
        raise DistributionError("The SDK wheel must match the current repository SDK source exactly.")
    return {name: _digest(content) for name, content in sorted(actual.items())}


def _checked_published_sdk(content, version="0.8.46"):
    if _digest(content) != _published(version)[0]["sha256"]:
        raise DistributionError("The published SDK wheel does not match its fixed official checksum.")
    _sdk_files(content, version)


def _checked_lock(content, project, version):
    pins = {}
    for line in content.decode().splitlines():
        if not line.strip() or line.startswith("#"):
            continue
        match = re.fullmatch(r"([a-z0-9][a-z0-9-]*)==([0-9][a-zA-Z0-9.+-]*)", line)
        if not match or match[1] in pins or match[1] == "meerkat-mobkit":
            raise DistributionError("Standalone dependencies must use exact package pins; the SDK comes from the bundled wheel.")
        pins[match[1]] = match[2]
    for requirement in project["project"]["dependencies"]:
        name = re.split(r"[<>=\[ ;]", requirement)[0].replace("_", "-").lower()
        if name == "meerkat-mobkit" and requirement != f"meerkat-mobkit=={version}":
            raise DistributionError("The project SDK requirement must match the bundled wheel.")
        if name != "meerkat-mobkit" and name not in pins:
            raise DistributionError("The standalone dependency lock is missing a runtime dependency.")


def _constant(content, name):
    try:
        values = [node.value for node in ast.parse(content).body if isinstance(node, ast.Assign)
            and any(isinstance(target, ast.Name) and target.id == name for target in node.targets)]
        if len(values) != 1 or not isinstance(values[0], ast.Constant) or not isinstance(values[0].value, str):
            raise ValueError
        return values[0].value
    except (SyntaxError, ValueError):
        raise DistributionError("The standalone runtime version binding is invalid.") from None


def _checked_versions(files, version):
    project = tomllib.loads(files["pyproject.toml"].decode())
    requirements = project["project"]["dependencies"]
    if not isinstance(requirements, list) or requirements.count(f"meerkat-mobkit=={version}") != 1:
        raise DistributionError("The project SDK requirement must match the bundled wheel.")
    _checked_lock(files["requirements-standalone.txt"], project, version)
    for path, name in (("launchkit/bootstrap.py", "VERSION"), ("launchkit/generator.py", "MOBKIT_VERSION"),
            ("launchkit/server.py", "VERSION")):
        if _constant(files[path], name) != version:
            raise DistributionError("The SDK and native runtime versions must match the bundled wheel.")
    if version in {"0.8.46", "0.8.47"} and _constant(files["launchkit/bootstrap.py"], "CORE_VERSION") != _published(version)[1]["core_version"]:
        raise DistributionError("The native runtime must match the qualified Core version.")


def _project_version(content):
    project = tomllib.loads(content.decode())
    values = [item.removeprefix("meerkat-mobkit==") for item in project["project"]["dependencies"]
        if isinstance(item, str) and item.startswith("meerkat-mobkit==")]
    if len(values) != 1 or values[0] not in SDK_WHEELS:
        raise DistributionError("The standalone SDK version is unsupported.")
    return values[0]


def package(root, wheel, output):
    root, wheel, output = Path(root).absolute(), Path(wheel).absolute(), Path(output).absolute()
    if output.exists():
        raise DistributionError("Choose a new distribution output path.")
    files = {name: _read(root / name) for name in TOP_LEVEL}
    for folder in ("launchkit", "assets", "web/dist"):
        for path in sorted((root / folder).rglob("*")):
            if path.is_symlink():
                raise DistributionError("Distribution inputs cannot contain symbolic links.")
            name = path.relative_to(root).as_posix()
            if path.is_file() and _public_path(name):
                files[name] = _read(path)
    files["web/freeform-animation.mjs"] = _read(root / "web/freeform-animation.mjs")
    version = _project_version(files["pyproject.toml"])
    wheel_name = SDK_WHEELS[version]
    files[wheel_name] = _read(wheel)
    if version in {"0.8.46", "0.8.47"}:
        _checked_published_sdk(files[wheel_name], version)
        sdk = {"version": version, "published": True, "wheel": wheel_name, "provenance": dict(_published(version)[0])}
    else:
        sdk = {"version": version, "published": False, "wheel": wheel_name,
            "source_sha256": _checked_sdk(files[wheel_name], root.parent / "sdk/python/meerkat_mobkit")}
    # Authenticate the existing fixed DOM package closure before selecting any
    # dependency file. No package manager, JavaScript or generated code runs.
    from .dom_check import prepare_dependencies
    dom_modes = {}
    with tempfile.TemporaryDirectory(prefix="mobkit-launch-distribution-") as temporary:
        dependencies, dom_hash = prepare_dependencies(Path(temporary).resolve(), source=root.parent / "console")
        for path in sorted(dependencies.rglob("*")):
            if path.is_file():
                relative = path.relative_to(dependencies).as_posix()
                name = "checker-dependencies/" + relative
                content = _read(path)
                original = root.parent / "console" / relative
                mode = original.lstat().st_mode & 0o777
                if mode not in (0o600, 0o700) or _read(original) != content:
                    raise DistributionError("The authenticated DOM dependencies changed during packaging.")
                files[name] = content
                dom_modes[name] = mode
    if not REQUIRED <= files.keys():
        raise DistributionError("The standalone distribution is missing required public files.")
    if len(files) > MAX_FILES or sum(map(len, files.values())) > MAX_TOTAL:
        raise DistributionError("The standalone distribution exceeds its bounds.")
    _checked_versions(files, version)
    manifest = {"format": "mobkit-launch-standalone-v1", "sdk": sdk,
        "frontend_source_sha256": _frontend_inputs(root), "dom_dependencies_sha256": dom_hash,
        "dom_file_modes": dom_modes,
        "files": {name: _digest(content) for name, content in sorted(files.items())}}
    if version in {"0.8.46", "0.8.47"}:
        manifest["native"] = dict(_published(version)[1])
    files[MANIFEST] = (json.dumps(manifest, sort_keys=True, indent=2) + "\n").encode()
    output.parent.mkdir(parents=True, exist_ok=True)
    with output.open("xb") as handle, zipfile.ZipFile(handle, "w", compression=zipfile.ZIP_DEFLATED) as archive:
        for name, content in sorted(files.items()):
            info = zipfile.ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
            info.compress_type = zipfile.ZIP_DEFLATED
            info.external_attr = (stat.S_IFREG | dom_modes.get(name, 0o755 if name == "start.sh" else 0o644)) << 16
            archive.writestr(info, content)
    return {"archive": str(output), "sha256": _digest(output.read_bytes()), "files": len(files),
        "sdk_wheel_sha256": manifest["files"][wheel_name], "sdk_version": version, "native_executed": False}


def _unique_object(pairs):
    result = {}
    for name, value in pairs:
        if name in result:
            raise DistributionError("The standalone manifest contains duplicate keys.")
        result[name] = value
    return result


def verify(root):
    root = Path(root).absolute()
    try:
        data = json.loads(_read(root / MANIFEST), object_pairs_hook=_unique_object)
    except json.JSONDecodeError:
        raise DistributionError("The standalone manifest is invalid.") from None
    if isinstance(data, dict) and data.get("format") == "meta-forge-product-standalone-v1":
        checked = verify_product(root)
        return checked["product"]["paths"]["meta_wheel"]
    if not isinstance(data, dict) or data.get("format") != "mobkit-launch-standalone-v1" or not isinstance(data.get("files"), dict):
        raise DistributionError("The standalone manifest is invalid.")
    sdk = data.get("sdk")
    if not isinstance(sdk, dict) or not isinstance(sdk.get("version"), str) or sdk["version"] not in SDK_WHEELS:
        raise DistributionError("The standalone SDK binding is invalid.")
    version, wheel_name = sdk["version"], SDK_WHEELS[sdk["version"]]
    if version in {"0.8.46", "0.8.47"}:
        provenance, native = _published(version)
        expected = {"version": version, "published": True, "wheel": wheel_name, "provenance": provenance}
        if sdk != expected or sdk.get("published") is not True or data.get("native") != native:
            raise DistributionError("The published SDK and native provenance binding is invalid.")
    elif (set(sdk) != {"version", "published", "wheel", "source_sha256"} or sdk.get("published") is not False
            or sdk.get("wheel") != wheel_name or not isinstance(sdk.get("source_sha256"), dict)):
        raise DistributionError("The standalone SDK binding is invalid.")
    if (len(data["files"]) > MAX_FILES or not (REQUIRED | {wheel_name}) <= data["files"].keys()
            or any(name in data["files"] for name in set(SDK_WHEELS.values()) - {wheel_name})):
        raise DistributionError("The standalone manifest is missing required public files.")
    dom_modes = data.get("dom_file_modes")
    dom_names = {name for name in data["files"] if name.startswith("checker-dependencies/")}
    if not dom_names or not isinstance(dom_modes, dict) or dom_modes.keys() != dom_names or any(
            type(mode) is not int or mode not in (0o600, 0o700) for mode in dom_modes.values()):
        raise DistributionError("The standalone DOM dependency modes are invalid.")
    total = 0
    for name, digest in data["files"].items():
        if not isinstance(name, str) or not _public_path(name) or not isinstance(digest, str) or not HEX.fullmatch(digest):
            raise DistributionError("The standalone manifest contains an invalid public file.")
        content = _read(root / name)
        total += len(content)
        if total > MAX_TOTAL:
            raise DistributionError("The standalone distribution exceeds its bounds.")
        if _digest(content) != digest:
            raise DistributionError("Standalone files changed. Unpack a fresh copy of the distribution.")
    content = _read(root / wheel_name)
    if version in {"0.8.46", "0.8.47"}:
        _checked_published_sdk(content, version)
    elif sdk["source_sha256"] != {name: _digest(value) for name, value in _sdk_files(content, version).items()}:
        raise DistributionError("The local SDK source binding does not match its bundled wheel.")
    _checked_versions({name: _read(root / name) for name in (
        "pyproject.toml", "requirements-standalone.txt", "launchkit/bootstrap.py", "launchkit/generator.py", "launchkit/server.py")}, version)
    # ZIP extraction does not consistently restore Unix modes. Restore only
    # authenticated public checker files; the existing DOM manifest then checks
    # their bytes AND original modes before any JavaScript can run.
    for name, mode in dom_modes.items():
        descriptor = os.open(root / name, os.O_RDONLY | os.O_NOFOLLOW)
        try:
            info = os.fstat(descriptor)
            if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_nlink != 1:
                raise DistributionError("Standalone checker files must be ordinary owned files.")
            with os.fdopen(os.dup(descriptor), "rb") as handle:
                if _digest(handle.read(MAX_FILE + 1)) != data["files"][name]:
                    raise DistributionError("Standalone checker files changed during verification.")
            if info.st_mode & 0o777 != mode:
                os.fchmod(descriptor, mode)
        finally:
            os.close(descriptor)
    return wheel_name


def verify_product(root):
    """Verify a colocated Meta/Connect candidate through its original target."""
    from .product_distribution import verify as verify_selected_product

    return verify_selected_product(root)


def sdk_wheels(root):
    """Return the host's SDK paths from the byte-verified standalone selection.

    A product retains its application SDK separately for the app environment.
    """
    root = Path(root).absolute()
    data = json.loads(_read(root / MANIFEST), object_pairs_hook=_unique_object)
    if isinstance(data, dict) and data.get("format") == "meta-forge-product-standalone-v1":
        checked = verify_product(root)
        return [checked["guide"]["sdk"]["path"], checked["product"]["paths"]["toolkit_wheel"]]
    return [verify(root)]


def runtime_kind(root):
    root = Path(root).absolute()
    data = json.loads(_read(root / MANIFEST), object_pairs_hook=_unique_object)
    if isinstance(data, dict) and data.get("format") == "meta-forge-product-standalone-v1":
        verify_product(root)
        return "meta-toolkit"
    verify(root)
    return "legacy"


def bootstrap_inputs(root):
    """Verify once and return the ordinary launcher's closed runtime inputs."""
    root = Path(root).absolute()
    data = json.loads(_read(root / MANIFEST), object_pairs_hook=_unique_object)
    if isinstance(data, dict) and data.get("format") == "meta-forge-product-standalone-v1":
        checked = verify_product(root)
        return ["meta-toolkit", checked["guide"]["sdk"]["path"],
            checked["product"]["paths"]["toolkit_wheel"]]
    return ["legacy", verify(root)]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--verify", type=Path, help="Verify an unpacked standalone distribution without starting it")
    parser.add_argument("--sdk-wheels", type=Path, help="Print verified bundled SDK wheel paths, one per line")
    parser.add_argument("--runtime-kind", type=Path, help="Print the verified distribution runtime family")
    parser.add_argument("--bootstrap-inputs", type=Path, help="Print verified launch inputs, one per line")
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument("--sdk-wheel", type=Path, help="Fixed official 0.8.46/0.8.47 wheel, or exact local-source 0.8.42 wheel for legacy source")
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    try:
        if args.verify:
            print(verify(args.verify))
        elif args.bootstrap_inputs:
            print("\n".join(bootstrap_inputs(args.bootstrap_inputs)))
        elif args.sdk_wheels:
            print("\n".join(sdk_wheels(args.sdk_wheels)))
        elif args.runtime_kind:
            print(runtime_kind(args.runtime_kind))
        elif args.sdk_wheel and args.output:
            print(json.dumps(package(args.root, args.sdk_wheel, args.output), sort_keys=True))
        else:
            parser.error("Supply --verify, or both --sdk-wheel and --output.")
    except (DistributionError, OSError, ValueError, KeyError, zipfile.BadZipFile):
        parser.exit(1, "Standalone preparation could not verify its public package inputs. Unpack a fresh bundle or rebuild from the complete source checkout.\n")


if __name__ == "__main__":
    main()
