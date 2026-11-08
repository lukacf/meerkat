#!/usr/bin/env python3
"""Assemble an offline, pinned GHE input bundle without installing or launching it."""
from __future__ import annotations

import argparse
import importlib.util
import io
import json
from pathlib import Path
import tarfile

SPEC = importlib.util.spec_from_file_location("github_enterprise_owner", Path(__file__).with_name("install-library-consumer.py"))
owner = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(owner)

ARCHIVE_SHA256 = "f797c20fe02a755865d0e2d46184ab47e31bf1619b0855b8d89c531e45a651c2"
ARCHIVE_BYTES = 9353785
MEMBERS = {"LICENSE": 1063, "README.md": 120055, "github-mcp-server": 29311136}
GUIDE = b"""# GitHub Enterprise selected reads

This separate official-github/enterprise-selected-reads profile retains the
GitHub MCP server v2.0.3 Linux x86_64 release, source revision
b98d115bbb080e9555466a96656ec4df605aea21. It does not change selected-reads.

Select a nonempty subset of these raw tools, in this order:
list_pull_requests, pull_request_read, list_commits, get_commit, actions_list,
actions_get, list_issues, issue_read, get_me.

get_me is the pinned server's authenticated-user read. Select it explicitly when
the trusted host needs account identity; it is not added to collection-only
selections. A successful provider response is still required to observe identity.

Configuration has exactly profile, operations and config. The config has exactly
token_env (GITHUB_PERSONAL_ACCESS_TOKEN) and github_host (a canonical lower-case
HTTPS DNS origin, with no port, path, userinfo, query or fragment). The origin must
name an explicitly selected enterprise instance, not github.com or localhost.
The host and tools become exact launch arguments and are checked on readback.
The fixed --toolsets= argument prevents inherited toolsets from adding tools.
Template placeholders are metadata only and must never be launched.

The native host must supply the protected PAT and control the entire child
environment. An empty env object does not remove inherited variables. Inherited
toolset, configuration, credential and endpoint overrides require host custody.
Tool selection is a read-tool ceiling, not a repository-argument restriction.
The caller's trusted programs and provider permissions must enforce repository
scope and method selection. No arbitrary flags, executable or environment map is
accepted by this installation profile.

This bundle is an offline source input. Assembly, configuration checking and
installation/readback do not execute the server, authenticate, inspect secrets,
call a provider, attach a native service or establish readiness. Live schema,
target-host and account qualification remain separate. Exact repository metadata
and arbitrary historical commit-status coverage are not established. Representative
reads do not establish full-history collection.

The upstream release archive includes its MIT LICENSE, retained here. This route
does not claim a complete dependency-notice audit or release qualification.
"""


def assemble(archive: Path, destination: Path):
    if not archive.is_absolute() or owner.existing_resolved(archive) != str(archive):
        raise owner.InstallError("canonical archive path required")
    raw = owner.read(archive.parent, archive.name, ARCHIVE_BYTES)
    if len(raw) != ARCHIVE_BYTES or owner.sha(raw) != ARCHIVE_SHA256:
        raise owner.InstallError("enterprise release archive differs")
    payloads = {}
    with tarfile.open(fileobj=io.BytesIO(raw), mode="r:gz") as stream:
        for member in stream:
            if (not member.isfile() or member.name not in MEMBERS or member.name in payloads
                    or member.size != MEMBERS[member.name]):
                raise owner.InstallError("enterprise release archive layout differs")
            with stream.extractfile(member) as selected:
                data = selected.read(member.size + 1)
            if len(data) != member.size:
                raise owner.InstallError("enterprise release archive member differs")
            payloads[member.name] = data
    if set(payloads) != set(MEMBERS):
        raise owner.InstallError("enterprise release archive incomplete")
    binary, license_bytes = payloads["github-mcp-server"], payloads["LICENSE"]
    if (owner.sha(binary) != owner.GHE_ARTIFACT_SHA256
            or owner.sha(license_bytes) != owner.GHE_LICENSE_SHA256):
        raise owner.InstallError("enterprise release payload differs")
    owner.check_official_executable(binary, "linux-x86_64")
    component = {"catalog_id": "github", "profile": owner.GHE_PROFILE,
        "artifact": "github/github-mcp-server", "target": "linux-x86_64",
        "source_revision": owner.GHE_SOURCE_REVISION, "artifact_sha256": owner.GHE_ARTIFACT_SHA256,
        "configuration_template": "github/mcp.template.json", "notices": "github/notices",
        "selected_operations": owner.GHE_TOOLS, "required_secret_environment": [owner.OFFICIAL_ENV],
        "runtime_profile": "GitHub MCP server v2.0.3 Linux x86_64; offline unexecuted input",
        "source_archive": {"sha256": ARCHIVE_SHA256, "bytes": ARCHIVE_BYTES}}
    files = {
        "delivery.json": owner.encoded({"format": "meerkat-toolkit.official-consumers.v1",
            "role": "service_connector", "components": [component]}),
        "github/github-mcp-server": binary,
        "github/mcp.template.json": owner.encoded(owner.official_template(owner.GHE_PROFILE)),
        "github/UPSTREAM-PROFILE.md": GUIDE,
        "github/notices/upstream-LICENSE": license_bytes,
    }
    contents = owner.encoded({"format": 1, "files": {
        name: {"sha256": owner.sha(data), "bytes": len(data)} for name, data in sorted(files.items())}})
    if (not destination.is_absolute() or destination.exists() or destination.is_symlink()
            or owner.existing_resolved(destination.parent) != str(destination.parent)
            or destination.name in ("", ".", "..")):
        raise owner.InstallError("canonical fresh destination required")
    destination.mkdir(mode=0o700)
    for name, data in {**files, "CONTENTS.json": contents}.items():
        path = destination / name
        path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
        with path.open("xb") as stream:
            stream.write(data)
        path.chmod(0o600)
    return {"scope": "offline_input_bundle_only", "bundle": str(destination),
        "contents_sha256": owner.sha(contents), "source_revision": owner.GHE_SOURCE_REVISION,
        "artifact_sha256": owner.GHE_ARTIFACT_SHA256,
        "excluded": ["No executable, provider call, credential access, installation, native attachment or release publication."]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--archive", required=True, type=Path)
    parser.add_argument("--destination", required=True, type=Path)
    args = parser.parse_args()
    try:
        print(json.dumps(assemble(args.archive, args.destination), indent=2))
    except (OSError, ValueError, tarfile.TarError) as error:
        parser.exit(2, f"enterprise bundle assembly failed: {error}\n")


if __name__ == "__main__":
    main()
