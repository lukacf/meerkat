"""Install the pinned local studio gateway from verified release archives."""

from __future__ import annotations

from copy import deepcopy
from dataclasses import dataclass, field
import hashlib
import io
import json
import os
from pathlib import Path
import platform
import re
import stat
import tarfile
import tempfile
import urllib.request

VERSION = "0.8.47"
CORE_VERSION = "0.8.51"
RELEASE_URL = f"https://github.com/lukacf/meerkat-mobkit/releases/download/v{VERSION}"
MAX_ARCHIVE_BYTES = 256 * 1024 * 1024
MAX_BINARY_BYTES = 512 * 1024 * 1024
HEX_DIGEST = re.compile(r"^[0-9a-f]{64}$")


@dataclass(frozen=True)
class SelectedGuideRuntime:
    """Trusted launcher selection for a fresh Meta guide generation.

    The caller accepts the actual candidate and source provenance before
    constructing this value. Byte checks preserve that selection; they do not
    establish native qualification, credentials or provider readiness.
    """

    gateway: object
    version: str
    core_version: str
    meta_source: str
    core_source: str
    _model_route: dict | None = field(default=None, repr=False)
    _model_custody: object = field(default=None, repr=False, compare=False)
    _route_bytes: bytes | None = field(default=None, init=False, repr=False)
    _gateway_pin: tuple | None = field(default=None, init=False, repr=False, compare=False)
    _gateway_observation: tuple | None = field(default=None, init=False, repr=False, compare=False)

    def __post_init__(self):
        from . import guide_sdk
        from .connect_installer_caller import ExactFile

        selected = guide_sdk.SELECTED_VERSION
        if (type(self.gateway) is not ExactFile
                or self.version != selected or self.core_version != selected
                or any(type(value) is not str or not re.fullmatch("[0-9a-f]{40}", value)
                    for value in (self.meta_source, self.core_source))):
            raise GatewayInstallError(f"The guide requires its exact selected Meta {selected} runtime.")
        object.__setattr__(self, "_gateway_pin", (self.gateway.path, self.gateway.sha256))
        route = deepcopy(self._model_route)
        if route is not None:
            allowed = {"provider", "model", "auth_binding", "meerkat_config_path"}
            if (type(route) is not dict or not {"provider", "model"} <= route.keys()
                    or not route.keys() <= allowed
                    or any(type(value) is not str or not value or len(value) > 4096
                        or "\x00" in value for value in route.values())):
                raise GatewayInstallError("The guide requires its original trusted model route.")
        object.__setattr__(self, "_model_route", route)
        object.__setattr__(self, "_route_bytes", json.dumps(route, sort_keys=True,
            ensure_ascii=True, allow_nan=False).encode())
        selected_config = route.get("meerkat_config_path") if route is not None else None
        if selected_config is not None:
            from .connect_application_startup import _ModelConfiguration

            if (type(self._model_custody) is not _ModelConfiguration
                    or str(self._model_custody.host_config.path) != selected_config):
                raise GatewayInstallError("The guide requires its original model config and credential custody.")
        elif self._model_custody is not None:
            raise GatewayInstallError("The guide model custody differs from its selected route.")
        self.recheck()

    @property
    def model_configuration(self):
        """Return selected public route fields, never native config contents."""
        self.recheck()
        return deepcopy(self._model_route)

    def _recheck_gateway(self, *, verify_bytes=False):
        """Reuse observed bytes only while this opened file stays unchanged.

        Ordinary capability/status reads still check canonical path, ownership,
        permissions and file identity. Initial selection, changed metadata and
        explicit native startup checks stream the full selected checksum.
        Nothing persists this process-local observation as qualification.
        """
        from .connect_installer_caller import CustodyError, ExactFile, MAX_EXECUTABLE_BYTES, _path

        if (type(self.gateway) is not ExactFile
                or (self.gateway.path, self.gateway.sha256) != self._gateway_pin):
            raise CustodyError("Preserve the selected guide gateway's original byte pin.")
        path = _path(self.gateway.path)
        descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
        try:
            before = os.fstat(descriptor)

            def identity(info):
                return (info.st_dev, info.st_ino, info.st_mode, info.st_uid,
                        info.st_nlink, info.st_size, info.st_mtime_ns, info.st_ctime_ns)

            if (not stat.S_ISREG(before.st_mode) or before.st_uid != os.getuid()
                    or before.st_nlink != 1 or before.st_mode & 0o077
                    or not before.st_mode & stat.S_IXUSR
                    or not 0 < before.st_size <= MAX_EXECUTABLE_BYTES):
                raise CustodyError("The selected guide gateway is not private, owned and executable.")
            observation = (str(path), self.gateway.sha256, identity(before))
            if verify_bytes or observation != self._gateway_observation:
                self.gateway.verify(executable=True)
            after, current = os.fstat(descriptor), path.lstat()
            if identity(before) != identity(after) or identity(after) != identity(current):
                raise CustodyError("The selected guide gateway changed during observation.")
            object.__setattr__(self, "_gateway_observation", observation)
        finally:
            os.close(descriptor)

    def recheck(self, *, verify_gateway=False):
        if json.dumps(self._model_route, sort_keys=True, ensure_ascii=True,
                allow_nan=False).encode() != self._route_bytes:
            raise GatewayInstallError("The selected guide model route changed.")
        self._recheck_gateway(verify_bytes=verify_gateway)
        if self._model_custody is not None:
            self._model_custody.require_environment()
            self._model_custody.require_filesystem_current()


