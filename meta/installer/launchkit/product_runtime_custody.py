"""Internal byte view under the original stopped destination source owner.

The accepted application retains its producer locators. Only the actual live
staging carrier can supply authenticated destination locators to a reader.
Nothing here admits installation, native inputs, a runtime or a replacement.
"""
from __future__ import annotations

import hashlib
from pathlib import Path
import stat


def product_source_custody(source_custody):
    """Reject uninitialized carriers as well as wire values and target pairs."""
    from .connect_installer_caller import CustodyError
    from .remote_product_destination_owner import StoppedProductSource
    if (type(source_custody) is not StoppedProductSource
            or any(not hasattr(source_custody, name) for name in (
                "_lease", "_root", "_files", "_observed", "_origin", "_relocated",
                "_artifacts", "_owner_current", "_closed", "_invalidated"))):
        raise CustodyError("Runtime bytes need their original initialized stopped source owner.")
    return source_custody


def require_product_source(source_custody, target=None, *, destination=None, files=None):
    """Check file custody only; the calling async owner brackets this reader."""
    from .connect_installer_caller import CustodyError
    from .runtime_candidates import MetaToolkitRuntimeTarget
    source_custody = product_source_custody(source_custody)
    if (target is not None and (type(target) is not MetaToolkitRuntimeTarget
                or source_custody.origin != target)):
        raise CustodyError("Relocated runtime bytes need their original live stopped source owner.")
    source_custody.require_filesystem_current()
    if destination is not None and Path(destination).absolute() != source_custody._root:
        raise CustodyError("Relocated runtime bytes belong to another original destination.")
    if files is not None:
        if type(files) is not dict or set(files) != set(source_custody.files):
            raise CustodyError("Relocated runtime reads need the exact full accepted application source.")
        for name, pin in source_custody._files.items():
            item = files[name]
            expected = source_custody._observed[-1].files[pin.path][1]
            if (type(item) is not tuple or len(item) != 2 or type(item[0]) is not bytes
                    or type(item[1]) is not int or len(item[0]) != pin.size
                    or hashlib.sha256(item[0]).hexdigest() != pin.sha256
                    or item[1] != stat.S_IMODE(expected[2])):
                raise CustodyError("Relocated runtime reads changed accepted source bytes or modes.")
    source_custody.require_filesystem_current()
    return source_custody


def relocated_artifacts(target, *, source_custody, verified_artifacts=None,
        destination=None, files=None):
    """Recheck the separately received closure without changing source identity."""
    from .connect_installer_caller import CustodyError
    from .runtime_candidates import MetaToolkitRuntimeArtifacts, recheck_meta_toolkit_artifacts
    source = require_product_source(source_custody, target, destination=destination, files=files)
    if verified_artifacts is not None and (type(verified_artifacts) is not MetaToolkitRuntimeArtifacts
            or verified_artifacts is not source.artifacts):
        raise CustodyError("Relocated runtime reads cannot substitute another artifact observation.")
    result = recheck_meta_toolkit_artifacts(source.artifacts, source.relocated)
    require_product_source(source, target, destination=destination, files=files)
    return result


def require_product_installation(source_custody, receipt):
    """Join an existing original install journal; staging is never that journal."""
    from .connect_installer_caller import CustodyError
    from .runtime_selection import parse_candidate_selection
    source = product_source_custody(source_custody)
    if (type(receipt) is not dict or receipt.get("scope") is not None
            or receipt.get("destination") != source.receipt["destination"]
            or any(receipt.get(name) != source.receipt[name] for name in
                ("guide_id", "deployment_id", "build_id", "package_sha256"))):
        raise CustodyError("The existing installation differs from its original staged source ownership.")
    require_product_source(source, parse_candidate_selection(receipt.get("runtime_candidate")),
        destination=receipt.get("destination"))


def gcp_product_deployment_target(source_custody, target, *, destination=None, files=None):
    """Join unchanged cloud source to its original entered Linux byte owner.

    This file-only reader rechecks the existing intake and package contracts.
    A schema or target declaration alone cannot select physical runtime bytes.
    The async caller retains its original task's owner checks separately.
    """
    from .connect_installer_caller import CustodyError
    from .remote_product_destination_owner import _context, _json, _package_source, MAX_APPLICATION_BYTES
    from .runtime_candidates import _Inputs
    from .runtime_selection import parse_candidate_selection
    from .bootstrap import platform_target
    source = require_product_source(source_custody, target, destination=destination, files=files)
    context = _context(source._context, source._owner_source)
    if context["schema"] != 2 or context.get("source_disposition", {}).get("target") != "gcp":
        raise CustodyError("GCP runtime reads need their original authenticated cloud source disposition.")
    if target.target != "linux-x86_64" or platform_target() != "x86_64-unknown-linux-gnu":
        raise CustodyError("GCP runtime reads require the original supported Linux x86_64 guest.")
    retained = _json(context)
    context_path = source._store / "owner.json"
    original = next((held.files[context_path][0] for held in source._observed
        if context_path in held.files), None)
    if (source._store != Path(context["host"]["home"]) / ".local/state/meta-forge/product-sources" / context["deployment_id"]
            or original is None or original.sha256 != hashlib.sha256(retained).hexdigest()
            or original.size != len(retained)):
        raise CustodyError("GCP runtime reads changed their original cloud source custody.")
    observed = _Inputs(source._store)
    observed.read(source._store / "owner.json", hashlib.sha256(retained).hexdigest(), len(retained),
        size=len(retained), keep=False)
    _, payload = observed.read(source._store / "package.zip", context["package_sha256"],
        MAX_APPLICATION_BYTES, size=context["package_size"])
    original_files, selected = _package_source(payload, context, source.origin)
    if (parse_candidate_selection(selected) != target or target.target != "linux-x86_64"
            or files is not None and files != original_files):
        raise CustodyError("GCP runtime reads differ from their original cloud source and Linux product.")
    observed.finish()
    require_product_source(source, target, destination=destination, files=original_files)
    return "linux"


async def await_product_validation(source_custody, function, /, *args, **kwargs):
    """Keep the original callback on its calling task and join one file worker."""
    if source_custody is None:
        from .deployment import await_validation
        return await await_validation(function, *args, **kwargs)
    from .remote_product_artifacts import _joined
    source_custody = product_source_custody(source_custody)
    source_custody.require_owner_current()
    def read():
        source_custody.require_filesystem_current()
        result = function(*args, **kwargs)
        source_custody.require_filesystem_current()
        return result
    result = await _joined(read, _owner_current=source_custody.require_owner_current)
    source_custody.require_owner_current()
    return result
