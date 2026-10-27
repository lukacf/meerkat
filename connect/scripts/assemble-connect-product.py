#!/usr/bin/env python3
"""Stage or assemble a selected Connect binary and resources; never build or run them."""
from __future__ import annotations

import argparse
import base64
from collections import deque
from contextlib import contextmanager
import gzip
import hashlib
import json
import mmap
import os
from pathlib import Path, PurePosixPath
import re
import runpy
import shutil
import stat
import struct
import tarfile
import tempfile
import unicodedata

ROOT = Path(__file__).resolve().parents[1]
TARGETS = {
    "macos-arm64": {
        "component": ROOT / "docs/verification/20261009-installed-entry-resources/macos-arm64/meerkat-connect-resources-macos-arm64.tar.gz",
        "component_sha256": "13e6e7fbb693eaa3cbe36e79db3ae8e332f521979e19dbc2f38f1f4468be803b",
        "component_size": 237876720,
        "resources_sha256": "bdec5a6ac9bbaecb786be6ffecbe14d725ba03edb48e9cc9247f1c25b4cfef5a",
        "host": "For Apple Silicon Macs. The bundled runtimes require macOS 13.5 or later.",
    },
    "linux-x86_64": {
        "component": ROOT / "docs/verification/20261009-installed-entry-resources/linux-x86_64/meerkat-connect-resources-linux-x86_64.tar.gz",
        "component_sha256": "2c321110e1ebc0e59ba8a313fcbe911423b923f64cba253360850a05f2f82b77",
        "component_size": 273324496,
        "resources_sha256": "ef9e0a2da8a83c02f8db27ca606021ea11364e46233567beffcaaa6b42d65ad8",
        "host": "For Linux x86_64 with glibc 2.39, as declared by this frozen library profile.",
    },
}
PREPARE = ROOT / "sdk/consumer-setup/product/prepare.py"
PREPARE_SHA = "57a40022bc7a1297eda463896c16c5d3902948d2eb38ccaa8d412aee3b0f715f"
RESOURCE_PACKAGER = ROOT / "scripts/package-connect-resources.py"
GATEWAY_VERIFIER = ROOT / "scripts/verify_native_gateway_subprocess.py"
RESOURCE_PACKAGER_SHA = "d71167042acd664e37601915588d5ed9822c0f3ed465df1a9ee979cb14f4bffe"
BINARY_KINDS = ("meerkat-connect", "toolkit_gateway")
COMPONENT_LIMIT = 1024 * 1024 * 1024
UNPACKED_LIMIT = 2 * 1024 * 1024 * 1024
ENTRY_LIMIT = 50008
GATEWAY_EVIDENCE_LIMIT = 32 * 1024 * 1024
STARTER_FILES = frozenset(("README.md", "bootstrap_host.py", "current-distribution.json",
                           "prepare_host.py", "release-manifest.json", "start.py"))
LICENSES = {
    "LICENSE-MIT": "b45d1f632f83aeb359da8e2b8716ad72591ec7267a489fdda39989c618d65a0f",
    "LICENSE-APACHE": "3a0ffd02ca79c2a687921b0f5d8094f2f6a7058a4c01775d1351ea6514fcd180",
}
README = """# Meerkat Connect

{host}
This archive includes Python, Node/npm, the selected integration library and
its setup resources. You do not need to install those runtimes or run a server.
Use the matching Meerkat `rkat` installation for ordinary agent work after setup.

In Terminal, open the directory containing the downloaded archive and run:

```sh
tar -xpf {archive}
./{prefix}/bin/meerkat-connect
```

Keep the whole extracted directory together. The `-p` extraction option
preserves the resource permissions required by the supplied installation.
To move the application, move that whole directory, then run its `bin/meerkat-connect`.

Choose a realm when prompted. Describe the service integration you need. Review
the setup helper, model, software, account and exact operations when asked.
Enter credentials only in protected controls, never in the conversation.
After successful finalization, enter `/quit` and wait for normal exit. Use an
ordinary `rkat` agent in the same realm for the business task. Installation,
saved credentials and access grants are separate outcomes; a real service call
establishes whether the integration works. Frontends additionally need a
compatible persistent application host.

## Assembly evidence and release status

The builder writes `assembly.json` beside this archive. It records component
checks for these exact binary and resource bytes. Its `not_qualified` list and qualification fields
identify what those checks do not establish. It does not qualify the complete
CLI setup, account, ordinary-agent or frontend journey. Historical scope names
ending in `preview` are retained as component-evidence identifiers; they do not
determine whether these bytes have subsequently been published.

Creating this archive does not publish it or establish a released product.
For a published release, follow its release notes for the exact native versions,
qualified platforms, supported account profiles and additional consumer results.
Those results must identify the same supplied binary and resources. A renamed
archive or a passing component check cannot substitute for them.

Project license texts are included at the top level. The supplied runtimes and
library retain their own licenses and notices under `libexec/meerkat-connect`.
"""

GATEWAY_README = """# Meerkat Toolkit application gateway

{host}
This prefix contains `bin/toolkit_gateway` and its complete installed Connect
resource tree, including Python, Node/npm and the selected integration library.
Keep the whole extracted directory together:

```sh
tar -xpf {archive}
```

An application host selects `./{prefix}/bin/toolkit_gateway` with the matching
SDK and its own native application selection. The host owns deployment, login
and startup. This archive does not install or activate an application by itself.
Ordinary Console users install and open their compatible application, sign in
through its normal login, and use its integrated setup panel.

## Assembly evidence and release status

The builder writes `assembly.json` beside this archive. It records the existing
installed-gateway component checks for these exact binary and resource bytes. Its `not_qualified`
list and qualification fields retain their component-only scope. They do not
qualify real-model execution, live account authentication, the complete Console
journey or every frontend. The historical scope name ending in `preview` is
retained as a component-evidence identifier; it does not determine whether
these bytes have subsequently been published.

Creating this archive does not publish it or establish a released product.
For a published release, follow its release notes for the exact native and SDK
versions, qualified application/platform and additional consumer results. Those
results must identify the same supplied binary and resources. A staged prefix
or a passing component check cannot substitute for them.

Project licenses are at the top level. Runtime and library licenses remain
under `libexec/meerkat-connect`. Preserve the resource permissions when moving
or extracting this prefix.
"""


def digest(path):
    value = hashlib.sha256()
    with path.open("rb") as stream:
        while block := stream.read(1024 * 1024):
            value.update(block)
    return value.hexdigest()


def require(condition, message):
    if not condition:
        raise ValueError(message)


def pin(path, expected, size=None):
    require(path.is_absolute() and not path.is_symlink()
            and stat.S_ISREG(path.lstat().st_mode), "regular absolute input required")
    require((size is None or path.stat().st_size == size) and digest(path) == expected,
            "input pin mismatch")


def unique_object(pairs):
    value = {}
    for key, item in pairs:
        require(key not in value, "duplicate record key")
        value[key] = item
    return value


def read_record(path, helpers=None):
    require(path.is_absolute() and not path.is_symlink()
            and stat.S_ISREG(path.lstat().st_mode) and path.stat().st_size <= 1024 * 1024,
            "bounded regular record required")
    with path.open("rb") as source:
        raw = source.read(1024 * 1024 + 1)
    require(len(raw) <= 1024 * 1024, "bounded regular record required")
    value = helpers["decode"](raw) if helpers else json.loads(raw, object_pairs_hook=unique_object)
    require(isinstance(value, dict) and type(value.get("schema_version")) is int
            and value["schema_version"] == 1, "unsupported record")
    return raw, value


