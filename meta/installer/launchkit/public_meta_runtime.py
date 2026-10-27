"""The public Meta release that compiled agent teams install.

An agent-team package pip-installs this exact PyPI wheel, checked against its
SHA-256 in a step of its own, and downloads the release's native gateways,
checked against the release's published checksum manifests. Applications keep
their own runtime selection; nothing here selects a product or a candidate.
"""
from __future__ import annotations

import argparse
import asyncio
from dataclasses import asdict, is_dataclass
import hashlib
import io
import json
import os
from pathlib import Path
import tarfile
import tempfile

VERSION = "0.9.4"
SDK_DISTRIBUTION = "meerkat-meta"
SDK_WHEEL_SHA256 = "46cc44eb3e05a4bd9fb2ec630c06f2e3c62548184b7f6c81101f042d8e11a659"
RELEASE_URL = f"https://github.com/lukacf/meerkat-meta/releases/download/v{VERSION}"
CATALOG_FORMAT = "forge-public-meta-model-catalog-v1"
MODEL_CATALOG = Path(__file__).with_name(f"meta_model_catalog_{VERSION.replace('.', '_')}.json")
MODELS_UNLISTED = ("The Meta {version} runtime this agent team installs does not list {models}. "
    "{listed}Choose a listed model for those classes. Your plan is preserved.")
LISTED_PER_PROVIDER = 3


def sdk_requirements():
    """The single hash-pinned wheel, installed with --no-deps --require-hashes."""
    return f"{SDK_DISTRIBUTION}=={VERSION} --hash=sha256:{SDK_WHEEL_SHA256}\n"


def model_catalog(path=MODEL_CATALOG):
    """Read the recorded catalog of the pinned release, refusing another version."""
    value = json.loads(Path(path).read_text())
    if (not isinstance(value, dict) or value.get("format") != CATALOG_FORMAT
            or value.get("version") != VERSION
            or not isinstance(value.get("models"), list) or not value["models"]
            or not isinstance(value.get("provider_defaults"), list)):
        raise ValueError(f"The recorded model catalog is not the pinned Meta {VERSION} catalog.")
    entries = list(value["models"])
    for defaults in value["provider_defaults"]:
        if not isinstance(defaults, dict) or not isinstance(defaults.get("models"), list):
            raise ValueError("The recorded model catalog has invalid provider defaults.")
        entries += defaults["models"]
    if any(not isinstance(entry, dict) or any(not isinstance(entry.get(key), str) or not entry[key]
            for key in ("provider", "id")) for entry in entries):
        raise ValueError("The recorded model catalog has an invalid entry.")
    return value


def unlisted_models(selections, catalog=None):
    """The (provider, model) pairs the pinned catalog does not list.

    The same allowlist rule as a selected product's catalog: a model counts
    when the catalog lists it, directly or in a provider's defaults.
    """
    catalog = model_catalog() if catalog is None else catalog
    known = {(entry["provider"], entry["id"]) for entry in catalog["models"]}
    known.update((entry["provider"], entry["id"]) for defaults in catalog["provider_defaults"]
        for entry in defaults["models"])
    return sorted(set(selections) - known)


def listed_models(provider, catalog=None):
    """A few models the pinned catalog lists for a provider: its default first,
    then its recommended models in catalog order."""
    catalog = model_catalog() if catalog is None else catalog
    names = [row["default_model_id"] for row in catalog["provider_defaults"]
        if row.get("provider") == provider and isinstance(row.get("default_model_id"), str)]
    names += [entry["id"] for entry in catalog["models"]
        if entry["provider"] == provider and entry.get("tier") == "recommended"]
    return list(dict.fromkeys(names))[:LISTED_PER_PROVIDER]


