"""Read an original workspace receipt without observing or controlling its app."""
from __future__ import annotations

import asyncio
import json
from uuid import UUID

from .projects import DraftConflict
from .remote_deployment import private_file

MAX_RECORD_BYTES = 1024 * 1024


def _identity(value):
    try:
        if not isinstance(value, str) or str(UUID(value)) != value:
            raise ValueError
    except (ValueError, AttributeError):
        raise ValueError("An exact original setup and deployment ID are required.") from None
    return value


def _read(kind, service, guide_id, record_id):
    # The server supplies its existing workspace service, never a request path
    # or workspace selector. Supplying an ID cannot cross that service's root.
    _identity(guide_id)
    _identity(record_id)
    if kind == "deployment":
        directory = service._directory(guide_id, record_id)
        path = directory / "receipt.json"
    elif kind == "remote-deployment":
        directory = service.directory(guide_id, record_id)
        path = directory / "receipt.json"
    elif kind == "gcp-deployment":
        # _directory creates folders; a lookup must not do that. _read accepts
        # this exact original path and verifies the operation and its journals.
        directory = service.root / "gcp-provisioning" / guide_id
        path = directory / (record_id + ".json")
    else:
        raise ValueError("Use an existing deployment receipt kind.")
    try:
        payload = private_file(path, MAX_RECORD_BYTES)
    except FileNotFoundError:
        # Only an absent receipt is missing. A missing package, failed read or
        # invalid ownership below is an observation failure, never removal.
        raise LookupError("The original deployment receipt was not found. Its absence does not prove removal.") from None

    if kind == "remote-deployment":
        _, receipt = service.stored(guide_id, record_id)
    elif kind == "gcp-deployment":
        receipt = service._read(directory, record_id)
    else:
        try:
            receipt = json.loads(payload)
        except (ValueError, RecursionError):
            raise DraftConflict("The original deployment receipt could not be verified.") from None
    identity_key = "operation_id" if kind == "gcp-deployment" else "deployment_id"
    if (not isinstance(receipt, dict) or receipt.get("guide_id") != guide_id
            or receipt.get(identity_key) != record_id
            or not isinstance(receipt.get("status"), str) or not receipt["status"]):
        raise DraftConflict("The original deployment receipt could not be verified.")

    # Deliberately project only inventory facts. In particular, stored readiness,
    # access, console URLs, settings and capabilities cannot authorize dispatch.
    result = {"kind": kind, "guide_id": guide_id, "record_id": record_id,
        "recorded_status": receipt["status"],
        "observation": {"available": False, "reason": "retained_record_only"}}
    destination = receipt.get("destination")
    if kind == "gcp-deployment":
        if not isinstance(destination, dict):
            raise DraftConflict("The original cloud destination could not be verified.")
        result["destination"] = {key: destination[key] for key in ("project", "zone", "instance")}
    elif not isinstance(destination, str):
        raise DraftConflict("The original deployment destination could not be verified.")
    else:
        result["destination"] = destination
    if kind == "remote-deployment":
        endpoint = receipt.get("endpoint")
        if not isinstance(endpoint, dict) or not isinstance(endpoint.get("host"), str):
            raise DraftConflict("The original remote destination could not be verified.")
        result["host"] = endpoint["host"]
    return result


async def original_deployment_record(kind, service, guide_id, record_id):
    """Return retained facts only; callers still need fresh native observation.

    No latest pointer, registry, guide mutation, runtime probe, cloud request,
    browser reconciliation or interruption settlement is involved.
    """
    return await asyncio.to_thread(_read, kind, service, guide_id, record_id)