def hash_value(value):
    return isinstance(value, str) and len(value) == 64 and all(ch in "0123456789abcdef" for ch in value)


def positive_size(value, limit):
    return type(value) is int and 0 < value <= limit


def selected_resources(target, record_path, record_sha, binary_kind):
    supplied = (record_path is not None, record_sha is not None, binary_kind is not None)
    require(not any(supplied) or all(supplied), "explicit resource selection requires all three inputs")
    if not any(supplied):
        return TARGETS[target], None, None
    require(binary_kind in BINARY_KINDS and hash_value(record_sha), "invalid explicit resource selection")
    raw, record = read_record(record_path)
    require(hashlib.sha256(raw).hexdigest() == record_sha, "resource build record pin mismatch")
    require(record_path.parent.resolve(strict=True) == record_path.parent,
            "canonical resource record parent required")
    require(record.get("target") == target
            and hash_value(record.get("resources_sha256"))
            and hash_value(record.get("archive_sha256"))
            and positive_size(record.get("archive_bytes"), COMPONENT_LIMIT), "resource build scope mismatch")
    component = record_path.parent / ("meerkat-connect-resources-" + target + ".tar.gz")
    original_archive = record.get("archive")
    require(isinstance(original_archive, str) and "\x00" not in original_archive,
            "resource build layout mismatch")
    original = PurePosixPath(original_archive)
    # Producer paths remain provenance when the original record and its
    # hash-bound sibling archive move to a different assembly host.
    require(original.anchor == "/" and str(original) == original_archive
            and ".." not in original.parts and original.name == component.name
            and record.get("resources") == str(original.parent / ("meerkat-connect-" + target)
                                               / "libexec/meerkat-connect"),
            "resource build layout mismatch")
    selection = record.get("input_selection")
    require(isinstance(selection, dict) and selection.get("kind") == "explicit_pinned_connect_resource_inputs",
            "explicit resource input provenance required")
    library, starter = selection.get("library"), selection.get("starter")
    require(isinstance(library, dict) and isinstance(library.get("path"), str)
            and Path(library["path"]).is_absolute() and hash_value(library.get("sha256"))
            and positive_size(library.get("bytes"), COMPONENT_LIMIT), "invalid selected library identity")
    require(isinstance(starter, dict) and isinstance(starter.get("path"), str)
            and Path(starter["path"]).is_absolute() and hash_value(starter.get("manifest_sha256"))
            and isinstance(starter.get("files"), dict) and set(starter["files"]) == STARTER_FILES,
            "invalid selected starter identity")
    for selected in starter["files"].values():
        require(isinstance(selected, dict) and hash_value(selected.get("sha256"))
                and positive_size(selected.get("bytes"), 2 * 1024 * 1024), "invalid selected starter file")
    require(starter["files"]["release-manifest.json"]["sha256"] == starter["manifest_sha256"],
            "selected starter manifest mismatch")
    pin(PREPARE, record.get("prepare_source_sha256"))
    pin(RESOURCE_PACKAGER, record.get("packager_source_sha256"))
    return {**TARGETS[target], "component": component,
            "component_sha256": record["archive_sha256"], "component_size": record["archive_bytes"],
            "resources_sha256": record["resources_sha256"]}, raw, record


def component_members(source, target):
    prefix = "meerkat-connect-" + target
    resource = prefix + "/libexec/meerkat-connect"
    entries, collision_keys, total = {}, set(), 0
    for member in source:
        name = member.name.rstrip("/") if member.isdir() else member.name
        require(name and len(name.encode("utf-8")) <= 4096 and "\\" not in name
                and not PurePosixPath(name).is_absolute()
                and all(part not in ("", ".", "..") for part in name.split("/")),
                "invalid component archive path")
        require(name in (prefix, prefix + "/bin", prefix + "/libexec", resource)
                or name.startswith(resource + "/"), "unexpected component archive path")
        key = unicodedata.normalize("NFC", name)
        if target == "macos-arm64":
            key = key.casefold()
        require(key not in collision_keys and len(entries) < ENTRY_LIMIT,
                "component archive collision or entry limit")
        collision_keys.add(key)
        require((member.isfile() or member.isdir() or member.issym()) and not member.sparse
                and member.mode & 0o7000 == 0 and type(member.size) is int and member.size >= 0,
                "unsupported component archive entry")
        require(member.isfile() or member.size == 0, "non-file component payload")
        total += member.size
        require(total <= UNPACKED_LIMIT, "component archive size limit")
        if name in (prefix, prefix + "/bin", prefix + "/libexec", resource):
            require(member.isdir(), "component layout must use directories")
        entries[name] = member
    for name in (prefix, prefix + "/bin", prefix + "/libexec", resource):
        require(name in entries, "incomplete component layout")
    for name, member in entries.items():
        if name != prefix:
            parent = PurePosixPath(name).parent.as_posix()
            require(parent in entries and entries[parent].isdir(), "component descends through a non-directory")
        if not member.issym():
            continue
        require(member.linkname and len(member.linkname.encode("utf-8")) <= 4096
                and "\\" not in member.linkname and not PurePosixPath(member.linkname).is_absolute(),
                "invalid component link")
        # Resolve against the complete archive namespace before creating any
        # path. This also rejects chained escapes, cycles and dangling links.
        pending = deque((*PurePosixPath(name).parent.parts, *member.linkname.split("/")))
        resolved, followed = [], 0
        while pending:
            part = pending.popleft()
            if part in ("", "."):
                continue
            if part == "..":
                require(len(resolved) > len(PurePosixPath(resource).parts), "component link escapes resources")
                resolved.pop()
                continue
            resolved.append(part)
            current = "/".join(resolved)
            require(current in entries, "component link target missing")
            selected = entries[current]
            if selected.issym():
                followed += 1
                require(followed <= 40 and selected.linkname
                        and "\\" not in selected.linkname
                        and not PurePosixPath(selected.linkname).is_absolute(), "component link cycle or escape")
                resolved.pop()
                pending.extendleft(reversed(selected.linkname.split("/")))
                require(len(pending) <= 4096, "component link expansion limit")
            elif pending:
                require(selected.isdir(), "component link traverses a file")
        require("/".join(resolved) == resource or "/".join(resolved).startswith(resource + "/"),
                "component link escapes resources")
    return entries


@contextmanager
def checked_component(selected, target):
    component = selected["component"]
    require(positive_size(selected["component_size"], COMPONENT_LIMIT), "component archive size limit")
    require(component.is_absolute() and not component.is_symlink()
            and stat.S_ISREG(component.lstat().st_mode), "regular absolute input required")
    # Capture and hash the same compressed bytes. Rechecking a mutable path
    # after a separate read cannot detect an A -> B -> A replacement.
    with os.fdopen(os.open(component, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK), "rb") as original, \
            tempfile.TemporaryFile() as captured, tempfile.TemporaryFile() as staged:
        metadata = os.fstat(original.fileno())
        require(stat.S_ISREG(metadata.st_mode) and metadata.st_size == selected["component_size"],
                "regular component with selected size required")
        total, observed = 0, hashlib.sha256()
        while block := original.read(1024 * 1024):
            total += len(block)
            require(total <= COMPONENT_LIMIT, "component archive size limit")
            observed.update(block)
            captured.write(block)
        require(total == selected["component_size"] and observed.hexdigest() == selected["component_sha256"],
                "component archive pin mismatch")
        captured.seek(0)
        # Bound decompression before tar metadata is parsed. Both retained
        # captures stay open through extraction; the input path is not reopened.
        total = 0
        with gzip.GzipFile(fileobj=captured, mode="rb") as compressed:
            while block := compressed.read(1024 * 1024):
                total += len(block)
                require(total <= UNPACKED_LIMIT, "component decompression limit")
                staged.write(block)
        staged.seek(0)
        with tarfile.open(fileobj=staged, mode="r:") as source:
            entries = component_members(source, target)
            yield source, entries


