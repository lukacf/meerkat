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
        if "reader_evidence" in current:
            _verify_reused_current(current, Documents())
        else:
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


# This version consumes reviewed build records, not an operational build helper.
# The legacy fresh-native-source schema remains separate above.
READER_EVIDENCE_KEYS = {"schema", "context", "build_bindings", "supervision", "resource_inputs"}
BUILD_BINDING_KEYS = {"build_result", "root_build_review", "peer_build_review", "outer_terminal"}
CURRENT_ONLY_INPUTS = {
    "scripts/fixtures/historical_g_current_binding.py",
    "scripts/fixtures/r5_historical_g_recovery.py",
    "scripts/fixtures/test_historical_g_current_binding_reuse.py",
    "docs/site/guides/host-composition.mdx",
}


def evidence_value(documents, selected):
    require(reference(selected)["size"] <= 16 * 1024 * 1024, "JSON evidence exceeds bound")
    return json.loads(documents.file(selected).read_bytes(), object_pairs_hook=unique)


def inventory(documents, selected):
    value = evidence_value(documents, selected)
    if isinstance(value, list):
        require(all(isinstance(row, dict) and isinstance(row.get("path"), str) for row in value)
            and len({row["path"] for row in value}) == len(value), "unique inventory paths required")
        value = {row["path"]: row for row in value}
    require(isinstance(value, dict) and value, "complete source inventory required")
    result = {}
    for name, row in value.items():
        path = Path(name)
        require(isinstance(name, str) and not path.is_absolute() and ".." not in path.parts
            and name == path.as_posix(), "relative source inventory path required")
        mode = row.get("git_mode", row.get("mode"))
        require(mode in ("100644", "100755") and type(row["size"]) is int and row["size"] >= 0
            and re.fullmatch(r"[0-9a-f]{64}", row["sha256"]), "source inventory identity differs")
        result[name] = {"sha256": row["sha256"], "size": row["size"], "mode": mode}
    return result


def _same_build_selection(observed, selected, output, documents):
    """Admit only the writer's selection and its exact observer case alias."""
    output = Path(output)
    selected_path = documents.file(selected)
    observed_path = documents.file(observed)
    require(output.is_absolute() and output.resolve(strict=True) == output
        and selected_path == output / "selection.json"
        and observed_path in (selected_path, output / "SELECTION.json"),
        "build selection paths differ from the exact writer/observer siblings")
    require(reference(observed)["sha256"] == reference(selected)["sha256"]
        and reference(observed)["size"] == reference(selected)["size"]
        and observed_path.samefile(selected_path), "build selection physical identity differs")


