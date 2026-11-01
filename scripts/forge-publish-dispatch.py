#!/usr/bin/env python3
"""Build, and optionally dispatch, a Forge publication from checked records.

Run it with python3 -I from a checkout that has the candidate's source
commit. Each input file is private and named with the SHA-256 its owner
reported:

- Forge's candidate review projection (forge-candidate-review-v1);
- Connect's A5, application and Guide join records.

Every file's full bytes must match their SHA-256 before the file is parsed.
The tool reads the candidate run and its manifest artifact through gh, and
builds the identity-only selection that forge-release.py's selection command
emits. It then checks:

- the projection, with the publish job's own parser: strict, accepted, naming
  this candidate, its package map equal to the candidate manifest's, and its
  join digests equal to the digests of the actual join files;
- each join record by its kind (JOIN_KINDS): accepted, for its kind's one
  agreed target, and naming this candidate's identity and subjects. The A5
  owner join has no candidate block, so its component identities are compared
  with the candidate's own release inputs instead. An unknown kind is refused,
  and every join must cover the same target.

The request is derived only from these checked bytes; the caller supplies
paths and expected digests, never request values. --dry-run prints the
request. --dispatch also starts it, from main, and only while main's head is
--merged-sha.

Residual: a repository writer can still dispatch forge-release.yml by hand.
The publish job then checks only the projection's own bytes against the
selection and the candidate manifest. The join records behind the projection
are checked here and nowhere else.
"""
from __future__ import annotations

import argparse
import base64
import importlib.util
import json
from pathlib import Path, PurePosixPath
import sys

ROOT = Path(__file__).resolve().parents[1]
MAX_RECORD_BYTES = 1024 * 1024
WORKFLOW_FILE = "forge-release.yml"
CONNECT_OWNER = "connect-native-mac"