def extract_component(source, entries, destination):
    # No archive path descends through a link. Create all real objects before
    # links, then restore exact directory modes after their contents exist.
    for name, member in sorted(entries.items(), key=lambda item: (item[0].count("/"), item[0])):
        path = destination / name
        if member.isdir():
            path.mkdir(mode=0o755)
        elif member.isfile():
            with path.open("xb") as output, source.extractfile(member) as payload:
                shutil.copyfileobj(payload, output)
            path.chmod(member.mode)
    for name, member in entries.items():
        if member.issym():
            path = destination / name
            path.symlink_to(member.linkname)
            if stat.S_IMODE(path.lstat().st_mode) != member.mode:
                os.chmod(path, member.mode, follow_symlinks=False)
            require(stat.S_IMODE(path.lstat().st_mode) == member.mode, "component link mode changed")
    for name, member in sorted(entries.items(), reverse=True):
        if member.isdir():
            (destination / name).chmod(member.mode)


def composition_record(reference, helpers, candidate):
    path = Path(reference["path"])
    pin(path, reference["sha256"])
    _, value = read_record(path, helpers)
    require(value["kind"] == "meerkat_connect_installed_composition"
            and value["scope"] == "installed_product_native_catalog_preparation_and_help"
            and value["status"] == "passed" and value["candidate"] == candidate,
            "installed composition scope or candidate mismatch")
    tested = value["test_executable"]
    require({key: tested[key] for key in ("sha256", "size")} == candidate["test_executable"],
            "installed test executable mismatch")
    check_binary(Path(tested["path"]), tested["sha256"], target=candidate["target"],
                 resources_sha=candidate["resources_sha256"])
    pin(Path(tested["path"]), tested["sha256"], tested["size"])
    execution = value["execution"]
    require(execution["exit_code"] == 0 and isinstance(execution["argv"], list)
            and execution["argv"] and all(isinstance(arg, str) for arg in execution["argv"])
            and "--exact" in execution["argv"] and "--ignored" in execution["argv"]
            and value["scope"] in execution["argv"]
            and Path(execution["argv"][0]).is_absolute()
            and Path(execution["argv"][0]).resolve(strict=True) == Path(tested["path"]).resolve(strict=True),
            "installed execution identity mismatch")
    # The retained integration-test executable supplies this evidence. The
    # companion CLI is not thereby exercised, nor is its terminal interaction.
    require(isinstance(value["evidence"], list) and value["evidence"], "installed evidence required")
    for evidence in value["evidence"]:
        pin(Path(evidence["path"]), evidence["sha256"])


def shutdown_record(reference, helpers, candidate):
    path = Path(reference["path"])
    pin(path, reference["sha256"])
    _, value = read_record(path, helpers)
    require(value["kind"] == "meerkat_connect_native_shutdown_review"
            and value["scope"] == "active_setup_worker_shutdown"
            and value["disposition"] == "accepted" and value["candidate"] == candidate,
            "accepted candidate-bound native shutdown review required")
    require(isinstance(value["evidence"], list) and value["evidence"], "shutdown evidence required")
    for evidence in value["evidence"]:
        pin(Path(evidence["path"]), evidence["sha256"])


def gateway_capture(path):
    require(path.is_absolute() and not path.is_symlink(), "absolute regular gateway evidence required")
    with os.fdopen(os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK), "rb") as stream:
        info = os.fstat(stream.fileno())
        require(stat.S_ISREG(info.st_mode) and 0 <= info.st_size <= GATEWAY_EVIDENCE_LIMIT,
                "bounded regular gateway evidence required")
        raw = stream.read(GATEWAY_EVIDENCE_LIMIT + 1)
    require(len(raw) <= GATEWAY_EVIDENCE_LIMIT, "bounded regular gateway evidence required")
    return raw


def gateway_evidence(reference):
    """Capture the exact bounded bytes named by an existing verifier reference."""
    require(isinstance(reference, dict) and isinstance(reference.get("path"), str)
            and hash_value(reference.get("sha256")), "invalid gateway evidence reference")
    raw = gateway_capture(Path(reference["path"]))
    require(hashlib.sha256(raw).hexdigest() == reference["sha256"], "gateway evidence pin mismatch")
    if "bytes" in reference:
        require(type(reference["bytes"]) is int and len(raw) == reference["bytes"],
                "gateway evidence size mismatch")
    return raw


def gateway_json(reference):
    # Parse the same captured bytes that passed the selected digest. Never
    # reopen a record after checking its pathname.
    raw = gateway_evidence(reference)
    return raw, json.loads(raw, object_pairs_hook=unique_object)