def _verify_reused_current(current, documents):
    evidence = documents.read(current["reader_evidence"])
    require(set(evidence) == READER_EVIDENCE_KEYS
        and evidence["schema"] == "historical-g-current-reader-build-v1",
        "closed current reader evidence required")
    bindings = evidence["build_bindings"]
    require(isinstance(bindings, dict) and set(bindings) == BUILD_BINDING_KEYS
        and same(bindings["build_result"], current["emission"]), "closed actual build bindings required")
    source = documents.read(current["source"])
    context = documents.read(evidence["context"])
    require(same(source["context"], evidence["context"])
        and all(source[key] == context[key] for key in ("source_root", "source_head", "source_tree", "core_head", "meta_head")),
        "build source/context differs")
    command_spec = documents.read(source["command"])
    for row in source["files"]:
        documents.file(row)
    root = Path(context["source_root"])
    require(root.is_absolute() and root.resolve() == root
        and all(re.fullmatch(r"[0-9a-f]{40}", context[key])
            for key in ("source_head", "source_tree", "core_head", "meta_head"))
        and current["core"] == context["core_head"] and current["meta"] == context["meta_head"],
        "exact current source/upstream identities required")
    require(documents.file(current["root_source_review"]).resolve() != documents.file(current["peer_source_review"]).resolve(),
        "distinct source reviewers required")
    for key in ("root_source_review", "peer_source_review"):
        review = documents.read(current[key])
        require(review["status"] == "source_go_unexecuted" and review["findings"] == []
            and same(review["source"], current["source"]), "actual build-source review differs")
    peer_graph = documents.read(context["peer_graph_review"])
    root_graph = documents.read(context["root_graph_review"])
    require(documents.file(context["root_graph_review"]).resolve() != documents.file(context["peer_graph_review"]).resolve()
        and peer_graph["status"] == "actual_final_path_graph_verified" and peer_graph["findings"] == []
        and all(peer_graph[key] == context[key] for key in GRAPH_FIELDS)
        and root_graph["status"] == "root_graph_go_for_development_build" and root_graph["findings"] == []
        and same(root_graph["source_commit"], context["source_commit"])
        and same(root_graph["metadata_result"], context["metadata_result"])
        and root_graph["exact_graph_equal_after_candidate_root_relocation"] is True,
        "actual current graph reviews differ")
    documents.file(context["source_commit"])
    documents.file(context["cargo_manifest_inventory"])
    metadata = documents.read(context["metadata"])
    metadata_result = documents.read(context["metadata_result"])
    metadata_command = documents.read(metadata_result["command"])
    require(metadata_result["status"] == "actual_locked_metadata_complete"
        and all(metadata_result[key] == context[key] for key in ("source_root", "source_head", "source_tree"))
        and same(metadata_result["metadata"], context["metadata"])
        and metadata_command["exit_code"] == 0 and metadata_command["timed_out"] is False
        and metadata_command["cwd"] == str(root), "actual current metadata terminal differs")
    documents.file(metadata_command["stdout"])
    documents.file(metadata_command["stderr"])
    require(same(metadata_command["stdout"], context["metadata"]), "raw metadata bytes differ")
    for key, relative, digest in (("native_manifest", "native-candidate/Cargo.toml", "manifest_sha256"),
                                  ("native_lock", "native-candidate/Cargo.lock", "lock_sha256")):
        require(documents.file(current[key]) == root / relative
            and reference(current[key])["sha256"] == context[digest], "current manifest/lock differs")

    result = documents.read(bindings["build_result"])
    selected = documents.read(result["selection"])
    emitted = documents.read(result["build"])
    require(result["status"] == "passed" and same(emitted["selection"], result["selection"])
        and same(selected["source"], current["source"]) and same(selected["context"], evidence["context"])
        and all(selected[key] == context[key] for key in ("source_root", "source_head", "source_tree"))
        and selected["core"] == current["core"] and selected["meta"] == current["meta"]
        and same(selected["manifest"], current["native_manifest"]) and same(selected["lockfile"], current["native_lock"])
        and same(selected["metadata"], context["metadata"])
        and same(selected["graph_root"], context["root_graph_review"])
        and same(selected["graph_peer"], context["peer_graph_review"]), "actual current build selection differs")
    root_review = documents.read(bindings["root_build_review"])
    peer_review = documents.read(bindings["peer_build_review"])
    require(documents.file(bindings["root_build_review"]).resolve() != documents.file(bindings["peer_build_review"]).resolve()
        and set(peer_review) == {"status", "source_head", "source", "result", "root_review", "findings"}
        and same(peer_review["root_review"], bindings["root_build_review"]), "distinct six-field build peer required")
    for review in (root_review, peer_review):
        require(review["status"] == "terminal_pass_reviewed" and review["findings"] == []
            and review["source_head"] == context["source_head"]
            and same(review["source"], current["source"]) and same(review["result"], bindings["build_result"]),
            "successful current build reviews differ")
    inputs = documents.read(selected["inputs"])
    admission = documents.read(selected["admission"])
    require(same(inputs["source"], current["source"])
        and same(inputs["root_source_review"], current["root_source_review"])
        and same(inputs["peer_source_review"], current["peer_source_review"])
        and admission["status"] == "actual_retention_build_admitted" and admission["findings"] == []
        and admission["lane_idle"] is True and same(admission["source"], current["source"])
        and same(admission["inputs"], selected["inputs"]), "actual build admission differs")
    documents.file(admission["idle_evidence"])
    outer = documents.read(bindings["outer_terminal"])
    require(outer["status"] == "terminal" and type(outer["exit_code"]) is int and outer["exit_code"] == 0
        and outer["direct_child_reaped"] is True and type(outer["pid"]) is int and outer["pid"] > 0
        and outer["deadline_expired"] is False and outer["received_signals"] == []
        and outer["primary_failure"] is None and outer["cooperative_interrupt_sent"] is False
        and outer["cwd"] == str(root)
        and outer["argv"][-4:] == ["--inputs-sha256", reference(selected["inputs"])["sha256"],
            "--admission-sha256", reference(selected["admission"])["sha256"]]
        and str(documents.file(selected["runner"])) in outer["argv"], "actual build outer terminal differs")
    documents.file(outer["stdout"]); documents.file(outer["stderr"])
    supervision = documents.read(evidence["supervision"])
    require(supervision["status"] == "passed" and supervision["exit_code"] == 0
        and same(supervision["source"], current["source"]) and same(supervision["inputs"], selected["inputs"])
        and supervision["output"] == inputs["output"] == admission["output"]
        and Path(reference(bindings["build_result"])["path"]).parent == Path(inputs["output"])
        and supervision["first_signal"] is None and supervision["signal_counts"] == {}
        and supervision["restore_errors"] == [] and supervision["exception_class"] is None,
        "actual build owner supervision differs")
    cargo = emitted["cargo"]
    require(cargo["exit_code"] == 0 and cargo["timed_out"] is False
        and "failure" not in cargo and "cleanup" not in cargo and cargo["cwd"] == str(root)
        and cargo["argv"] == selected["argv"] == admission["argv"] == command_spec["argv"]
        and command_spec["cwd"] == str(root)
        and admission["cwd"] == str(root)
        and cargo["env"]["MEERKAT_CONNECT_RESOURCES_SHA256"] == current["resources_sha256"],
        "actual compiler command/terminal differs")
    argv = cargo["argv"]
    require(argv.count("--manifest-path") == 1 and argv[argv.index("--manifest-path") + 1] == str(root / "native-candidate/Cargo.toml")
        and all(flag in argv for flag in ("test", "--locked", "--offline", "--no-run"))
        and argv.count("--config") == 1 and argv[argv.index("--config") + 1] == str(documents.file(context["cargo_policy"]))
        and same(selected["cargo_policy"], context["cargo_policy"]), "actual compiler invocation/policy differs")
    child = documents.read(emitted["direct_child"])
    command = documents.read(child["command"])
    _same_build_selection(child["selection"], result["selection"], inputs["output"], documents)
    require(command == cargo and child["schema"] == "actual-recorder-child-observation-v1"
        and child["command_name"] == "build"
        and same(child["log"], cargo["log"]) and child["expected_argv"] == child["actual_argv"] == argv
        and child["cwd"] == str(root) and child["wait_hook_observed"] is True
        and child["owner_run_finished"] is True and child["owner_returned_normally"] is True
        and child["owner_exception_class"] is None and type(child["returncode"]) is int and child["returncode"] == 0
        and type(child["child_pid"]) is int and child["child_pid"] > 0 and child["direct_child_reaped"] is True,
        "actual compiler child/reap differs")
    postflight = documents.read(result["postflight"])
    require(postflight["status"] == "passed" and postflight["census"]["forbidden_processes"] == []
        and same(postflight["source"], current["source"]) and same(postflight["inputs"], selected["inputs"]),
        "actual build postflight differs")
    _verify_reader_artifacts(current, context, emitted, metadata, documents)
    _verify_reader_resources(current, context, selected, evidence, documents)


