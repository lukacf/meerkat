"""Read-only current-artifact joins for the historical G fixture.

This checks existing source/review/emission records. It does not run a gate,
produce an acceptance receipt, inspect native stores or establish authority.
"""
from pathlib import Path
import hashlib
import json
import re


class CurrentBindingRefused(ValueError):
    pass


def require(value, message):
    if not value:
        raise CurrentBindingRefused(message)


def reference(row):
    require(isinstance(row, dict) and set(row) in (
        {"path", "sha256", "size"}, {"path", "sha256", "bytes"}), "closed file pin required")
    size = row.get("size", row.get("bytes"))
    require(isinstance(row["path"], str) and Path(row["path"]).is_absolute()
        and isinstance(row["sha256"], str) and re.fullmatch(r"[0-9a-f]{64}", row["sha256"])
        and type(size) is int and size >= 0, "invalid file pin")
    return {"path": row["path"], "sha256": row["sha256"], "size": size}


def same(left, right):
    return reference(left) == reference(right)


def unique(pairs):
    result = {}
    for key, value in pairs:
        require(key not in result, "duplicate JSON key")
        result[key] = value
    return result


class Documents:
    def __init__(self):
        self.pins = {}

    def file(self, row):
        row = reference(row)
        path = Path(row["path"])
        require(path.is_file() and not path.is_symlink() and path.stat().st_size == row["size"],
            "selected file type/size differs")
        with path.open("rb") as stream:
            require(hashlib.file_digest(stream, "sha256").hexdigest() == row["sha256"],
                "selected file bytes differ")
        require(str(path) not in self.pins or self.pins[str(path)] == row,
            "conflicting selected file pins")
        self.pins[str(path)] = row
        return path

    def read(self, row):
        require(reference(row)["size"] <= 16 * 1024 * 1024, "JSON evidence exceeds bound")
        value = json.loads(self.file(row).read_bytes(), object_pairs_hook=unique)
        require(isinstance(value, dict), "JSON evidence must be an object")
        return value


SOURCE_FIELDS = ("source_root", "source_head", "source_tree", "source_inventory",
    "cargo_manifest_inventory", "definition_inventory", "manifest_sha256", "lock_sha256",
    "core_head", "meta_head", "metadata", "metadata_result", "historical_native_check")
GRAPH_FIELDS = tuple(key for key in SOURCE_FIELDS
    if key not in ("definition_inventory", "historical_native_check"))
CORE_PACKAGES = ("meerkat-core", "meerkat-runtime", "meerkat-session", "meerkat-mob")


def verify_current(current):
    """Refuse mismatched existing records; return no approval or runtime result."""
    try:
        _verify_current(current, Documents())
    except (KeyError, TypeError, OSError, json.JSONDecodeError) as error:
        raise CurrentBindingRefused("malformed or unavailable current artifact evidence") from error