_selected_guide_runtime = None


def configure_selected_guide_runtime(selected):
    """Select once from the original trusted launcher before server import."""
    global _selected_guide_runtime
    if type(selected) is not SelectedGuideRuntime:
        raise GatewayInstallError("Select the original trusted Meta guide runtime.")
    if _selected_guide_runtime is not None and selected is not _selected_guide_runtime:
        raise GatewayInstallError("The original guide runtime cannot be replaced during this process.")
    selected.recheck()
    _selected_guide_runtime = selected
    return selected


def selected_guide_runtime():
    selected = _selected_guide_runtime
    if selected is not None:
        selected.recheck()
    return selected


def selected_guide_gateway():
    selected = selected_guide_runtime()
    return selected.gateway.path if selected is not None else None


def export_selected_guide_context():
    """Capture private parent-owned worker control, never model-visible input."""
    selected = selected_guide_runtime()
    if selected is None:
        return None
    return {"gateway": selected.gateway.pin(), "version": selected.version,
        "core_version": selected.core_version, "meta_source": selected.meta_source,
        "core_source": selected.core_source,
        "model_configuration": selected.model_configuration,
        "model_custody": (selected._model_custody.selection()
            if selected._model_custody is not None else None)}


def restore_selected_guide_context(value):
    """Recheck an original parent's private worker selection before admission.

    The caller must obtain this value from its owned control file, never a
    browser request, model-visible input or an environment-selected route.
    """
    from .connect_installer_caller import ExactFile

    names = {"gateway", "version", "core_version", "meta_source", "core_source",
        "model_configuration", "model_custody"}
    if (type(value) is not dict or value.keys() != names
            or type(value["gateway"]) is not dict
            or value["gateway"].keys() != {"path", "sha256"}):
        raise GatewayInstallError("The guide worker needs its original private runtime selection.")
    pin = value["gateway"]
    if type(pin["path"]) is not str or type(pin["sha256"]) is not str:
        raise GatewayInstallError("The guide worker gateway selection is invalid.")
    custody = None
    if value["model_custody"] is not None:
        from .connect_application_startup import _ModelConfiguration

        custody = _ModelConfiguration.observe(value["model_custody"])
    selected = SelectedGuideRuntime(gateway=ExactFile(Path(pin["path"]), pin["sha256"]),
        version=value["version"], core_version=value["core_version"],
        meta_source=value["meta_source"], core_source=value["core_source"],
        _model_route=value["model_configuration"], _model_custody=custody)
    return configure_selected_guide_runtime(selected)


class GatewayInstallError(RuntimeError):
    """A gateway could not be verified or installed."""


def package_tool_path(package_root: Path, binary_name: str) -> Path:
    """Keep each package release beside retained tools, never over their bytes."""
    if binary_name not in {"rpc_gateway", "mobkit_gateway"}:
        raise GatewayInstallError("Choose an official MobKit runtime or maintenance binary.")
    return Path(package_root).expanduser().absolute() / ".tools" / "releases" / VERSION / ".tools" / binary_name


def platform_target() -> str:
    architecture = {"arm64": "aarch64", "aarch64": "aarch64", "x86_64": "x86_64", "AMD64": "x86_64"}.get(platform.machine())
    system = platform.system()
    suffix = {"Darwin": "apple-darwin", "Linux": "unknown-linux-gnu"}.get(system)
    if not architecture or not suffix:
        raise GatewayInstallError("The studio gateway supports macOS and glibc Linux on Intel or ARM64.")
    if system == "Linux":
        libc, version = platform.libc_ver()
        try:
            compatible = libc == "glibc" and tuple(int(p) for p in version.split(".")[:2]) >= (2, 31)
        except ValueError:
            compatible = False
        if not compatible:
            raise GatewayInstallError("The Linux gateway requires glibc 2.31 or newer. Alpine/musl is not supported.")
    return f"{architecture}-{suffix}"