def load_forge():
    spec = importlib.util.spec_from_file_location("forge_release_for_dispatch", ROOT / "scripts" / "forge-release.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


forge = load_forge()
rc, require, ProtocolError = forge.rc, forge.require, forge.ProtocolError


def checked_bytes(path, expected, label, limit=MAX_RECORD_BYTES):
    """A regular file's exact bytes, which must match the expected SHA-256 before anything parses them."""
    rc.digest(expected, f"{label} expected SHA-256")
    path = Path(path)
    require(path.is_file() and not path.is_symlink(), f"{label} is not a regular file")
    data = path.read_bytes()
    require(0 < len(data) <= limit, f"{label} is empty or larger than {limit} bytes")
    require(rc.hash_bytes(data) == expected, f"{label} bytes differ from their expected SHA-256")
    return data


def strict_json(data, label):
    try:
        return rc.parse_json(data.decode("utf-8"))
    except (UnicodeDecodeError, ValueError):
        raise ProtocolError(f"{label} is not strict JSON") from None


def text(value, label):
    require(isinstance(value, str) and value, f"{label} must be a nonempty string")
    return value


def git_bytes(sha, path):
    """One file of the candidate's source commit, from this checkout's git objects."""
    return rc.run(["git", "-C", str(ROOT), "show", f"{rc.digest(sha, 'source SHA', 40)}:{path}"])


def candidate(run_id, attempt, work):
    """The candidate run, its verified manifest and the selection forge-release.py emits for it."""
    rc.integer(run_id, "run ID")
    rc.integer(attempt, "run attempt")
    sha = rc.digest(rc.api(rc.repo_endpoint(f"actions/runs/{run_id}")).get("head_sha"), "candidate SHA", 40)
    run_info = rc.verify_run(run_id, attempt, sha)
    name = forge.manifest_artifact_name(attempt)
    item = forge.candidate_artifacts(run_id, [name])[name]
    require(isinstance(item.get("digest"), str) and item["digest"].startswith("sha256:"),
            f"manifest artifact {name} has no digest")
    identity = {"id": item["id"], "name": name, "digest": rc.digest(item["digest"].removeprefix("sha256:"))}
    packed = rc.download_artifact(identity, run_info, work)
    folder = rc.unpack_artifact(packed, work / "manifest", [forge.MANIFEST, forge.MANIFEST_PROVENANCE])
    manifest_bytes = (folder / forge.MANIFEST).read_bytes()
    manifest = forge.validate_manifest(strict_json(manifest_bytes, "candidate manifest"))
    require(manifest["source_sha"] == sha and manifest["run_id"] == run_id and manifest["run_attempt"] == attempt
            and manifest["workflow"]["ref"] == forge.MAIN, "only a main candidate can be published")
    selection = forge.validate_selection({
        "schema_version": 1, "kind": "forge-accepted-candidate", "repository": rc.repository(),
        "run_id": run_id, "run_attempt": attempt,
        "manifest_artifact": {"id": identity["id"], "digest": identity["digest"],
                              "manifest_sha256": rc.hash_bytes(manifest_bytes)}})
    return selection, manifest


def release_inputs(manifest):
    """The candidate's own release inputs, whose bytes the candidate manifest pins."""
    data = git_bytes(manifest["source_sha"], forge.INPUTS)
    require(rc.hash_bytes(data) == manifest["inputs_sha256"], "the checkout's candidate inputs differ from the manifest's")
    return forge.validate_inputs(strict_json(data, "candidate release inputs"))


# --- Connect join records, by kind -------------------------------------------------------------


def check_candidate_join(record, slot, context):
    """connect_application_candidate_join and connect_guide_candidate_join: Connect's exact contract."""
    rc.exact_keys(record, "schema_version kind disposition owner target candidate subjects evidence scope limitations",
                  f"{slot} record")
    require(record["schema_version"] == 1 and type(record["schema_version"]) is int, f"{slot} schema_version must be 1")
    require(record["owner"] == CONNECT_OWNER, f"{slot} names another owner")
    target = record["target"]
    packages = forge.manifest_packages(context["manifest"])
    require(target in packages, f"{slot} names a target the candidate did not build")
    candidate_block = record["candidate"]
    rc.exact_keys(candidate_block, "run_id run_attempt manifest_sha256", f"{slot} candidate")
    rc.integer(candidate_block["run_id"], f"{slot} run ID")
    rc.integer(candidate_block["run_attempt"], f"{slot} run attempt")
    rc.digest(candidate_block["manifest_sha256"], f"{slot} manifest SHA-256")
    selection = context["selection"]
    require((candidate_block["run_id"], candidate_block["run_attempt"], candidate_block["manifest_sha256"])
            == (selection["run_id"], selection["run_attempt"], selection["manifest_artifact"]["manifest_sha256"]),
            f"{slot} accepts another candidate")
    subjects = record["subjects"]
    rc.exact_keys(subjects, "package_sha256 release_inputs_sha256", f"{slot} subjects")
    rc.digest(subjects["package_sha256"], f"{slot} package SHA-256")
    rc.digest(subjects["release_inputs_sha256"], f"{slot} release inputs SHA-256")
    require(subjects["package_sha256"] == packages[target], f"{slot} accepts another {target} package")
    require(subjects["release_inputs_sha256"] == context["manifest"]["inputs_sha256"],
            f"{slot} accepts other release inputs")
    evidence = record["evidence"]
    require(isinstance(evidence, list) and evidence, f"{slot} evidence must be a nonempty list")
    for item in evidence:
        rc.exact_keys(item, "role path sha256 size_bytes", f"{slot} evidence")
        text(item["role"], f"{slot} evidence role")
        text(item["path"], f"{slot} evidence path")
        rc.digest(item["sha256"], f"{slot} evidence SHA-256")
        rc.integer(item["size_bytes"], f"{slot} evidence size")
    text(record["scope"], f"{slot} scope")
    require(isinstance(record["limitations"], list) and all(isinstance(item, str) for item in record["limitations"]),
            f"{slot} limitations must be a list of strings")


def check_a5_join(record, slot, context):
    """connect_a5_owner_join: preserved as issued, with no candidate block. Its component identities
    (provider program, A1 gateway, resources, collection program) must be the candidate's inputs."""
    require("candidate" not in record, f"{slot} carries a candidate block its kind does not define")
    require(record.get("schema_version") == 1 and type(record.get("schema_version")) is int,
            f"{slot} schema_version must be 1")
    require(record.get("owner") == CONNECT_OWNER, f"{slot} names another owner")
    target = record.get("target")
    inputs = context["inputs"]["targets"].get(target) if isinstance(target, str) else None
    require(inputs is not None and "native_programs" in inputs and set(inputs["native_programs"]) >= {"provider", "collection"},
            f"{slot} names a target without candidate provider and collection programs")
    files, programs, paths = inputs["files"], inputs["native_programs"], inputs["paths"]

    def same(pin, value, keys, label):
        require(isinstance(value, dict) and all(value.get(theirs) == pin.get(ours) for ours, theirs in keys),
                f"{slot} {label} differs from the candidate's inputs")

    provider = record.get("provider") or {}
    same(files[programs["provider"]["executable"]], provider.get("executable"),
         [(key, key) for key in ("url", "archive_size", "archive_sha256", "member", "size", "sha256")], "provider program")
    same(files[programs["provider"]["evidence"]], provider.get("evidence"),
         [(key, key) for key in ("url", "size", "sha256")], "provider evidence")
    assembly_pin = files[paths["assembly"]]
    require("path" in assembly_pin, "the candidate's assembly is not a checked-in original")
    assembly_bytes = git_bytes(context["manifest"]["source_sha"], assembly_pin["path"])
    require((len(assembly_bytes), rc.hash_bytes(assembly_bytes)) == (assembly_pin["size"], assembly_pin["sha256"]),
            "the checkout's candidate assembly differs from its pin")
    assembly = strict_json(assembly_bytes, "candidate assembly")
    a1 = record.get("a1_gateway") or {}
    archive_name = str(PurePosixPath(paths["assembly"]).parent / str(assembly.get("archive")))
    require(archive_name in files, "the candidate's inputs do not pin its assembly's gateway archive")
    same(files[archive_name], a1.get("archive"), [("url", "url"), ("size", "size_bytes"), ("sha256", "sha256")],
         "A1 gateway archive")
    same({"path": f"meerkat-connect-{target}/bin/toolkit_gateway", "sha256": assembly["binary_sha256"],
          "size": assembly["binary_bytes"]}, a1.get("member"), [(key, key) for key in ("path", "sha256", "size")],
         "A1 gateway program")
    require(record.get("resource_manifest_sha256") == assembly["resources_sha256"],
            f"{slot} resources differ from the candidate's assembly")
    collection = record.get("collection_public_assets")
    names = programs["collection"]
    require(isinstance(collection, list) and sorted(item.get("key") for item in collection if isinstance(item, dict))
            == sorted(names.values()), f"{slot} collection assets differ from the candidate's collection program")
    for item in collection:
        pin = files[item["key"]]  # every key is one of the collection program's pinned names (checked above)
        keys = ([("url", "url"), ("archive_size", "bytes"), ("archive_sha256", "sha256")] if "member" in pin
                else [("url", "url"), ("size", "bytes"), ("sha256", "sha256")])
        same(pin, item, keys, f"collection asset {item['key']}")


# Each join slot's accepted record kinds; extend when Connect defines another kind.
# Each join slot's accepted record kinds, with the one target each kind's contract fixes for
# this selected tuple (Connect 884e273e, Forge f010d2fe). Extend when Connect defines another
# kind; a multi-target kind has to be an explicit new entry.
JOIN_KINDS = {
    "a5_join": {"connect_a5_owner_join": {"check": check_a5_join, "target": "linux-x86_64"}},
    "application_join": {"connect_application_candidate_join": {"check": check_candidate_join, "target": "linux-x86_64"}},
    "guide_join": {"connect_guide_candidate_join": {"check": check_candidate_join, "target": "linux-x86_64"}},
}


def check_join(slot, data, context):
    """Checks one join record by its kind and returns the target it covers."""
    record = strict_json(data, f"{slot} record")
    require(isinstance(record, dict), f"{slot} record is not an object")
    kinds = JOIN_KINDS[slot]
    require(record.get("kind") in kinds, f"{slot} record kind {record.get('kind')!r} is not one this tool checks")
    require(record.get("disposition") == "accepted", f"{slot} record is not accepted")
    kind = kinds[record["kind"]]
    require(record.get("target") == kind["target"], f"{slot} record must cover {kind['target']}")
    kind["check"](record, slot, context)
    return record["target"]


# --- The request ----------------------------------------------------------------------------------


def build_request(args, work):
    projection = checked_bytes(args.projection, args.projection_sha256, "candidate review projection",
                               forge.MAX_REVIEW_BYTES)
    joins = {slot: checked_bytes(getattr(args, slot), getattr(args, f"{slot}_sha256"), f"{slot} record")
             for slot in forge.REVIEW_JOINS}
    selection, manifest = candidate(args.run_id, args.run_attempt, work)
    review = forge.parse_candidate_review(projection, args.projection_sha256)
    forge.review_names_selection(review, selection)
    require(review["candidate"]["packages"] == forge.manifest_packages(manifest),
            "the candidate review accepts other package digests than the candidate manifest's")
    for slot, data in joins.items():
        require(review["joins"][slot] == rc.hash_bytes(data), f"the candidate review covers another {slot} record")
    context = {"selection": selection, "manifest": manifest, "inputs": release_inputs(manifest)}
    targets = {slot: check_join(slot, data, context) for slot, data in joins.items()}
    require(len(set(targets.values())) == 1, "the joins cover different targets")
    return {"workflow": WORKFLOW_FILE, "repository": rc.repository(), "ref": "main", "inputs": {
        "release_mode": "publish",
        "artifact_selection": json.dumps(selection, sort_keys=True, separators=(",", ":")),
        "candidate_review": base64.b64encode(projection).decode("ascii"),
        "candidate_review_sha256": args.projection_sha256}}


def require_main_head(merged_sha):
    head = rc.api(rc.repo_endpoint("branches/main"))["commit"]["sha"]
    require(head == rc.digest(merged_sha, "merged SHA", 40), f"main's head is {head}, not the merged {merged_sha}")


def dispatch(request, merged_sha):
    require_main_head(merged_sha)
    argv = ["gh", "workflow", "run", request["workflow"], "-R", request["repository"], "--ref", request["ref"]]
    for key, value in request["inputs"].items():
        argv += ["-f", f"{key}={value}"]
    rc.run(argv)


def parse_args(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--run-id", type=int, required=True)
    parser.add_argument("--run-attempt", type=int, required=True)
    parser.add_argument("--projection", required=True)
    parser.add_argument("--projection-sha256", required=True)
    for slot in forge.REVIEW_JOINS:
        parser.add_argument(f"--{slot.replace('_', '-')}", dest=slot, required=True)
        parser.add_argument(f"--{slot.replace('_', '-')}-sha256", dest=f"{slot}_sha256", required=True)
    parser.add_argument("--merged-sha", required=True)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--dry-run", action="store_true")
    mode.add_argument("--dispatch", action="store_true")
    return parser.parse_args(argv)


def main(argv=None):
    args = parse_args(argv)
    try:
        require_main_head(args.merged_sha)
        with rc.workspace() as work:
            request = build_request(args, work)
        if args.dispatch:
            dispatch(request, args.merged_sha)
            print(f"dispatched {WORKFLOW_FILE} publish from main {args.merged_sha}; "
                  f"candidate review {request['inputs']['candidate_review_sha256']}")
        else:
            print(json.dumps(request, indent=2, sort_keys=True))
    except ProtocolError as error:
        print(f"REFUSED: {error}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main())
