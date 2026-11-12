"""The Guide runs the workspace's own Meta release.

Meta, Core and Connect share this repository's version and release together.
SELECTED_VERSION is that version: the Guide runs Meta of it with Core of the
same version, never an older, separately pinned release.

In a source checkout, the development environment and the installer workflow
install this commit's Meta Python SDK (sdk/python), and a gateway the Guide
runs is built from the same commit. A packaged Forge carries the same
release's published client wheel and rpc_gateway: the release inputs pin both
by size and SHA-256, the package manifest records them, and packaging and
product verification refuse a client of another release. The client wheel
confers no application SDK, native gateway or persisted-state compatibility.

scripts/bump-sdk-versions.sh moves SELECTED_VERSION with Meta's version and
scripts/verify-version-parity.sh checks that they agree.
"""

from email.parser import BytesParser
import io
import re
import zipfile
import zlib

SELECTED_VERSION = "0.9.14"
DISTRIBUTION = "meerkat-meta"
MAX_WHEEL_BYTES = 32 * 1024 * 1024
# The largest dist-info entry verify() reads, METADATA or WHEEL. The client
# wheel's METADATA is about a kilobyte; a long description would fit too.
MAX_DIST_INFO_BYTES = 1024 * 1024


def declared_core_version():
    """The Core version of the Guide's Meta release: the same workspace version."""
    if type(SELECTED_VERSION) is not str or re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", SELECTED_VERSION) is None:
        raise ValueError("The Guide needs the workspace's exact Meta version.")
    return SELECTED_VERSION


def wheel_path():
    """Where a packaged Forge carries the Guide's client wheel."""
    return f"vendor/guide/meerkat_meta-{declared_core_version()}-py3-none-any.whl"


def selection(sha256, size):
    """The package manifest's record of the Guide's client wheel."""
    if type(sha256) is not str or re.fullmatch("[0-9a-f]{64}", sha256) is None or type(size) is not int or size <= 0:
        raise ValueError("The Guide client wheel needs its exact size and SHA-256.")
    return {"path": wheel_path(), "version": declared_core_version(), "sha256": sha256, "bytes": size}


def _dist_info_entry(archive, name, refusal):
    """One dist-info entry's bytes, refused past MAX_DIST_INFO_BYTES.

    MAX_WHEEL_BYTES bounds only the compressed wheel, and a small entry can
    expand to gigabytes. The entry's declared size is checked first, and the
    read stops past the bound whatever the entry declares. Only stored and
    deflated entries are read: zipfile expands those no further than a read
    asks, but expands each compressed chunk of its other methods in full.
    """
    entry = archive.getinfo(name)
    if (entry.file_size > MAX_DIST_INFO_BYTES
            or entry.compress_type not in (zipfile.ZIP_STORED, zipfile.ZIP_DEFLATED)):
        raise ValueError(refusal)
    with archive.open(entry) as stream:
        content = stream.read(MAX_DIST_INFO_BYTES + 1)
    if len(content) > MAX_DIST_INFO_BYTES:
        raise ValueError(refusal)
    return content


def verify(content):
    """Refuse bytes that are not the meerkat-meta wheel of the Guide's release.

    Byte identity is the release inputs' pin and the package manifest's record.
    This reads only the wheel's own metadata, each entry within
    MAX_DIST_INFO_BYTES; nothing is imported or installed. An entry zipfile
    cannot read (corrupt, truncated, encrypted, or of an unsupported method)
    is the same refusal.
    """
    version = declared_core_version()
    refusal = f"The Guide client is not the published Meta {version} wheel."
    info = f"meerkat_meta-{version}.dist-info/"
    try:
        if type(content) is not bytes or not 0 < len(content) <= MAX_WHEEL_BYTES:
            raise ValueError(refusal)
        with zipfile.ZipFile(io.BytesIO(content)) as archive:
            metadata = BytesParser().parsebytes(_dist_info_entry(archive, info + "METADATA", refusal))
            wheel = BytesParser().parsebytes(_dist_info_entry(archive, info + "WHEEL", refusal))
    except (ValueError, KeyError, EOFError, NotImplementedError, RuntimeError, zipfile.BadZipFile, zlib.error):
        raise ValueError(refusal) from None
    if (metadata.get_all("Name") != [DISTRIBUTION] or metadata.get_all("Version") != [version]
            or wheel.get_all("Root-Is-Purelib") != ["true"] or wheel.get_all("Tag") != ["py3-none-any"]):
        raise ValueError(refusal)