def _verify_reader_artifacts(current, context, emitted, metadata, documents):
    def package(name):
        rows = [row for row in metadata["packages"] if row["name"] == name]
        require(len(rows) == 1 and rows[0]["source"] is None, "one current path package required")
        return rows[0]
    events = [json.loads(line, object_pairs_hook=unique)
        for line in documents.file(emitted["cargo"]["log"]).read_text().splitlines() if line.startswith("{")]
    finishes = [row for row in events if row.get("reason") == "build-finished"]
    require(len(finishes) == 1 and finishes[0]["success"] is True, "one successful compiler completion required")
    targets = {"meerkat_connect": ("meerkat-connect", "lib", "meerkat_connect"),
        "toolkit_mobkit": ("toolkit-mobkit", "lib", "toolkit_mobkit"),
        "meerkat_connect_bin": ("meerkat-connect", "bin", "meerkat-connect")}
    require(set(emitted["binaries"]) == set(targets), "all three emitted test binaries required")
    for key, (name, kind, target) in targets.items():
        binary = emitted["binaries"][key]; expected = package(name)
        require(str(documents.file(binary["binary"])) == binary["path"]
            and binary["package_id"] == expected["id"] and binary["manifest_path"] == expected["manifest_path"]
            and Path(binary["manifest_path"]) == Path(context["source_root"]) / "crates" / name / "Cargo.toml"
            and binary["target"]["kind"] == [kind] and binary["target"]["name"] == target,
            "actual current emitted target differs")
        artifacts = [row for row in events if row.get("reason") == "compiler-artifact"
            and row.get("executable") == binary["path"] and row.get("package_id") == binary["package_id"]
            and row.get("manifest_path") == binary["manifest_path"] and row.get("target") == binary["target"]
            and row.get("profile", {}).get("test") is True]
        require(len(artifacts) == 1, "exact compiler artifact event absent or ambiguous")
    require(same(emitted["binaries"]["meerkat_connect"]["binary"], current["executable"]),
        "selected reader is not the emitted current Connect library")
    core, meta = emitted["core_dependency"], emitted["meta_dependency"]
    require(core["revision"] == current["core"] and meta["revision"] == current["meta"]
        and set(core["packages"]) == set(CORE_PACKAGES), "compiled upstream tuple differs")
    for name, row in [*core["packages"].items(), ("meerkat-meta", meta)]:
        expected = package(name)
        require(row["package_id"] == expected["id"] and str(documents.file(row["manifest"])) == expected["manifest_path"],
            "compiled upstream package differs")
    core_root = Path(package("meerkat-core")["manifest_path"]).parents[2]
    expected_sources = {"crates/meerkat-runtime/src/meerkat_machine/session_management.rs",
        "crates/meerkat-mob/src/runtime/session_service.rs"}
    require(set(core["sources"]) == expected_sources, "compiled Core source witnesses differ")
    for name, row in core["sources"].items():
        require(documents.file(row) == core_root / name, "compiled Core source differs")
    meta_root = Path(package("meerkat-meta")["manifest_path"]).parents[2]
    require(documents.file(meta["gateway"]) == meta_root / "crates/meerkat-meta/src/rpc_gateway.rs",
        "compiled Meta source differs")