def _download(url: str, limit: int) -> bytes:
    request = urllib.request.Request(url, headers={"User-Agent": "Meta-Forge/1", "Accept": "application/octet-stream"})
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            if not response.geturl().startswith("https://"):
                raise GatewayInstallError("The gateway download redirected to an insecure URL.")
            chunks = []
            size = 0
            while chunk := response.read(min(1024 * 1024, limit + 1 - size)):
                size += len(chunk)
                if size > limit:
                    raise GatewayInstallError("The gateway download exceeded its size limit.")
                chunks.append(chunk)
            return b"".join(chunks)
    except OSError as error:
        raise GatewayInstallError("Could not download the official gateway release. Check network access and try again.") from error


def _file_digest(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        while chunk := handle.read(1024 * 1024):
            digest.update(chunk)
    return digest.hexdigest()


def _verified_cache(destination: Path, receipt: Path, archive_name: str) -> bool:
    if not destination.is_file() or destination.is_symlink() or not receipt.is_file() or receipt.is_symlink():
        return False
    try:
        if receipt.stat().st_mode & 0o077:
            return False
        data = json.loads(receipt.read_text())
        if (not isinstance(data, dict) or data.get("version") != VERSION or data.get("core_version") != CORE_VERSION
                or data.get("archive") != archive_name
                or data.get("release_url") != RELEASE_URL
                or not HEX_DIGEST.fullmatch(str(data.get("archive_sha256", "")))
                or not HEX_DIGEST.fullmatch(str(data.get("binary_sha256", "")))):
            return False
        return os.access(destination, os.X_OK) and _file_digest(destination) == data["binary_sha256"]
    except (OSError, ValueError, TypeError):
        return False


def _atomic_write(path: Path, content: bytes, mode: int) -> None:
    temporary = None
    try:
        with tempfile.NamedTemporaryFile(prefix=f".{path.name}-", dir=path.parent, delete=False) as handle:
            temporary = Path(handle.name)
            os.fchmod(handle.fileno(), mode)
            handle.write(content)
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temporary, path)
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)


def download_gateway(destination: Path, *, binary_name: str = "rpc_gateway") -> Path:
    """Install the pinned gateway locally, reusing only verified cached bytes."""
    destination = Path(destination).expanduser().absolute()
    target = platform_target()
    archive_prefix = {"rpc_gateway": "mobkit-rpc-gateway", "mobkit_gateway": "mobkit-gateway"}.get(binary_name)
    if archive_prefix is None:
        raise GatewayInstallError("Choose an official MobKit runtime or maintenance binary.")
    archive_name = f"{archive_prefix}-{VERSION}-{target}.tar.gz"
    receipt = destination.with_name(destination.name + ".receipt.json")
    if _verified_cache(destination, receipt, archive_name):
        return destination

    checksum_bytes = _download(f"{RELEASE_URL}/checksums.sha256", 1024 * 1024)
    try:
        entries = [line.split() for line in checksum_bytes.decode("ascii").splitlines()]
    except UnicodeDecodeError as error:
        raise GatewayInstallError("The release checksum manifest is invalid.") from error
    matches = [row[0] for row in entries if len(row) == 2 and row[1].lstrip("*") == archive_name]
    if len(matches) != 1 or not HEX_DIGEST.fullmatch(matches[0]):
        raise GatewayInstallError("The release checksum manifest must name this archive exactly once.")
    archive_bytes = _download(f"{RELEASE_URL}/{archive_name}", MAX_ARCHIVE_BYTES)
    archive_digest = hashlib.sha256(archive_bytes).hexdigest()
    if archive_digest != matches[0]:
        raise GatewayInstallError("Gateway download checksum mismatch. The existing gateway was not changed.")
    try:
        with tarfile.open(fileobj=io.BytesIO(archive_bytes), mode="r:gz") as archive:
            members = archive.getmembers()
            if (len(members) != 1 or members[0].name != binary_name or not members[0].isfile()
                    or not 0 < members[0].size <= MAX_BINARY_BYTES):
                raise GatewayInstallError("The gateway archive has an unexpected layout. Nothing was installed.")
            handle = archive.extractfile(members[0])
            if handle is None:
                raise GatewayInstallError("The gateway archive is missing its executable.")
            binary = handle.read(MAX_BINARY_BYTES + 1)
            if len(binary) != members[0].size:
                raise GatewayInstallError("The gateway archive is incomplete.")
    except (tarfile.TarError, EOFError, OSError) as error:
        raise GatewayInstallError("The gateway archive could not be read. Nothing was installed.") from error

    destination.parent.mkdir(parents=True, mode=0o700, exist_ok=True)
    _atomic_write(destination, binary, 0o700)
    receipt_data = {"version": VERSION, "core_version": CORE_VERSION, "archive": archive_name, "release_url": RELEASE_URL,
                    "archive_sha256": archive_digest, "binary_sha256": hashlib.sha256(binary).hexdigest()}
    _atomic_write(receipt, (json.dumps(receipt_data, indent=2) + "\n").encode(), 0o600)
    return destination