def models_unlisted_reason(missing, catalog=None):
    catalog = model_catalog() if catalog is None else catalog
    names = ", ".join(f"{provider} {model}" for provider, model in missing)
    listed = ""
    for provider in dict.fromkeys(provider for provider, _ in missing):
        choices = listed_models(provider, catalog)
        if choices:
            listed += f"Listed {provider} models include {', '.join(choices)}. "
    return MODELS_UNLISTED.format(version=VERSION, models=names, listed=listed)


def _plain(value):
    if is_dataclass(value):
        return _plain(asdict(value))
    if isinstance(value, dict):
        return {key: _plain(item) for key, item in value.items()}
    if isinstance(value, (list, tuple)):
        return [_plain(item) for item in value]
    return value


def _verified_gateway(archive, manifest, directory):
    name = Path(archive).name
    payload = Path(archive).read_bytes()
    rows = [line.split() for line in Path(manifest).read_text(encoding="ascii").splitlines()]
    expected = [row[0] for row in rows if len(row) == 2 and row[1].lstrip("*") == name]
    digest = hashlib.sha256(payload).hexdigest()
    if (len(expected) != 1 or expected[0] != digest
            or not name.startswith(f"meta-rpc-gateway-{VERSION}-")):
        raise SystemExit(f"{name} is not this release's checksum-listed rpc gateway archive.")
    with tarfile.open(fileobj=io.BytesIO(payload), mode="r:gz") as bundle:
        member = bundle.next()
        if member is None or member.name != "rpc_gateway" or not member.isfile() or bundle.next() is not None:
            raise SystemExit("The rpc gateway archive has an unexpected layout.")
        bundle.extract(member, directory, filter="data")
    gateway = Path(directory) / "rpc_gateway"
    os.chmod(gateway, 0o700)
    return gateway, name, digest


async def _read_catalog(gateway):
    from meerkat_meta import Meta
    # A mob needs one profile; no member is spawned, so no provider is called.
    definition = ('[mob]\nid = "forge-model-catalog"\n\n'
        '[profiles.reader]\nmodel = "gpt-5.5"\nprovider = "openai"\n')
    runtime = await Meta.builder().mob_inline(definition).gateway(str(gateway)).build()
    try:
        return _plain(await runtime.mob_handle().models_catalog())
    finally:
        await runtime.shutdown()


def record_model_catalog(archive, manifest, output):
    """Record the pinned release's native catalog from its own gateway and SDK.

    A developer step, run with the pinned meerkat-meta wheel installed and the
    release's rpc gateway archive and checksum manifest downloaded.
    """
    from importlib.metadata import version
    if version(SDK_DISTRIBUTION) != VERSION:
        raise SystemExit(f"Install {SDK_DISTRIBUTION} {VERSION} before recording its catalog.")
    with tempfile.TemporaryDirectory() as directory:
        gateway, name, digest = _verified_gateway(archive, manifest, directory)
        catalog = asyncio.run(_read_catalog(gateway))
    record = {"format": CATALOG_FORMAT, "version": VERSION,
        "source": {"release": RELEASE_URL, "archive": name, "archive_sha256": digest,
            "sdk": f"{SDK_DISTRIBUTION}=={VERSION}", "sdk_wheel_sha256": SDK_WHEEL_SHA256,
            "method": "runtime.mob_handle().models_catalog()"},
        "models": catalog["models"], "provider_defaults": catalog["provider_defaults"]}
    Path(output).write_text(json.dumps(record, indent=2, ensure_ascii=True) + "\n")
    model_catalog(output)


def main():
    parser = argparse.ArgumentParser(description="Record the pinned Meta release's model catalog.")
    parser.add_argument("--archive", required=True, help="meta-rpc-gateway release archive for this host")
    parser.add_argument("--manifest", required=True, help="the release checksum manifest that lists it")
    parser.add_argument("--output", default=str(MODEL_CATALOG))
    arguments = parser.parse_args()
    record_model_catalog(arguments.archive, arguments.manifest, arguments.output)


if __name__ == "__main__":
    main()
