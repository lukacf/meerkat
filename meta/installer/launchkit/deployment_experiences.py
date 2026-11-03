"""Read declared interface links from the exact owned installed package."""
from __future__ import annotations

import json
import os
from pathlib import Path
import stat

from .application_experiences import declared_application_experiences, project_application_experiences
from .deployment import parse_setting
from .deployment_evidence import project_deployment_evidence
from .deployment_updates import checked_package
from .projects import DraftConflict


def nonsecret_port_settings(destination, keys):
    """Read only declared nonsecret fields; private values never leave this host."""
    if not keys:
        return {}
    descriptor = os.open(Path(destination) / ".env", os.O_RDONLY | os.O_NOFOLLOW)
    with os.fdopen(descriptor, "r", encoding="utf-8") as source:
        info = os.fstat(source.fileno())
        if (not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_nlink != 1
                or stat.S_IMODE(info.st_mode) != 0o600 or info.st_size > 256 * 1024):
            raise ValueError("Declared interface port settings need the owned private configuration.")
        result = {}
        for line in source:
            key, separator, value = line.strip().partition("=")
            if separator and key in keys:
                result[key] = parse_setting(value)
        return result


async def installed_application_experiences(service, guide_id):
    state = await service._guide(guide_id)
    receipt = await service.guide_status(guide_id, state=state)
    result = {key: receipt.get(key) for key in ("deployment_id", "build_id", "package_sha256")}
    result.update(source_digest=None, metadata_sha256=None, scope="declared-source", verified=False, entries=[])
    if receipt.get("source_verified") is not True:
        result["error"] = "The installed source could not be verified. Declared application links remain unavailable."
        return result
    try:
        _, files = checked_package(service._directory(guide_id), "package.zip", receipt["package_sha256"])
        definition = json.loads(files["deployment.json"][0])
        declared = declared_application_experiences(files, definition)
        keys = {row["port_field"] for row in declared["entries"] if "port_field" in row}
        settings = nonsecret_port_settings(receipt["destination"], keys)
        native = project_deployment_evidence(state, receipt)
        projection = project_application_experiences(files, definition,
            source_digest=receipt["source_digest"], configuration_values=settings,
            console_url=receipt.get("console_url") if native["native_runtime_available"] else None,
            remote=definition.get("target") == "gcp")
        result.update(projection)
    except (OSError, ValueError, KeyError, TypeError, DraftConflict):
        # Source declarations and private parse failures cannot fail installation
        # or disclose raw environment values in a public status response.
        result["error"] = "Declared application links need valid source metadata and nonsecret port settings. Installation controls remain available."
    return result