def gateway_verification(reference, raw, candidate, build_raw):
    """Consume the existing installed-product verifier, not CLI acceptance."""
    require(hashlib.sha256(raw).hexdigest() == reference["sha256"], "gateway verification pin mismatch")
    report = json.loads(raw, object_pairs_hook=unique_object)
    require(isinstance(report, dict) and type(report.get("schema")) is int
            and report["schema"] == 1 and report.get("status") == "passed"
            and report.get("installed_candidate") == candidate
            and report.get("toolkit_gateway_sha256") == candidate["binary"]["sha256"]
            and report.get("raw_gateway") is None and report.get("raw_gateway_sha256") is None,
            "installed gateway verification scope or candidate mismatch")
    pin(GATEWAY_VERIFIER, report.get("script_sha256"))
    inputs = report["installed_inputs"]
    require(set(inputs) == {"staging", "native_build", "resource_build", "source_manifest", "resource_manifest"},
            "incomplete installed gateway inputs")
    _, staging = gateway_json(inputs["staging"])
    staged = Path(inputs["staging"]["path"]).parent
    package = staged / ("meerkat-connect-" + candidate["target"])
    resources = package / "libexec/meerkat-connect"
    require(staging.get("schema_version") == 1
            and staging.get("scope") == "unqualified_product_prefix_for_verification"
            and staging.get("candidate") == candidate and staging.get("binary_kind") == "toolkit_gateway"
            and staging.get("target") == candidate["target"] and staging.get("prefix") == package.name
            and Path(report["toolkit_gateway"]) == package / "bin/toolkit_gateway",
            "verified gateway differs from staged candidate")
    require(Path(inputs["native_build"]["path"]) == staged / "build-record.json"
            and gateway_evidence(inputs["native_build"]) == build_raw
            and inputs["native_build"]["sha256"] == staging["build_record_sha256"],
            "verification changed the original native build record")
    require(Path(inputs["resource_build"]["path"]) == staged / "resource-build-record.json"
            and inputs["resource_build"]["sha256"] == candidate["resource_build_record_sha256"]
            == staging["resource_build_record_sha256"], "verified resource build differs")
    _, resource_build = gateway_json(inputs["resource_build"])
    require(resource_build["resources_sha256"] == candidate["resources_sha256"]
            and resource_build["archive_sha256"] == staging["resource_component_sha256"],
            "verified resource archive differs")
    require(inputs["source_manifest"]["sha256"] == candidate["source_manifest_sha256"]
            and inputs["resource_manifest"]["sha256"] == candidate["resources_sha256"]
            and Path(inputs["resource_manifest"]["path"]) == resources / "resources.json",
            "verified source or resource manifest differs")
    gateway_evidence(inputs["source_manifest"])
    gateway_evidence(inputs["resource_manifest"])
    check_binary(package / "bin/toolkit_gateway", candidate["binary"]["sha256"],
                 target=candidate["target"], resources_sha=candidate["resources_sha256"])
    # The reviewed verifier names the modules it actually imported. These are
    # source identities, not a claim that SDK source paths are installed wheels.
    sdk_sources = report["sdk_sources"]
    sdk_files = {"meerkat_toolkit/host_control.py", "meerkat_toolkit/application_runtime.py",
                 "meerkat_meta/__init__.py", "meerkat_meta/builder.py", "meerkat_meta/runtime.py",
                 "meerkat_meta/_transport.py", "meerkat_meta/agent_builder.py",
                 "meerkat_meta/identity_first_models.py", "meerkat_meta/identity_first_providers.py",
                 "meerkat_meta/types.py"}
    require(isinstance(sdk_sources, list) and len(sdk_sources) == len(sdk_files)
            and {"/".join(Path(row["path"]).parts[-2:]) for row in sdk_sources} == sdk_files,
            "installed verifier SDK source closure missing")
    for source in sdk_sources:
        gateway_evidence(source)
    cases = report["cases"]
    names = ("installed-product-success", "installed-product-resource-corruption")
    require(isinstance(cases, list) and [case.get("case") for case in cases] == list(names),
            "both actual installed gateway cases are required")
    report_dir = Path(reference["path"]).parent
    for case, name in zip(cases, names):
        directory = report_dir / name
        # The report embeds each complete result. Compare it to the retained
        # existing result file without introducing another receipt format.
        case_path = directory / "result.json"
        case_raw = gateway_capture(case_path)
        require(json.loads(case_raw, object_pairs_hook=unique_object) == case,
                "gateway case result differs from selected report")
        require(case.get("status") == "passed" and case.get("candidate") == candidate
                and case.get("forced_cleanup") is False and case.get("scoped_execution_proven") is False
                and all(case.get(key) is True for key in ("natural_gateway_reap", "sdk_readers_joined",
                        "observed_children_gone", "provider_joined")), "gateway case lacks joined cleanup")
        termination = case["termination"]
        require(termination["state"] == "exited" and termination["signal"] is None
                and termination["escalation"] == "none", "gateway case used forced cleanup")
        for key, filename in (("model_configuration", "meerkat.toml"),
                              ("preparation_plan", "application-plan.json"),
                              ("events", "events.json"), ("gateway_stderr", "gateway.stderr")):
            require(Path(case[key]["path"]) == directory / filename, "gateway case evidence path differs")
            gateway_evidence(case[key])
        _, events = gateway_json(case["events"])
        require(isinstance(events, list) and 0 < len(events) <= 4096
                and all(isinstance(event, dict) for event in events), "invalid gateway event log")
        if name == names[0]:
            gateway_success(case, events, package)
        else:
            require(case.get("sentinel_connections") == [], "corrupt gateway contacted provider sentinel")
            gateway_resource_refusal(case, events, resources, directory, resource_build["prepare_source_sha256"])
    return {"candidate": candidate, "installed_gateway_verification_sha256": reference["sha256"],
            "cli_tty_qualified": False}


def gateway_success(case, events, package):
    require(case["termination"]["exit_code"] == 0 and case.get("http_listener_closed") is True,
            "gateway success did not exit normally")
    sent = [event["message"] for event in events if event.get("event") == "sdk_wire_out"]
    received = [event["message"] for event in events if event.get("event") == "sdk_wire_response"]
    def request(method):
        values = [value for value in sent if value.get("method") == method]
        require(len(values) == 1, "gateway request correlation missing: " + method)
        return values[0]
    def response(request):
        values = [value for value in received if value.get("id") == request["id"]]
        require(len(values) == 1 and "error" not in values[0], "gateway response correlation missing")
        return values[0]
    init, catalog, shutdown = (request(method) for method in ("meta/init", "meta/models/catalog", "meta/shutdown"))
    plan_raw, plan = gateway_json(case["preparation_plan"])
    preparation = init["params"]["prepared_application"]
    require(preparation["digest"] == case["preparation_plan"]["sha256"]
            and base64.b64decode(preparation["bundle_base64"], validate=True) == plan_raw
            and init["params"]["persistent_state"] == plan["application"]["realm"]["state_root"],
            "native init differs from the exact selected application plan")
    require(init["params"]["runtime_options"]["meerkat_config_path"] == case["model_configuration"]["path"],
            "catalog configuration was not selected by native init")
    ready = [event["params"] for event in events if event.get("event") == "sdk_init_event"
             and event.get("method") == "meta/init_settled" and event["params"].get("outcome") == "ready"]
    require(ready == [case["native_ready"]] and ready[0]["init_id"] == init["params"]["init_id"],
            "native Ready is not correlated to actual init")
    starts = [event for event in events if event.get("event") == "main_gateway_started"]
    require(len(starts) == 1 and starts[0]["pid"] == case["main_gateway_pid"]
            and starts[0]["binary"] == str(package / "bin/toolkit_gateway"),
            "verification used another main gateway")
    _, catalog_value = gateway_json(case["model_catalog"])
    require(bool(catalog_value) and response(catalog)["result"] == catalog_value,
            "model catalog differs from actual native response")
    require(response(shutdown) == case["shutdown_response"]
            and all(case["shutdown_response"]["result"].get(key) is True
                    for key in ("shutdown", "runtime_cleanup_completed", "host_cleanup_completed")),
            "actual gateway shutdown did not acknowledge both owners")
    finalization = case["finalization"]
    selection = finalization["selection"]
    require(selection == case["application_selection"], "gateway adopted a different application selection")
    _, manifest = gateway_json(finalization["completed_manifest"])
    require(manifest["kind"] == "application_setup_preparation" and manifest["schema"] == 1,
            "native application preparation evidence missing")
    directory = Path(finalization["completed_manifest"]["path"]).parent
    for item in manifest["artifacts"]:
        require(Path(item["path"]).name == item["path"], "completed artifact is not a basename")
        gateway_evidence({"path": str(directory / item["path"]), "sha256": item["sha256"]})
    gateway_evidence(finalization["completed_setup"])
    gateway_evidence(finalization["preparer_handoff"])
    for key in ("activation", "policy_manifest", "local_policy"):
        gateway_evidence(selection[key])
    for name, kind in (("prepare-policy", "application_policy_prepared"),
                       ("validate-policy", "application_policy_validated")):
        command = finalization[name]
        require(command["exit_code"] == 0 and command["argv"][0] == str(package / "bin/toolkit_gateway")
                and command["argv"][1] == "--" + name.replace("-policy", "-application-policy"),
                "native finalization command differs")
        commands = [event for event in events if event.get("event") == "native_artifact_command"
                    and event.get("name") == name]
        require(len(commands) == 1 and commands[0]["pid"] == command["pid"]
                and commands[0]["argv"] == command["argv"][1:],
                "native finalization command was not the observed process")
        _, output = gateway_json(command["stdout"])
        gateway_evidence(command["stderr"])
        require(output == {"kind": kind, "selection": selection}, "native finalization output differs")
    published = [index for index, event in enumerate(events) if event.get("event") == "fixture_published"]
    publication = [event for event in events if event.get("event") == "fixture_publish_entered"]
    require(len(publication) == 1 and publication[0]["selection"] == selection,
            "fixture publication selected different native artifacts")
    settled = [index for index, event in enumerate(events) if event.get("event") == "sdk_init_event"
               and event.get("method") == "meta/init_settled" and event["params"].get("outcome") == "ready"]
    require(len(published) == 1 and len(settled) == 1 and published[0] < settled[0],
            "native Ready preceded application publication")
    gateway_acquisition(case, events, Path(case["events"]["path"]).parent)