def _verify_current(current, documents):
    source = documents.read(current["source"])
    candidate = documents.read(source["candidate"])
    require(candidate["schema"] == "connect-local-development-native-source-v1"
        and candidate["status"] == "local_development_native_source_reviewed",
        "reviewed current native source binding required")
    require(all(source[key] == candidate[key] for key in ("source_root", "source_head", "source_tree"))
        and current["core"] == candidate["core_head"] and current["meta"] == candidate["meta_head"],
        "current source or tuple differs")
    root = Path(candidate["source_root"])
    require(root.is_absolute() and all(re.fullmatch(r"[0-9a-f]{40}", candidate[key])
        for key in ("source_head", "source_tree", "core_head", "meta_head")), "exact Git identities required")
    for key in ("root_source_review", "peer_source_review"):
        review = documents.read(current[key])
        require(review["status"] in ("source_go", "source_go_unexecuted") and review["findings"] == []
            and same(review["source"], current["source"])
            and all(review[field] == candidate[field]
                for field in ("source_head", "source_tree", "core_head", "meta_head")),
            "prepared native source review differs")
    for key, status, fields in (
        ("root_source_review", ("source_go", "source_go_unexecuted"), SOURCE_FIELDS),
        ("independent_source_review", ("source_go", "source_go_unexecuted"), SOURCE_FIELDS),
        ("graph_root_review", ("actual_final_path_graph_verified",), GRAPH_FIELDS),
        ("graph_peer_review", ("actual_final_path_graph_verified",), GRAPH_FIELDS),
    ):
        review = documents.read(candidate[key])
        require(review["status"] in status and review["findings"] == []
            and all(review[field] == candidate[field] for field in fields), "current source/graph review differs")
    for key in ("source_inventory", "cargo_manifest_inventory", "definition_inventory", "historical_native_check"):
        documents.file(candidate[key])
    metadata = documents.read(candidate["metadata"])
    metadata_run = documents.read(candidate["metadata_result"])
    require(metadata_run["exit_code"] == 0 and metadata_run["timed_out"] is False
        and metadata_run["cwd"] == str(root) and metadata_run["source_head"] == candidate["source_head"]
        and metadata_run["source_tree"] == candidate["source_tree"]
        and same(metadata_run["metadata"], candidate["metadata"]), "actual metadata record differs")

    def package(name):
        rows = [row for row in metadata["packages"] if row["name"] == name]
        require(len(rows) == 1 and rows[0]["source"] is None, "one current path package required")
        return rows[0]

    for key, relative, digest in (("native_manifest", "native-candidate/Cargo.toml", "manifest_sha256"),
                                  ("native_lock", "native-candidate/Cargo.lock", "lock_sha256")):
        path = documents.file(current[key])
        require(path == root / relative and reference(current[key])["sha256"] == candidate[digest],
            "current native manifest/lock differs")
    result = documents.read(current["emission"])
    require(result["status"] == "passed", "actual successful current emission required")
    selected = documents.read(result["selection"])
    emitted = documents.read(result["build"])
    require(same(emitted["selection"], result["selection"])
        and all(selected[key] == candidate[key] for key in ("source_root", "source_head", "source_tree"))
        and same(selected["manifest"], current["native_manifest"])
        and same(selected["lockfile"], current["native_lock"])
        and same(selected["resource_manifest"], current["resource_manifest"])
        and selected["resources_sha256"] == current["resources_sha256"], "emission selection differs")
    binary = emitted["binaries"]["meerkat_connect"]
    current_package = package("meerkat-connect")
    require(same(binary["binary"], current["executable"])
        and binary["path"] == reference(current["executable"])["path"]
        and binary["package_id"] == current_package["id"]
        and binary["manifest_path"] == current_package["manifest_path"] == str(root / "crates/meerkat-connect/Cargo.toml")
        and binary["target"]["kind"] == ["lib"] and binary["target"]["name"] == "meerkat_connect",
        "actual current emitted Connect library differs")
    documents.file(current["executable"])
    core = emitted["core_dependency"]
    meta = emitted["meta_dependency"]
    require(core["revision"] == current["core"] and meta["revision"] == current["meta"]
        and set(core["packages"]) == set(CORE_PACKAGES), "emitted upstream tuple differs")
    for name, row in [*core["packages"].items(), ("meerkat-meta", meta)]:
        expected = package(name)
        require(row["package_id"] == expected["id"]
            and str(documents.file(row["manifest"])) == expected["manifest_path"],
            "emitted upstream package differs from reviewed metadata")
    cargo = emitted["cargo"]
    require(cargo["exit_code"] == 0 and cargo["timed_out"] is False and "failure" not in cargo
        and cargo["cwd"] == str(root) and cargo["env"]["MEERKAT_CONNECT_RESOURCES_SHA256"] == current["resources_sha256"],
        "actual compiler terminal differs")
    argv = cargo["argv"]
    require(argv.count("--manifest-path") == 1
        and argv[argv.index("--manifest-path") + 1] == str(root / "native-candidate/Cargo.toml")
        and all(flag in argv for flag in ("test", "--locked", "--offline", "--no-run")), "compiler invocation differs")
    events = [json.loads(line, object_pairs_hook=unique)
        for line in documents.file(cargo["log"]).read_text().splitlines() if line.startswith("{")]
    require(any(row.get("reason") == "build-finished" and row.get("success") is True for row in events),
        "successful compiler event absent")
    artifacts = [row for row in events if row.get("reason") == "compiler-artifact"
        and row.get("executable") == binary["path"] and row.get("package_id") == binary["package_id"]
        and row.get("manifest_path") == binary["manifest_path"] and row.get("target") == binary["target"]
        and row.get("profile", {}).get("test") is True]
    require(len(artifacts) == 1, "exact compiler artifact event absent or ambiguous")
    resource = documents.read(current["resource_receipt"])
    require(resource["status"] == "actual_fresh_resource_composition_verified"
        and resource["findings"] == [] and resource["source_head"] == candidate["source_head"]
        and resource["source_tree"] == candidate["source_tree"]
        and same(resource["build"], selected["resource_build"])
        and same(resource["manifest"], current["resource_manifest"])
        and same(resource["resource_locator"], current["resource_executable"])
        and resource["full_resource_tree_verified"] is True and resource["archive_matches_resource_tree"] is True,
        "actual current resource receipt differs")
    build = documents.read(resource["build"])
    require(build["resources_sha256"] == current["resources_sha256"]
        and Path(build["resources"]) / "resources.json" == documents.file(current["resource_manifest"]),
        "actual resource build differs")
    documents.file(current["resource_executable"])
    run = documents.read(resource["run"])
    require(run["exit_code"] == 0 and run["cwd"] == str(root)
        and run["source_head"] == candidate["source_head"]
        and run["argv"][2] == str(root / "scripts/package-connect-resources.py"), "resource run differs")
    # These joins consume previously reviewed records. They neither rerun the
    # source/graph/resource verifiers nor replace their required execution gates.