def _verify_reader_resources(current, context, selected, evidence, documents):
    prior = documents.read(context["prior_inputs"])
    reuse = documents.read(context["reuse"])
    require(reuse["status"] == "source_delta_allows_reuse_of_unchanged_components_only"
        and same(reuse["current_commit"], context["source_commit"])
        and same(reuse["current_inventory"], context["source_inventory"])
        and prior["core_head"] == current["core"] and prior["meta_head"] == current["meta"]
        and same(prior["composition"], context["composition"]), "retained component context differs")
    old = inventory(documents, reuse["prior_inventory"])
    new = inventory(documents, context["source_inventory"])
    changed = {name for name in set(old) | set(new) if old.get(name) != new.get(name)}
    require(changed == set(reuse["changed_paths"]), "declared component delta differs")
    # Native Rust is compiled anew. Only the exact reader/test/guide paths
    # above may be current-only inputs. Actual component input maps below must
    # prove that none is consumed; no blanket directory exemption is permitted.
    require(all((name.startswith("crates/") and name.endswith(".rs")) or name in CURRENT_ONLY_INPUTS
        for name in changed), "changed non-native component requires fresh resources")
    composition = documents.read(context["composition"])
    resource_inputs = documents.read(evidence["resource_inputs"])
    resource = documents.read(current["resource_receipt"])
    peer = documents.read(context["resource_peer_review"])
    require(same(resource["source_payload_review"], peer["source_payload_review"]),
        "retained resource payload reviews differ")
    payload = documents.read(resource["source_payload_review"])
    require(payload["status"] == "source_payloads_verified"
        and payload["source_head"] == prior["source_head"] and payload["source_tree"] == prior["source_tree"]
        and same(payload["build"], context["resource_build"])
        and same(payload["manifest"], current["resource_manifest"])
        and any(same(row, evidence["resource_inputs"]) for row in payload["files"]),
        "resource input map was not part of the actual retained payload review")
    require(composition["source_head"] == prior["source_head"]
        and composition["core_head"] == current["core"] and composition["meta_head"] == current["meta"]
        and resource_inputs["source_root"] == prior["source_root"]
        and resource_inputs["source_tree"] == prior["source_tree"]
        and resource_inputs["connect_source"] == prior["source_head"]
        and same(resource_inputs["expected_sdk_files"], composition["sdk_expected_files"]),
        "original component execution identity differs")
    expected_sdk = documents.read(composition["sdk_expected_files"])["connect"]
    require(expected_sdk["repository"] == prior["source_root"] and expected_sdk["revision"] == prior["source_head"],
        "retained SDK source identity differs")
    producers = resource_inputs["producer_sources"]
    protected = set(producers) | set(expected_sdk["files"])
    require(producers and expected_sdk["files"] and not (protected & CURRENT_ONLY_INPUTS)
        and not (changed & protected), "changed reader or native input is consumed by retained components")
    for name in protected:
        require(name in old and name in new and old[name] == new[name], "retained component source differs")
        physical = documents.file({"path": str(Path(context["source_root"]) / name),
            "sha256": new[name]["sha256"], "size": new[name]["size"]})
        require(bool(physical.stat().st_mode & 0o111) == (new[name]["mode"] == "100755"),
            "retained component executable mode differs")
    for name, row in producers.items():
        require(documents.file(row) == Path(prior["source_root"]) / name
            and reference(row)["sha256"] == old[name]["sha256"]
            and reference(row)["size"] == old[name]["size"], "original producer source differs")
    for name, row in expected_sdk["files"].items():
        require({key: row[key] for key in ("sha256", "size", "mode")} == old[name], "SDK source inventory differs")
    require(same(reuse["prior_resources"], current["resource_receipt"])
        and same(composition["resource_review"], current["resource_receipt"])
        and documents.file(current["resource_receipt"]).resolve() != documents.file(context["resource_peer_review"]).resolve(),
        "original resource review associations differ")
    for review in (resource, peer):
        require(review["status"] == "actual_fresh_resource_composition_verified" and review["findings"] == []
            and review["source_head"] == prior["source_head"] and same(review["build"], context["resource_build"])
            and same(review["manifest"], current["resource_manifest"]), "retained resource reviews differ")
    require(resource["source_tree"] == prior["source_tree"]
        and resource["full_resource_tree_verified"] is True and resource["archive_matches_resource_tree"] is True
        and same(resource["resource_locator"], current["resource_executable"])
        and same(selected["resource_build"], context["resource_build"])
        and same(selected["resource_manifest"], current["resource_manifest"])
        and selected["resources_sha256"] == current["resources_sha256"], "selected retained resources differ")
    build = documents.read(context["resource_build"])
    require(build["resources_sha256"] == current["resources_sha256"]
        and reference(current["resource_manifest"])["sha256"] == current["resources_sha256"]
        and Path(build["resources"]) / "resources.json" == documents.file(current["resource_manifest"]),
        "retained resource build differs")
    documents.file(current["resource_executable"])
    run = documents.read(resource["run"])
    require(run["exit_code"] == 0 and run["timed_out"] is False and run["cwd"] == prior["source_root"]
        and run["source_head"] == prior["source_head"]
        and run["argv"][2] == str(Path(prior["source_root"]) / "scripts/package-connect-resources.py"),
        "original resource terminal differs")