def gateway_public_story(content):
    """Apply the existing verifier installed_public_story result contract."""
    require(isinstance(content, str), "public story result is not text")
    decoder = json.JSONDecoder(object_pairs_hook=unique_object)
    values, offset = [], 0
    while offset < len(content):
        while offset < len(content) and content[offset] in " \t\r\n":
            offset += 1
        if offset == len(content):
            break
        value, offset = decoder.raw_decode(content, offset)
        values.append(value)
        require(len(values) <= 2, "unexpected extra public service result block")
    require(len(values) == 2 and isinstance(values[0], list)
            and isinstance(values[1], dict) and set(values[1]) == {"value"}
            and isinstance(values[1]["value"], str), "expected actual Agno text and structured result pair")
    stories = json.loads(values[1]["value"], object_pairs_hook=unique_object)
    # Match the producer's JSON comparison: true, 1 and 1.0 are distinct.
    actual, structured = (json.dumps(value, sort_keys=True, separators=(",", ":"),
                                    ensure_ascii=False, allow_nan=False).encode("utf-8")
                          for value in (values[0], stories))
    require(actual == structured and isinstance(stories, list) and len(stories) == 1,
            "public service results disagree or did not return exactly one story")
    story = stories[0]
    require(isinstance(story, dict) and type(story.get("id")) is int
            and 0 < story["id"] <= (1 << 64) - 1
            and isinstance(story.get("title"), str) and story["title"].strip(),
            "public service did not return a typed nonempty story")
    return {"id": story["id"], "title": story["title"]}


def gateway_business_tool_count(business, messages, b_index, result_index):
    """Count the exact selected B schedule, including native catalog controls."""
    catalog = business.get("original_catalog")
    if catalog is None:
        return 1
    operation = "get_top_hackernews_stories"
    require(set(catalog) == {"search_call_id", "load_call_id", "search_result_history_index",
            "load_result_history_index", "operation", "helper_catalog_scope_reused",
            "search_result", "load_result"}
            and catalog["search_call_id"] == "original-catalog-search"
            and catalog["load_call_id"] == "original-catalog-load"
            and catalog["operation"] == operation and catalog["helper_catalog_scope_reused"] is False,
            "gateway original catalog observation differs from its exact schedule")
    search_index, load_index = catalog["search_result_history_index"], catalog["load_result_history_index"]
    require(type(search_index) is int and type(load_index) is int
            and b_index < search_index < load_index < result_index,
            "gateway original catalog results did not precede its one read")
    for index, label in ((search_index, "search"), (load_index, "load")):
        row = messages[index]
        require(row.get("role") == "tool" and row.get("tool_call_id") == catalog[label + "_call_id"]
                and json.loads(row["content"]) == catalog[label + "_result"],
                "gateway original catalog result differs from retained model history")
    expected = [("original-catalog-search", "tool_catalog_search", {"query": operation, "limit": 50}),
                ("original-catalog-load", "tool_catalog_load", {"names": [operation]}),
                ("original-public-story", operation, {"num_stories": 1})]
    calls = [(call["id"], call["function"]["name"], json.loads(call["function"]["arguments"]))
        for row in messages[b_index + 1:result_index] if row.get("role") == "assistant"
        for call in row.get("tool_calls", [])]
    require(calls == expected, "gateway B tool calls differ from exact search, load and one read")
    return len(expected)


def gateway_controller_recovery(acquisition, business, messages, b_index, directory, receipt):
    """A display notice is accepted only with the exact B's retained typed audit."""
    notice = ("The requested model operation is unavailable under current authorization. "
              "Continue with the authorized controller without provider-hosted capabilities, "
              "or choose another permitted action.")
    positions = [i for i, row in enumerate(messages) if row.get("role") == "user"
                 and row.get("content") == notice]
    if not positions:
        require("controller_recovery_history_index" not in business
                and "controller_recovery_audit" not in acquisition,
                "controller recovery evidence has no observed feedback")
        return
    first_tool = next((i for i, row in enumerate(messages) if i > b_index
                       and (row.get("tool_calls") or row.get("role") == "tool")), len(messages))
    require(len(positions) == 1, "controller feedback must occur exactly once")
    position = positions[0]
    before = messages[b_index + 1:position]
    calls = [call.get("id") if isinstance(call, dict) else None
        for row in before for call in row.get("tool_calls", [])]
    results = [row.get("tool_call_id") for row in before if row.get("role") == "tool"]
    completed_boundary = (bool(calls) and all(isinstance(value, str) and value for value in calls)
        and len(calls) == len(set(calls))
        and calls == results)
    require(b_index < position and (position < first_tool or completed_boundary)
            and business.get("controller_recovery_history_index") == position
            and business.get("original_catalog") is not None,
            "controller feedback is not one selected B recovery at a completed boundary")
    pin = acquisition.get("controller_recovery_audit")
    require(isinstance(pin, dict) and Path(pin["path"]) == directory / "original-b-controller-recovery.json",
            "controller recovery lacks its retained native audit")
    _, audit = gateway_json(pin)
    expected = {"session_id", "input_id", "run_id", "row_sha256", "refused_operation_id",
                "refusal_count", "reason", "model", "provider", "usage", "hosted_capabilities",
                "recovery_operation_id", "recovered_http_status", "policy_publication_sequence",
                "policy_observed_at_ms"}
    require(set(audit) == expected and audit["session_id"] == receipt["outcome"]["session_id"]
            and audit["input_id"] == receipt["input_id"] and audit["run_id"] == receipt["run_id"]
            and hash_value(audit["row_sha256"])
            and type(audit["refusal_count"]) is int and audit["refusal_count"] > 0
            and audit["reason"] == "reprepare_required" and audit["model"] == "fixture-original-model"
            and audit["provider"] == "self_hosted" and audit["usage"] == "controller_inference"
            and audit["hosted_capabilities"] == [] and audit["recovered_http_status"] == 200
            and all(isinstance(audit[key], str) and audit[key]
                    for key in ("refused_operation_id", "recovery_operation_id"))
            and audit["refused_operation_id"] != audit["recovery_operation_id"]
            and all(type(audit[key]) is int and audit[key] >= 0
                    for key in ("policy_publication_sequence", "policy_observed_at_ms")),
            "controller recovery audit does not prove exact currentness recovery")


