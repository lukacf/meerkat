"""Setup projection of ComposeRuntime's image acquisition and proposal owner."""
from __future__ import annotations

import copy
import hashlib
import os
from pathlib import Path
import re
import stat

SCOPE = "acquire_agent_selected_container_images"
HEX = re.compile(r"[0-9a-f]{64}\Z")
REQUEST_ID = re.compile(r"[0-9a-f]{32}\Z")
IDENTIFIER = re.compile(r"[A-Za-z0-9][A-Za-z0-9_.-]{0,127}\Z")
INPUT_LIMIT = 2 * 1024 * 1024
TOOL_LIMIT = 128 * 1024 * 1024
OUTPUT_LIMIT = 8 * 1024 * 1024
FILES = {"host-profile.json", "selection.json", "backend-profile.json", "inputs.json",
         "request.json", "prepared.json", "image-acquisition.json"}


def sha(raw):
    return hashlib.sha256(raw).hexdigest()


def _inventory(path, limit):
    result = {}
    fd = os.open(path, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        with os.scandir(fd) as entries:
            for entry in entries:
                if len(result) >= limit:
                    raise ValueError()
                mode = entry.stat(follow_symlinks=False).st_mode
                if not (stat.S_ISREG(mode) or stat.S_ISDIR(mode)):
                    raise ValueError()
                result[entry.name] = "directory" if stat.S_ISDIR(mode) else "file"
    finally:
        os.close(fd)
    return result


def _proposal(io, directory, selection_sha256, host_sha256, output=None):
    io.canonical_directory(directory)
    receipt_raw = io.read_file(directory, "proposal.json", OUTPUT_LIMIT)
    receipt = io.decode(receipt_raw)
    io.keys(receipt, ("schema_version", "status", "scope", "plan_sha256", "files"))
    if ((output is not None and io.decode(output) != receipt) or type(receipt["schema_version"]) is not int
            or receipt["schema_version"] != 1 or receipt["status"] != "proposed"
            or receipt["scope"] != "compose_plan_for_host_review"
            or not isinstance(receipt["plan_sha256"], str) or not HEX.fullmatch(receipt["plan_sha256"])
            or not isinstance(receipt["files"], dict) or set(receipt["files"]) != FILES):
        raise ValueError()
    if output is None:
        entries = _inventory(directory, len(FILES) + 3)
        optional = {name: kind for name, kind in ((".lock", "file"), (".docker", "directory")) if name in entries}
        if entries != {**{name: "file" for name in FILES | {"proposal.json"}}, **optional}:
            raise ValueError()
        if ".lock" in optional:
            io.read_file(directory, ".lock", 1024)
        if ".docker" in optional:
            if (_inventory(directory / ".docker", 1) != {"config.json": "file"}
                    or io.read_file(directory, ".docker/config.json", 1024) != b"{}\n"):
                raise ValueError()
    records = {}
    for name, expected in receipt["files"].items():
        raw = io.read_file(directory, name, OUTPUT_LIMIT)
        if not isinstance(expected, str) or not HEX.fullmatch(expected) or sha(raw) != expected:
            raise ValueError()
        if name in ("prepared.json", "image-acquisition.json"):
            records[name] = io.decode(raw)
    prepared, acquired = records["prepared.json"], records["image-acquisition.json"]
    if (type(prepared.get("schema_version")) is not int or prepared["schema_version"] != 1
            or prepared.get("status") != "prepared" or prepared.get("plan_sha256") != receipt["plan_sha256"]
            or not isinstance(prepared.get("plan"), dict)
            or type(acquired.get("schema_version")) is not int or acquired["schema_version"] != 1
            or acquired.get("scope") != "registry_image_acquisition_only" or acquired.get("status") != "observed"
            or acquired.get("plan_sha256") != receipt["plan_sha256"]
            or acquired.get("host_profile_sha256") != host_sha256 or acquired.get("selection_sha256") != selection_sha256):
        raise ValueError()
    if (receipt["files"]["selection.json"] != selection_sha256
            or receipt["files"]["host-profile.json"] != host_sha256
            or io.read_file(directory, "proposal.json", OUTPUT_LIMIT) != receipt_raw):
        raise ValueError()
    return {"scope": "compose_plan_for_host_review", "proposal_directory": str(directory),
            "receipt_sha256": sha(receipt_raw), "proposal": receipt}


def readback(io, request, job_directory):
    """Read the final proposal without inspecting or changing the current engine.

    The caller verifies the durable request digest and owns exclusive custody.
    Image availability, runtime liveness and historical execution stay unknown.
    """
    try:
        request = copy.deepcopy(request)
        args = request["arguments"]
        io.keys(args, ("request_id", "staged_id", "selection_sha256"))
        for name, regex in (("request_id", REQUEST_ID), ("staged_id", IDENTIFIER), ("selection_sha256", HEX)):
            if not isinstance(args[name], str) or not regex.fullmatch(args[name]):
                raise ValueError()
        workspace, job = Path(request["workspace"]), Path(job_directory)
        if (request["family"] != "selected-compose" or not REQUEST_ID.fullmatch(request["job_id"])
                or job != workspace / "acquisitions" / request["job_id"]):
            raise ValueError()
        io.canonical_directory(job)
        assets = workspace / "compose-acquisition"
        io.canonical_directory(assets)
        pins = request["host"]["assets"]
        io.keys(pins, ("profile.json", "runtime.py", "planner", "host-profile.json"))
        for name, expected in pins.items():
            if not isinstance(expected, str) or not HEX.fullmatch(expected) or sha(io.read_file(assets, name, TOOL_LIMIT)) != expected:
                raise ValueError()
        profile = io.decode(io.read_file(assets, "profile.json", INPUT_LIMIT))
        if (pins["profile.json"] != request["host"]["profile_sha256"] or profile["scope"] != SCOPE
                or profile["staging_root"] != request["staging_root"] or profile["runtime"]["sha256"] != pins["runtime.py"]
                or profile["planner"]["sha256"] != pins["planner"] or profile["host_profile"]["sha256"] != pins["host-profile.json"]
                or sha(io.read_file(job, "selection.json", INPUT_LIMIT)) != args["selection_sha256"]):
            raise ValueError()
        return _proposal(io, job / "proposal", args["selection_sha256"], pins["host-profile.json"])
    except Exception:
        raise io.Error("compose_acquisition_readback_unconfirmed", 409) from None


class ComposeAcquisition:
    def __init__(self, io, profile_path, profile_sha, python_path):
        self.io = io
        self.workspace = None
        self.execution_uncertain = False
        try:
            self.profile_path = Path(profile_path)
            self.profile_sha = profile_sha
            self.python = Path(python_path)
            if not isinstance(profile_sha, str) or not HEX.fullmatch(profile_sha):
                raise ValueError()
            self.profile_raw = self.read(self.profile_path, INPUT_LIMIT)
            if sha(self.profile_raw) != profile_sha:
                raise ValueError()
            host = io.decode(self.profile_raw)
            io.keys(host, ("schema_version", "scope", "staging_root", "python_sha256",
                           "runtime", "planner", "host_profile", "operation_timeout_seconds"))
            if (type(host["schema_version"]) is not int or host["schema_version"] != 1
                    or host["scope"] != SCOPE or type(host["operation_timeout_seconds"]) is not int
                    or not 1 <= host["operation_timeout_seconds"] <= 3600):
                raise ValueError()
            self.staging = Path(host["staging_root"])
            if not self.staging.is_absolute() or str(self.staging) != host["staging_root"]:
                raise ValueError()
            io.canonical_directory(self.staging)
            self.sources = {"python": self.read(self.python, TOOL_LIMIT)}
            if sha(self.sources["python"]) != host["python_sha256"] or not self.python.stat().st_mode & 0o111:
                raise ValueError()
            for name in ("runtime", "planner", "host_profile"):
                item = host[name]
                io.keys(item, ("path", "sha256"))
                if not isinstance(item["sha256"], str) or not HEX.fullmatch(item["sha256"]):
                    raise ValueError()
                self.sources[name] = self.read(Path(item["path"]), TOOL_LIMIT if name != "host_profile" else INPUT_LIMIT)
                if sha(self.sources[name]) != item["sha256"]:
                    raise ValueError()
            self.host = copy.deepcopy(host)
        except (OSError, ValueError, TypeError, KeyError, AttributeError, io.Error):
            raise io.Error("invalid_compose_acquisition_profile") from None

    def read(self, path, limit):
        if not path.is_absolute() or str(path) != str(path.resolve(strict=True)):
            raise ValueError()
        return self.io.read_file(self.io.canonical_directory(path.parent), path.name, limit)

    def bind(self, workspace):
        self.io.canonical_directory(workspace)
        if workspace == self.staging or workspace in self.staging.parents or self.staging in workspace.parents:
            raise self.io.Error("invalid_compose_acquisition_workspace")
        self.workspace = workspace
        self.assets = workspace / "compose-acquisition"
        self.assets.mkdir(mode=0o700)
        self.frozen = {"profile.json": self.profile_raw, "runtime.py": self.sources["runtime"],
                       "planner": self.sources["planner"], "host-profile.json": self.sources["host_profile"]}
        for name, raw in self.frozen.items():
            self.io.write_new(self.assets / name, raw)
            (self.assets / name).chmod(0o500 if name == "planner" else 0o400)

    def verify(self):
        try:
            if self.read(self.profile_path, INPUT_LIMIT) != self.profile_raw:
                raise ValueError()
            if self.read(self.python, TOOL_LIMIT) != self.sources["python"]:
                raise ValueError()
            for name in ("runtime", "planner", "host_profile"):
                if self.read(Path(self.host[name]["path"]), TOOL_LIMIT) != self.sources[name]:
                    raise ValueError()
            for name, raw in self.frozen.items():
                if self.io.read_file(self.assets, name, TOOL_LIMIT) != raw:
                    raise ValueError()
        except (OSError, ValueError, self.io.Error):
            raise self.io.Error("compose_acquisition_host_changed", 409) from None

    def summary(self):
        return {"available": True, "scope": SCOPE, "staging_root": str(self.staging),
                "host_profile_sha256": self.profile_sha, "execution_uncertain": self.execution_uncertain,
                "timeout_seconds": self.host["operation_timeout_seconds"]}

    def arguments(self, request):
        try:
            self.io.keys(request, ("request_id", "staged_id", "selection_sha256"))
            for name, regex in (("request_id", REQUEST_ID), ("staged_id", IDENTIFIER), ("selection_sha256", HEX)):
                if not isinstance(request[name], str) or not regex.fullmatch(request[name]):
                    raise ValueError()
        except (ValueError, TypeError, KeyError, self.io.Error):
            raise self.io.Error("invalid_compose_acquisition_arguments") from None
        return copy.deepcopy(request)

    def run(self, request, directory, run, check_stop):
        if self.execution_uncertain:
            raise self.io.Error("compose_acquisition_reopen_required", 409)
        self.verify()
        check_stop()
        raw = self.io.read_file(self.staging, request["staged_id"] + "/selection.json", INPUT_LIMIT)
        if sha(raw) != request["selection_sha256"]:
            raise self.io.Error("compose_acquisition_selection_changed", 409)
        path = directory / "selection.json"
        self.io.write_new(path, raw)
        proposal = directory / "proposal"
        argv = [str(self.python), "-I", "-B", str(self.assets / "runtime.py"),
                "--workspace", str(proposal), "--planner", str(self.assets / "planner"),
                "--planner-sha256", self.host["planner"]["sha256"], "propose", "--acquire-images",
                "--host-profile", str(self.assets / "host-profile.json"),
                "--host-profile-sha256", self.host["host_profile"]["sha256"]]
        try:
            # Only the existing runtime may interpret the graph or image facts.
            # Stop first asks it to cancel/join its own child; forced loss leaves
            # this session uncertain and requires fresh host custody.
            code, output = run(argv, input_file=path, input_sha256=sha(raw), output_limit=OUTPUT_LIMIT,
                               timeout=self.host["operation_timeout_seconds"], termination_grace=10)
            check_stop()
            self.verify()
            if code:
                raise self.io.Error("compose_acquisition_unconfirmed", 409)
            try:
                result = _proposal(self.io, proposal, sha(raw), self.host["host_profile"]["sha256"], output)
            except Exception:
                raise self.io.Error("compose_acquisition_result_invalid", 409) from None
            check_stop()
            return result
        except BaseException:
            self.execution_uncertain = True
            raise