def gateway_acquisition(case, events, directory):
    acquisition = case.get("acquisition")
    require(isinstance(acquisition, dict)
            and acquisition.get("original_task_completion") == "native_finalized_completed_for_exact_b_input",
            "gateway acquisition lacks original task completion")
    retained = {}
    for key, filename in (("model_requests", "model-requests.json"),
                          ("service_parent_custody", "service-parent-custody.json"),
                          ("original_business_observation", "original-business-observation.json")):
        require(isinstance(case.get(key), dict) and Path(case[key]["path"]) == directory / filename,
                "gateway acquisition evidence path differs")
        _, retained[key] = gateway_json(case[key])
    # The business file is written before finally joins retirement and mutates
    # the returned custody object. Compare stable business facts, then require
    # the final custody file and case to attest that same joined operation.
    business_record = retained["original_business_observation"]
    require(isinstance(business_record, dict)
            and {key: value for key, value in business_record.items() if key != "custody"}
                == {key: value for key, value in acquisition.items() if key != "custody"},
            "gateway original business evidence differs")
    custody = retained["service_parent_custody"]
    require(isinstance(custody, dict) and custody == acquisition.get("custody")
            and custody.get("transfer") == "sdk_owned" and custody.get("retirement") == "joined"
            and custody.get("rule_owner_created") is True
            and type(custody.get("rule_releases_after_joined")) is int
            and custody["rule_releases_after_joined"] == 1
            and custody.get("rule_exclusion_still_held") is False
            and custody.get("operation_observation") == custody.get("retirement_observation")
                == acquisition.get("observation")
            and custody.get("proposal") == acquisition.get("proposal")
            and custody.get("ack") == acquisition.get("ack") == {"state": "answer_forwarded"},
            "gateway acquisition custody did not join the same operation")
    custody_events = [event for event in events if event.get("event") == "service_parent_custody_observed"]
    require(len(custody_events) == 1 and all(custody_events[0].get(key) == value for key, value in custody.items()),
            "gateway final custody is not the observed SDK retirement")
    for key, filename in (("authenticated_event", "authenticated-fixture-event.json"),
                          ("proposal", "service-parent-proposal.json")):
        require(Path(acquisition[key]["path"]) == directory / filename,
                "gateway acquisition source evidence path differs")
        _, retained[key] = gateway_json(acquisition[key])
    original, admission = acquisition["original"], acquisition["admission"]
    proposal, observed, fresh = retained["proposal"], acquisition["observation"], acquisition["fresh_original"]
    require(admission["state"] == "admitted" and admission["source_settlement"] == "fresh_acceptance_promoted"
            and admission["deduplicated"] is False and admission["turn_ticket"]
            and proposal["original"] == original and observed["outcome"] == "succeeded"
            and observed["custody"] == "joined" and observed["reason"] is None
            and observed["parent_issued"] is True and observed["parent_issuance_id"] == proposal["issuance_id"]
            and observed["child"]["state"] == "grant_issued" and observed["source"] == fresh
            and fresh["route"] == original["route"] == proposal["decision"]["route"]
                == observed["child"]["grant"]["route"]
            and fresh["original"] == proposal["decision"]["decision"] == observed["child"]["grant"]["decision"]
            and fresh["original"] != original["original"]
            and acquisition["rule_releases_after_joined"] == 1
            and acquisition["helper_result"] == {"state": "authorized", "grant_issued": True,
                "fresh_instruction_retained": True, "ready": False},
            "gateway acquisition did not join its original and fresh source")
    admissions = [event for event in events if event.get("event") == "authenticated_original_admitted"]
    require(len(admissions) == 1 and admissions[0]["original"] == original and admissions[0]["outcome"] == admission,
            "gateway original admission is not the observed native input")
    business, requests = acquisition["business"], retained["model_requests"]
    require(isinstance(requests, list) and 0 < len(requests) <= 512
            and all(isinstance(item, dict) for item in requests)
            and {item.get("model") for item in requests} == {"fixture-original-model", "fixture-helper-model"}
            and type(case.get("model_request_count")) is int and case["model_request_count"] == len(requests)
            and case.get("model_schedule_errors") == []
            and type(business["model_request_index"]) is int and 0 <= business["model_request_index"] < len(requests),
            "gateway controlled model requests are missing or inconsistent")
    request = requests[business["model_request_index"]]
    request_raw = json.dumps(request, sort_keys=True, separators=(",", ":"), ensure_ascii=False, allow_nan=False).encode("utf-8")
    require(hashlib.sha256(request_raw).hexdigest() == business["model_request_sha256"]
            and request["model"] == "fixture-original-model"
            and type(business["story"]["id"]) is int and business["story"]["id"] > 0
            and isinstance(business["story"]["title"], str) and business["story"]["title"].strip()
            and business["answer"] == "Hacker News story " + str(business["story"]["id"]) + ": " + business["story"]["title"],
            "gateway business answer differs from its selected model request")
    messages = request["messages"]
    a_index, b_index, result_index = (business[key] for key in
        ("a_input_history_index", "b_input_history_index", "matching_result_history_index"))
    require(isinstance(messages, list) and all(type(index) is int for index in (a_index, b_index, result_index))
            and 0 <= a_index < b_index < result_index < len(messages)
            and all(messages[index].get("role") == "user"
                and messages[index].get("content") == retained["authenticated_event"]["event"]["content"]
                for index in (a_index, b_index))
            and messages[result_index].get("role") == "tool"
            and messages[result_index].get("tool_call_id") == "original-public-story"
            and messages[result_index].get("content") == business["actual_tool_result"],
            "gateway business result is not in the selected fresh B model history")
    require(gateway_public_story(business["actual_tool_result"]) == business["story"],
            "gateway business story differs from the selected tool result")
    prepared = [event for event in events if event.get("event") == "original_public_answer_prepared"]
    require(len(prepared) == 1 and all(prepared[0].get(key) == value for key, value in business.items()),
            "gateway business answer is not the observed original response")
    native_b = acquisition["native_b"]
    receipts = [event for event in events if event.get("event") == "original_native_receipt_observed"]
    require(type(acquisition["native_receipt_reads"]) is int and acquisition["native_receipt_reads"] == len(receipts)
            and receipts and all(event["original"] == fresh for event in receipts)
            and receipts[-1]["observation"] == native_b and native_b["state"] == "admitted"
            and native_b["session_id"] == admission["session_id"] and native_b["input_id"] != admission["input_id"]
            and native_b["receipt"]["source"] in ("live_runtime", "durable_store"),
            "gateway B receipt is not the observed fresh input")
    receipt = native_b["receipt"]["report"]
    require(receipt["status"] == "finalized" and receipt["input_id"] == native_b["input_id"]
            and isinstance(receipt["run_id"], str) and bool(receipt["run_id"])
            and receipt["scope"] == {"kind": "run", "run_id": receipt["run_id"]}
            and receipt["owner_input_id"] == native_b["input_id"]
            and receipt["recipient_input_ids"] == [native_b["input_id"]]
            and receipt["terminal"] == {"outcome_type": "consumed"}
            and receipt["outcome"]["completion_type"] == "completed"
            and receipt["outcome"]["session_id"] == admission["session_id"]
            and type(receipt["outcome"]["tool_calls"]) is int
            and receipt["outcome"]["tool_calls"] == gateway_business_tool_count(business, messages, b_index, result_index)
            and receipt["outcome"]["text"] == business["answer"],
            "gateway B lacks its own finalized Completed/Consumed business result")
    gateway_controller_recovery(acquisition, business, messages, b_index, directory, receipt)


def gateway_resource_refusal(case, events, resources, directory, prepare_sha):
    refusal = ("Installed Connect resources are unavailable or changed; "
               "reinstall the complete product. No setup command was started.")
    require(case.get("resource_refusal") == refusal
            and refusal.encode() in gateway_evidence(case["gateway_stderr"])
            and isinstance(case.get("connect_error"), dict) and case.get("finalization") == {}
            and not any(event.get("event") in ("fixture_published", "fixture_publish_entered", "native_artifact_command") or
                        (event.get("event") == "sdk_init_event" and event.get("method") == "meta/init_settled"
                         and event["params"].get("outcome") == "ready") for event in events),
            "negative case did not discriminate native resource refusal")
    changed = case["resource_corruption"]
    clone = directory / resources.parent.parent.name
    require(Path(changed["before"]["path"]) == resources / "prepare.py"
            and changed["before"]["sha256"] == prepare_sha
            and Path(changed["after"]["path"]) == clone / "libexec/meerkat-connect/prepare.py"
            and changed["before"]["sha256"] != changed["after"]["sha256"],
            "negative case did not change the selected resource")
    gateway_evidence(changed["before"])
    gateway_evidence(changed["after"])
    starts = [event for event in events if event.get("event") == "main_gateway_started"]
    require(len(starts) == 1 and starts[0]["binary"] == str(clone / "bin/toolkit_gateway"),
            "negative launched another gateway")
    candidate = case["candidate"]
    check_binary(clone / "bin/toolkit_gateway", candidate["binary"]["sha256"],
                 target=candidate["target"], resources_sha=candidate["resources_sha256"])
    pin(clone / "libexec/meerkat-connect/resources.json", candidate["resources_sha256"])
    _, plan = gateway_json(case["preparation_plan"])
    marker = directory / "bundled-entry-entered"
    require(Path(changed["marker"]) == marker and not os.path.lexists(marker)
            and Path(plan["output"]) == directory / "application-preparation"
            and not os.path.lexists(plan["output"]), "negative entered bundled preparation")


def build_record(path, binary, binary_sha, helpers, *, target="macos-arm64",
                 resources_sha=None, binary_kind=None, resource_record_sha=None, stage_only=False,
                 gateway_reference=None, gateway_raw=None):
    resources_sha = resources_sha or TARGETS[target]["resources_sha256"]
    raw, value = read_record(path, helpers)
    require(value["schema_version"] == 1 and value["target"] == target
            and value["resources_sha256"] == resources_sha, "build scope mismatch")
    built = value["binary"]
    require(Path(built["path"]).is_absolute()
            and Path(built["path"]).resolve(strict=True) == binary.resolve(strict=True)
            and built["sha256"] == binary_sha and built["size"] == binary.stat().st_size,
            "build binary mismatch")
    cargo = value["cargo"]
    require(cargo["exit_code"] == 0 and isinstance(cargo["argv"], list)
            and cargo["argv"] and all(isinstance(item, str) for item in cargo["argv"])
            and cargo["env"]["MEERKAT_CONNECT_RESOURCES_SHA256"] == resources_sha,
            "successful resource-bound build required")
    source = value["source_manifest"]
    pin(Path(source["path"]), source["sha256"])
    candidate = {"target": target, "source_manifest_sha256": source["sha256"],
                 "resources_sha256": resources_sha}
    identity = {"sha256": binary_sha, "size": binary.stat().st_size}
    if resource_record_sha is not None:
        require(value.get("binary_kind") == binary_kind
                and value.get("resource_build_record_sha256") == resource_record_sha,
                "explicit build selection mismatch")
        argv = cargo["argv"]
        require(argv.count("--bin") == 1 and argv.index("--bin") + 1 < len(argv)
                and argv[argv.index("--bin") + 1] == binary_kind, "selected build binary kind mismatch")
        candidate.update(binary_kind=binary_kind, binary=identity,
                         resource_build_record_sha256=resource_record_sha)
    else:
        candidate["companion_cli_binary"] = identity
    if stage_only:
        require(resource_record_sha is not None, "staging requires explicit resource selection")
        if binary_kind == "toolkit_gateway":
            return raw, {"candidate": candidate}
    if binary_kind == "toolkit_gateway":
        require(gateway_reference is not None and gateway_raw is not None,
                "final gateway qualification is not available")
        return raw, gateway_verification(gateway_reference, gateway_raw, candidate, raw)
    tested = value["test_executable"]
    check_binary(Path(tested["path"]), tested["sha256"], target=target, resources_sha=resources_sha)
    pin(Path(tested["path"]), tested["sha256"], tested["size"])
    candidate["test_executable"] = {"sha256": tested["sha256"], "size": tested["size"]}
    if stage_only:
        return raw, {"candidate": candidate}
    composition_record(value["installed_composition"], helpers, candidate)
    shutdown_record(value["native_shutdown_review"], helpers, candidate)
    return raw, {"candidate": candidate,
        "installed_composition_sha256": value["installed_composition"]["sha256"],
        "native_shutdown_review_sha256": value["native_shutdown_review"]["sha256"],
        "cli_tty_qualified": False}


def check_binary(binary, expected, *, target="macos-arm64", resources_sha=None):
    require(target in TARGETS, "unsupported assembly target")
    resources_sha = resources_sha or TARGETS[target]["resources_sha256"]
    pin(binary, expected)
    require(32 <= binary.stat().st_size <= 1024 * 1024 * 1024, "binary size outside bound")
    with binary.open("rb") as stream:
        header = stream.read(64)
        if target == "macos-arm64":
            magic, cpu, _subtype, filetype = struct.unpack("<IIII", header[:16])
            require((magic, cpu, filetype) == (0xFEEDFACF, 0x0100000C, 2),
                    "Mac arm64 executable required")
        else:
            require(len(header) == 64 and header[:7] == b"\x7fELF\x02\x01\x01",
                    "Linux ELF64 little-endian executable required")
            kind, machine, version, entry = struct.unpack_from("<HHIQ", header, 16)
            header_size = struct.unpack_from("<H", header, 52)[0]
            require(kind in (2, 3) and machine == 62 and version == 1
                    and entry != 0 and header_size == 64, "Linux x86_64 executable required")
        with mmap.mmap(stream.fileno(), 0, access=mmap.ACCESS_READ) as data:
            # This rejects a mismatch. The reviewed build record owns provenance.
            require(data.find(resources_sha.encode("ascii")) >= 0,
                    "embedded resource digest missing")


def write_new(path, data, mode=0o644):
    with path.open("xb") as stream:
        stream.write(data)
    path.chmod(mode)


def verify_selected_payload(resources, manifest, record):
    if record is None:
        return
    require(manifest["prepare"] == "prepare.py", "selected preparation path changed")
    pin(resources / "prepare.py", record["prepare_source_sha256"])
    library = record["input_selection"]["library"]
    require(manifest["archive"] == "archives/" + Path(library["path"]).name,
            "selected resource library path mismatch")
    pin(resources / manifest["archive"], library["sha256"], library["bytes"])
    starter = record["input_selection"]["starter"]
    require({path.name for path in (resources / "starter").iterdir()} == STARTER_FILES,
            "selected resource starter closure mismatch")
    for name, selected in starter["files"].items():
        pin(resources / "starter" / name, selected["sha256"], selected["bytes"])


def assemble(binary, binary_sha, record_path, destination, *, target="macos-arm64",
             resource_record_path=None, resource_record_sha=None, binary_kind=None, stage_only=False,
             gateway_verification_path=None, gateway_verification_sha=None, release_version=None):
    require(target in TARGETS, "unsupported assembly target")
    require(release_version is None or (isinstance(release_version, str)
            and re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", release_version) is not None),
            "release version must use MAJOR.MINOR.PATCH")
    selected, resource_raw, resource_record = selected_resources(
        target, resource_record_path, resource_record_sha, binary_kind)
    require(not stage_only or resource_raw is not None, "staging requires explicit resource selection")
    supplied_verification = (gateway_verification_path is not None, gateway_verification_sha is not None)
    require(not any(supplied_verification) or (all(supplied_verification)
            and binary_kind == "toolkit_gateway" and resource_raw is not None and not stage_only),
            "gateway verification requires final explicit gateway assembly and both inputs")
    gateway_reference = ({"path": str(gateway_verification_path), "sha256": gateway_verification_sha}
                         if all(supplied_verification) else None)
    gateway_raw = gateway_evidence(gateway_reference) if gateway_reference is not None else None
    resources_sha = selected["resources_sha256"]
    prefix = "meerkat-connect-" + target
    archive_name = ("meerkat-toolkit-gateway-" if binary_kind == "toolkit_gateway"
                    else "meerkat-connect-") + target + ".tar.gz"
    if release_version is not None:
        archive_name = ("meerkat-connect-gateway-" if binary_kind == "toolkit_gateway"
                        else "meerkat-connect-") + release_version + "-" + target + ".tar.gz"
    require(destination.is_absolute() and not destination.exists() and not destination.is_symlink()
            and destination.parent.resolve(strict=True) == destination.parent,
            "fresh destination with canonical parent required")
    if resource_raw is None:
        pin(PREPARE, PREPARE_SHA)
        if target == "linux-x86_64":
            pin(RESOURCE_PACKAGER, RESOURCE_PACKAGER_SHA)
    for name, expected in LICENSES.items():
        pin(ROOT / name, expected)
    helpers = runpy.run_path(str(PREPARE))
    check_binary(binary, binary_sha, target=target, resources_sha=resources_sha)
    record_raw, qualification = build_record(record_path, binary, binary_sha, helpers, target=target,
        resources_sha=resources_sha, binary_kind=binary_kind,
        resource_record_sha=resource_record_sha, stage_only=stage_only,
        gateway_reference=gateway_reference, gateway_raw=gateway_raw)
    if target == "linux-x86_64":
        runpy.run_path(str(RESOURCE_PACKAGER))["require_case_sensitive"](destination.parent)

    with checked_component(selected, target) as (source, entries):
        destination.mkdir(mode=0o755)
        extract_component(source, entries, destination)
    require({path.name for path in destination.iterdir()} == {prefix}, "unexpected component root")
    package = destination / prefix
    resources = package / "libexec/meerkat-connect"
    manifest, observed_sha = helpers["verify_resources"](resources)
    require(manifest["target"] == target and observed_sha == resources_sha
            and resources.stat().st_mode & 0o777 == 0o555
            and (resources / "resources.json").stat().st_mode & 0o777 == 0o444,
            "resource manifest or root modes changed")
    verify_selected_payload(resources, manifest, resource_record)
    require({path.name for path in package.iterdir()} == {"bin", "libexec"}
            and not list((package / "bin").iterdir()), "component bin must be empty")
    supplied_binary = package / "bin" / (binary_kind or "meerkat-connect")
    shutil.copyfile(binary, supplied_binary)
    supplied_binary.chmod(0o755)
    pin(supplied_binary, binary_sha)
    for name, expected in LICENSES.items():
        write_new(package / name, (ROOT / name).read_bytes(), 0o444)
        pin(package / name, expected)
    if stage_only:
        readme = ("# Unqualified Meerkat Connect verification prefix\n\n"
                  "This prefix contains the selected binary and resource bytes for verification.\n"
                  "It is not a qualified release. No packaged program was executed by staging.\n"
                  "Use the existing verifier to observe the actual native owner and resources.\n")
    else:
        template = GATEWAY_README if binary_kind == "toolkit_gateway" else README
        readme = template.format(host=selected["host"], archive=archive_name, prefix=prefix)
    write_new(package / "README.md", readme.encode("utf-8"), 0o444)

    if stage_only:
        _, after_sha = helpers["verify_resources"](resources)
        require(after_sha == resources_sha, "resources changed during staging")
        verify_selected_payload(resources, manifest, resource_record)
        pin(supplied_binary, binary_sha)
        write_new(destination / "build-record.json", record_raw)
        write_new(destination / "resource-build-record.json", resource_raw)
        result = {"schema_version": 1, "scope": "unqualified_product_prefix_for_verification",
                  "target": target, "binary_kind": binary_kind, "prefix": prefix,
                  "candidate": qualification["candidate"],
                  "build_record_sha256": hashlib.sha256(record_raw).hexdigest(),
                  "resource_build_record_sha256": resource_record_sha,
                  "resource_component_sha256": selected["component_sha256"],
                  "assembler_sha256": digest(Path(__file__).resolve())}
        write_new(destination / "staging.json", (json.dumps(result, indent=2) + "\n").encode())
        return result

    partial = destination / (archive_name + ".partial")
    with partial.open("xb") as stream, gzip.GzipFile(filename="", mode="wb", fileobj=stream, mtime=0) as compressed:
        with tarfile.open(fileobj=compressed, mode="w", format=tarfile.PAX_FORMAT) as archive:
            for path in [package, *sorted(package.rglob("*"))]:
                member = archive.gettarinfo(str(path), arcname=path.relative_to(destination).as_posix())
                member.uid = member.gid = 0
                member.uname = member.gname = ""
                member.mtime = 0
                if path.is_file() and not path.is_symlink():
                    with path.open("rb") as payload:
                        archive.addfile(member, payload)
                else:
                    archive.addfile(member)
    _, after_sha = helpers["verify_resources"](resources)
    require(after_sha == resources_sha, "resources changed during assembly")
    pin(supplied_binary, binary_sha)
    output = destination / archive_name
    partial.rename(output)
    archive_sha = digest(output)
    write_new(destination / (archive_name + ".sha256"), f"{archive_sha}  {archive_name}\n".encode("ascii"))
    write_new(destination / "build-record.json", record_raw)
    if resource_raw is not None:
        write_new(destination / "resource-build-record.json", resource_raw)
    if gateway_raw is not None:
        write_new(destination / "gateway-verification.json", gateway_raw)
    result = {"schema_version": 1, "target": target,
        "scope": "conversation_catalog_local_preparation_and_help_preview",
        "not_qualified": ["cli_terminal_interaction", "approval", "component_install", "account_sign_in",
                          "installed_integration_realm_persistence", "frontend_activation", "other_targets"],
        "archive": output.name, "archive_sha256": archive_sha, "archive_bytes": output.stat().st_size,
        "binary_sha256": binary_sha, "binary_bytes": supplied_binary.stat().st_size,
        "resources_sha256": resources_sha, "resource_component_sha256": selected["component_sha256"],
        "build_record_sha256": hashlib.sha256(record_raw).hexdigest(),
        "qualification": qualification,
        "assembler_sha256": digest(Path(__file__).resolve()), "licenses": LICENSES,
        "readme_sha256": digest(package / "README.md"), "resource_closure_unchanged": True}
    if resource_raw is not None:
        result.update(binary_kind=binary_kind, resource_build_record_sha256=resource_record_sha)
    if release_version is not None:
        result["release_version"] = release_version
    if binary_kind == "toolkit_gateway":
        result["scope"] = "installed_gateway_schema4_resource_startup_and_shutdown_preview"
        result["not_qualified"] = ["model_execution", "account_sign_in", "acquisition", "component_install",
                                   "installed_integration_realm_persistence", "frontend_activation", "other_targets"]
    write_new(destination / "assembly.json", (json.dumps(result, indent=2) + "\n").encode())
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__, allow_abbrev=False)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--binary-sha256", required=True)
    parser.add_argument("--build-record", type=Path, required=True)
    parser.add_argument("--destination", type=Path, required=True)
    parser.add_argument("--target", choices=tuple(TARGETS), default="macos-arm64")
    parser.add_argument("--release-version", help="Use versioned public archive names (MAJOR.MINOR.PATCH)")
    parser.add_argument("--resource-build-record", type=Path)
    parser.add_argument("--resource-build-record-sha256")
    parser.add_argument("--binary-kind", choices=BINARY_KINDS)
    parser.add_argument("--stage-only", action="store_true")
    parser.add_argument("--gateway-verification", type=Path)
    parser.add_argument("--gateway-verification-sha256")
    args = parser.parse_args()
    print(json.dumps(assemble(args.binary, args.binary_sha256, args.build_record, args.destination,
                              target=args.target, resource_record_path=args.resource_build_record,
                              resource_record_sha=args.resource_build_record_sha256,
                              binary_kind=args.binary_kind, stage_only=args.stage_only,
                              gateway_verification_path=args.gateway_verification,
                              gateway_verification_sha=args.gateway_verification_sha256,
                              release_version=args.release_version), sort_keys=True))


if __name__ == "__main__":
    main()
