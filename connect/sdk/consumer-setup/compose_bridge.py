"""Optional setup projection of the canonical, host-pinned Compose backend.

The backend owns its retained request, resource identities and lifecycle facts.
This bridge owns only host/input bindings and live-session dispatch correlation.
"""
from __future__ import annotations

import copy
import hashlib
import json
from pathlib import Path
import re


SCOPE = "execute_agent_selected_local_container_plan"
RESULT_SCOPE = "compose_backend_observation_only"
BINDING_SCOPE = "compose_backend_stdio"
ACTIONS = ("prepare", "start", "observe", "resume", "stop", "remove",
           "prepare_successor", "adopt_successor", "reconcile_successor", "rebind")
SUCCESSOR_ACTIONS = frozenset(("prepare_successor", "adopt_successor", "reconcile_successor", "rebind"))
INPUT_LIMIT = 2 * 1024 * 1024
PLAN_LIMIT = 8 * 1024 * 1024 + 1
TOOL_LIMIT = 128 * 1024 * 1024
HEX = re.compile(r"[0-9a-f]{64}\Z")
IDENTIFIER = re.compile(r"[A-Za-z0-9][A-Za-z0-9_.-]{0,127}\Z")
COMPONENT_ID = re.compile(r"[A-Za-z0-9][A-Za-z0-9._/:@-]{0,191}\Z")
REQUEST_ID = re.compile(r"[0-9a-f]{32}\Z")
# Only these fixed CLI codes may cross the setup boundary. They describe an
# observed failure, not whether effects occurred or whether retry is safe.
RUNTIME_ERRORS = frozenset((
    "invalid_json", "invalid_retained_path", "invalid_retained_file",
    "changed_retained_file", "retained_file_unavailable", "invalid_tool_binding",
    "tool_binding_changed", "invalid_bind_source_prefix", "command_output_limit",
    "workspace_unavailable", "operation_in_progress", "request_limit",
    "planning_failed", "invalid_planner_reply", "workspace_exists",
    "invalid_planner_files", "plan_changed", "inputs_changed", "record_changed",
    "docker_config_changed", "engine_observation_unavailable",
    "engine_observation_invalid", "engine_changed", "image_changed",
    "image_volume_unbound", "resource_mismatch", "resource_replaced",
    "foreign_network_member", "foreign_volume_user", "operation_unconfirmed",
    "project_not_absent", "resources_incomplete", "resources_not_stopped",
    "stale_observation", "deployment_not_started", "request_in_progress",
    "resources_disappeared", "invalid_state_disposition", "effect_unconfirmed",
    "compose_unavailable", "compose_interrupted",
    "custody_changed", "successor_unconfirmed", "successor_planning_failed",
    "invalid_successor", "successor_unavailable", "successor_limit",
    "successor_resources_unconfirmed", "successor_already_retained",
    "successor_state_changed", "no_pending_successor", "successor_effect_unresolved",
))
RUNTIME_DIAGNOSTICS = {(code + "\n").encode("ascii"): code for code in RUNTIME_ERRORS}


def component_id(value):
    # toolkit-package::Id syntax, distinct from local staging/request IDs.
    return (isinstance(value, str) and COMPONENT_ID.fullmatch(value) is not None and
            all(part not in ("", ".", "..") for part in value.split("/")))


def sha(raw):
    return hashlib.sha256(raw).hexdigest()


def wire(value):
    return (json.dumps(value, sort_keys=True, ensure_ascii=False, separators=(",", ":"),
                       allow_nan=False) + "\n").encode("utf-8")


class ComposeBridge:
    runtime_errors = RUNTIME_ERRORS

    def __init__(self, io, path, expected, setup_workspace):
        self.io = io
        self.plan = None
        self.successor_plan = None
        self.selection_state = None
        self.transition_sha256 = None
        self.execution_uncertain = False
        self.runner = None
        self.intents = {}
        self.session = setup_workspace / "compose"
        self.host_path = path
        self.host_sha256 = expected
        self.sources = {}
        self.source_refs = {}
        try:
            if not isinstance(expected, str) or not HEX.fullmatch(expected):
                raise ValueError()
            self.host_raw = self.read_path(path, 64 * 1024)
            if sha(self.host_raw) != expected:
                raise ValueError()
            host = io.decode(self.host_raw)
            io.keys(host, ("schema_version", "scope", "staging_root", "graph_workspace", "mode",
                           "expected_plan_sha256", "python", "runtime", "planner", "backend_profile",
                           "allowed_actions", "delete_state_namespaces", "operation_timeout_seconds",
                           *(("successor",) if "successor" in host else ())))
            if (type(host["schema_version"]) is not int or host["schema_version"] != 1 or
                    host["scope"] != SCOPE or host["mode"] not in ("new", "reopen") or
                    type(host["operation_timeout_seconds"]) is not int or
                    not 1 <= host["operation_timeout_seconds"] <= 3600):
                raise ValueError()
            actions = host["allowed_actions"]
            if (not isinstance(actions, list) or not actions or
                    any(type(item) is not str or item not in ACTIONS for item in actions) or
                    len(set(actions)) != len(actions)):
                raise ValueError()
            self.graph = self.directory_path(host["graph_workspace"], exists=host["mode"] == "reopen")
            self.staging = self.directory_path(host["staging_root"], exists=True)
            roots = (self.graph, self.staging, setup_workspace)
            if any(a == b or a in b.parents or b in a.parents
                   for i, a in enumerate(roots) for b in roots[i + 1:]):
                raise ValueError()
            if host["mode"] == "new":
                if host["expected_plan_sha256"] is not None or self.graph.exists() or self.graph.is_symlink():
                    raise ValueError()
            elif (not isinstance(host["expected_plan_sha256"], str) or
                  not HEX.fullmatch(host["expected_plan_sha256"]) or "prepare" in actions):
                raise ValueError()
            for name in ("python", "runtime", "planner", "backend_profile"):
                item = host[name]
                io.keys(item, ("path", "sha256"))
                if not isinstance(item["sha256"], str) or not HEX.fullmatch(item["sha256"]):
                    raise ValueError()
                source = Path(item["path"])
                raw = self.read_path(source, 256 * 1024 if name == "backend_profile" else TOOL_LIMIT)
                if sha(raw) != item["sha256"]:
                    raise ValueError()
                if name in ("python", "planner") and not source.stat().st_mode & 0o111:
                    raise ValueError()
                self.sources[name] = raw
                self.source_refs[name] = item
            successor = host.get("successor")
            if successor is not None:
                io.keys(successor, ("inputs", "backend_profile", "predecessor_plan_sha256", "transition_sha256"))
                if (host["mode"] != "reopen" or
                        successor["predecessor_plan_sha256"] != host["expected_plan_sha256"] or
                        (successor["transition_sha256"] is not None and
                         (not isinstance(successor["transition_sha256"], str) or
                          not HEX.fullmatch(successor["transition_sha256"])))):
                    raise ValueError()
                for name, key, limit in (("successor_inputs", "inputs", INPUT_LIMIT),
                                         ("successor_profile", "backend_profile", 256 * 1024)):
                    item = successor[key]
                    io.keys(item, ("path", "sha256"))
                    if not isinstance(item["sha256"], str) or not HEX.fullmatch(item["sha256"]):
                        raise ValueError()
                    raw = self.read_path(item["path"], limit)
                    if sha(raw) != item["sha256"]:
                        raise ValueError()
                    self.sources[name], self.source_refs[name] = raw, item
                inputs = io.decode(self.sources["successor_inputs"])
                io.keys(inputs, ("schema_version", "package_json", "manifest_json", "configuration", "policy", "files"))
                self.successor_raw = wire({**inputs,
                    "profile_json": self.sources["successor_profile"].decode("utf-8"),
                    "profile_sha256": successor["backend_profile"]["sha256"]})
                if len(self.successor_raw) > INPUT_LIMIT:
                    raise ValueError()
                self.transition_sha256 = successor["transition_sha256"]
            elif "successor" in host or set(actions) & SUCCESSOR_ACTIONS:
                raise ValueError()
            self.profile = io.decode(self.sources["backend_profile"])
            deleted = host["delete_state_namespaces"]
            if (not isinstance(deleted, list) or len(deleted) > 16 or
                    any(not component_id(s) for s in deleted) or
                    len(set(deleted)) != len(deleted) or
                    (host["mode"] == "new" and not set(deleted) <= self.profile.get("volumes", {}).keys())):
                raise ValueError()
            self.host = copy.deepcopy(host)
        except (OSError, ValueError, TypeError, KeyError, AttributeError, io.Error):
            raise io.Error("invalid_compose_profile") from None

    def directory_path(self, value, *, exists):
        if not isinstance(value, str) or not value or any(ord(c) < 32 for c in value):
            raise ValueError()
        path = Path(value)
        if not path.is_absolute() or str(path) != value:
            raise ValueError()
        self.io.canonical_directory(path if exists else path.parent)
        return path

    def read_path(self, path, limit):
        if not isinstance(path, Path):
            path = Path(path)
        if not path.is_absolute() or str(path) != str(path.resolve(strict=True)):
            raise ValueError()
        return self.io.read_file(self.io.canonical_directory(path.parent), path.name, limit)

    def bind(self, runner):
        self.runner = runner
        self.session.mkdir(mode=0o700)
        for name, raw in {"host.json": self.host_raw, "backend-profile.json": self.sources["backend_profile"],
                          "runtime.py": self.sources["runtime"], "planner": self.sources["planner"]}.items():
            self.io.write_new(self.session / name, raw)
        if self.host.get("successor"):
            self.io.write_new(self.session / "successor-inputs.json", self.sources["successor_inputs"])
            self.io.write_new(self.session / "successor-profile.json", self.sources["successor_profile"])
            self.io.write_new(self.session / "successor-request.json", self.successor_raw)
        (self.session / "planner").chmod(0o700)
        if self.host["mode"] == "reopen":
            self.retained()

    def verify(self):
        try:
            if self.read_path(self.host_path, 64 * 1024) != self.host_raw:
                raise ValueError()
            for name, raw in self.sources.items():
                if self.read_path(Path(self.source_refs[name]["path"]), TOOL_LIMIT) != raw:
                    raise ValueError()
            if self.host.get("successor"):
                for name, raw in (("successor-inputs.json", self.sources["successor_inputs"]),
                                  ("successor-profile.json", self.sources["successor_profile"]),
                                  ("successor-request.json", self.successor_raw)):
                    if self.io.read_file(self.session, name, INPUT_LIMIT) != raw:
                        raise ValueError()
            for name, raw in {"host.json": self.host_raw, "backend-profile.json": self.sources["backend_profile"],
                              "runtime.py": self.sources["runtime"], "planner": self.sources["planner"]}.items():
                if self.io.read_file(self.session, name, TOOL_LIMIT) != raw:
                    raise ValueError()
        except (OSError, ValueError, self.io.Error):
            raise self.io.Error("compose_host_changed", 409) from None

    def summary(self):
        value = {"available": True, "scope": SCOPE, "actions": list(self.host["allowed_actions"]),
                "mode": self.host["mode"], "graph_workspace": str(self.graph), "staging_root": str(self.staging),
                "host_profile_sha256": self.host_sha256,
                "backend_profile_sha256": self.plan["summary"]["profile_sha256"] if self.plan else self.host["backend_profile"]["sha256"],
                "plan_sha256": self.plan["plan_sha256"] if self.plan else None,
                "execution_uncertain": self.execution_uncertain,
                "delete_state_namespaces": list(self.host["delete_state_namespaces"]),
                "operation_timeout_seconds": self.host["operation_timeout_seconds"]}
        if self.host.get("successor"):
            successor = self.host["successor"]
            value["successor"] = {"inputs_sha256": successor["inputs"]["sha256"],
                "backend_profile_sha256": successor["backend_profile"]["sha256"],
                "predecessor_plan_sha256": successor["predecessor_plan_sha256"],
                "transition_sha256": self.transition_sha256,
                "successor_plan_sha256": self.successor_plan["plan_sha256"] if self.successor_plan else None}
        value["selected_plan_sha256"] = self.selection_state["selected"]["plan_sha256"] if self.selection_state else None
        value["pending_operation"] = self.selection_state["pending_operation"] if self.selection_state else None
        value["pending_transition"] = copy.deepcopy(self.selection_state["pending_transition"]) if self.selection_state else None
        value["rebind_required"] = bool(self.selection_state and self.selection_state["selected"] != self.plan)
        return value

    def admit(self, action):
        if action not in self.host["allowed_actions"]:
            raise self.io.Error("compose_action_not_allowed", 403)
        if self.execution_uncertain:
            raise self.io.Error("compose_host_reopen_required", 409)

    def arguments(self, action, request):
        wanted = {"request_id"} if action != "observe" else set()
        if action == "prepare":
            wanted |= {"staged_id", "inputs_sha256"}
        elif action == "prepare_successor":
            wanted |= {"predecessor_plan_sha256", "inputs_sha256"}
        elif action == "adopt_successor":
            wanted |= {"predecessor_plan_sha256", "transition_sha256", "observation_sha256"}
        elif action == "reconcile_successor":
            wanted |= {"transition_sha256", "observation_sha256"}
        elif action == "rebind":
            wanted |= {"plan_sha256", "transition_sha256"}
        elif action != "observe":
            wanted.add("plan_sha256")
        if action in ("resume", "stop", "remove"):
            wanted.add("observation_sha256")
        if action == "remove":
            wanted.add("delete_state")
        self.io.keys(request, wanted)
        for key, value in request.items():
            if key == "delete_state":
                valid = (isinstance(value, list) and len(value) <= 16 and
                         all(isinstance(x, str) for x in value) and len(set(value)) == len(value) and
                         set(value) <= set(self.host["delete_state_namespaces"]))
            elif not isinstance(value, str):
                valid = False
            elif key == "request_id":
                valid = REQUEST_ID.fullmatch(value)
            elif key == "staged_id":
                valid = IDENTIFIER.fullmatch(value)
            else:
                valid = HEX.fullmatch(value)
            if not valid:
                raise self.io.Error("invalid_compose_arguments")
        return copy.deepcopy(request)

    def check_profile(self, request):
        if (not isinstance(request, dict) or request.get("profile_json") != self.sources["backend_profile"].decode("utf-8") or
                request.get("profile_sha256") != self.host["backend_profile"]["sha256"]):
            raise self.io.Error("compose_profile_changed", 409)

    def preflight(self, path, raw):
        self.verify()
        code, output = self.runner.run_compose([str(self.session / "planner")],
            input_file=path, input_sha256=sha(raw), output_limit=PLAN_LIMIT,
            timeout=min(120, self.host["operation_timeout_seconds"]))
        self.verify()
        if code:
            raise self.io.Error("compose_plan_rejected", 409)
        value = self.io.decode(output)
        self.prepared(value, raw, self.host["backend_profile"]["sha256"])
        return value

    def prepared(self, value, raw=None, profile=None):
        if (not isinstance(value, dict) or type(value.get("schema_version")) is not int or value["schema_version"] != 1 or
                value.get("status") != "prepared" or not isinstance(value.get("plan_sha256"), str) or
                not HEX.fullmatch(value["plan_sha256"]) or not isinstance(value.get("plan"), dict)):
            raise self.io.Error("compose_result_invalid", 409)
        plan = value["plan"]
        if ((raw is not None and plan.get("request_sha256") != sha(raw)) or
                (profile is not None and plan.get("profile_sha256") != profile) or
                not isinstance(plan.get("request_components"), list) or len(plan["request_components"]) > 16 or
                any(not component_id(s) for s in plan["request_components"])):
            raise self.io.Error("compose_result_invalid", 409)

    def freeze(self, request):
        if self.plan is not None or self.graph.exists() or self.graph.is_symlink():
            raise self.io.Error("compose_graph_exists", 409)
        self.verify()
        raw = self.io.read_file(self.staging, request["staged_id"] + "/inputs.json", INPUT_LIMIT)
        if sha(raw) != request["inputs_sha256"]:
            raise self.io.Error("compose_inputs_changed", 409)
        inputs = self.io.decode(raw)
        self.io.keys(inputs, ("schema_version", "package_json", "manifest_json", "configuration", "policy", "files"))
        assembled = {**inputs, "profile_json": self.sources["backend_profile"].decode("utf-8"),
                     "profile_sha256": self.host["backend_profile"]["sha256"]}
        data = wire(assembled)
        if len(data) > INPUT_LIMIT:
            raise self.io.Error("compose_input_limit", 409)
        path = self.session / ("prepare-" + request["request_id"] + ".json")
        self.io.write_new(path, data)
        return path, data

    def command(self, action):
        return [self.host["python"]["path"], "-I", "-B", str(self.session / "runtime.py"),
                "--workspace", str(self.graph), "--planner", str(self.session / "planner"),
                "--planner-sha256", self.host["planner"]["sha256"], action]

    def runtime(self, args, *, path=None, raw=None, limit=PLAN_LIMIT):
        code, output, diagnostic = self.runner.run_compose_diagnostic(args, input_file=path,
            input_sha256=sha(raw) if raw else None, output_limit=limit,
            timeout=self.host["operation_timeout_seconds"])
        self.verify()
        if code:
            error = self.io.Error("compose_operation_unconfirmed", 409)
            if type(code) is int and code == 1 and output == b"":
                observed = RUNTIME_DIAGNOSTICS.get(diagnostic)
                if observed is not None:
                    error.runtime_error = observed
            raise error
        return self.io.decode(output)

    def canonical_summary(self, value):
        self.prepared(value)
        return {**self.prepared_summary(value), "status": "prepared_summary"}

    def validate_summary(self, value, raw=None, profile=None):
        self.io.keys(value, ("schema_version", "status", "plan_sha256", "summary"))
        if (type(value["schema_version"]) is not int or value["schema_version"] != 1 or
                value["status"] != "prepared_summary" or not isinstance(value["plan_sha256"], str) or
                not HEX.fullmatch(value["plan_sha256"])):
            raise self.io.Error("compose_result_invalid", 409)
        summary = value["summary"]
        self.io.keys(summary, ("request_sha256", "package_sha256", "manifest_sha256", "manifest_source_sha256",
            "profile_sha256", "configuration_sha256", "policy_sha256", "host_owner", "project",
            "initialization_order", "shutdown_order", "deployment_order", "request_components", "components", "files"))
        if (any(not isinstance(summary[key], str) or not HEX.fullmatch(summary[key]) for key in
                ("request_sha256", "package_sha256", "manifest_sha256", "manifest_source_sha256",
                 "profile_sha256", "configuration_sha256", "policy_sha256")) or
                raw is not None and summary["request_sha256"] != sha(raw) or
                profile is not None and summary["profile_sha256"] != profile):
            raise self.io.Error("compose_result_invalid", 409)
        for key in ("initialization_order", "shutdown_order", "deployment_order", "request_components"):
            if (not isinstance(summary[key], list) or len(summary[key]) > 16 or
                    any(not component_id(item) for item in summary[key])):
                raise self.io.Error("compose_result_invalid", 409)
        if not isinstance(summary["components"], dict) or not isinstance(summary["files"], dict):
            raise self.io.Error("compose_result_invalid", 409)
        for item in summary["components"].values():
            self.io.keys(item, ("service", "lifetime", "image", "image_id", "platform", "dependencies",
                               "deployment_dependencies", "readiness", "stop_timeout_ms"))
        for item in summary["files"].values():
            self.io.keys(item, ("sha256", "mode"))

    def proposal(self, value):
        successor = self.host.get("successor")
        if (not successor or not isinstance(value, dict) or
                set(value) != {"schema_version", "status", "transition_sha256", "predecessor", "successor"} or
                type(value["schema_version"]) is not int or value["schema_version"] not in (1, 2) or
                value["status"] != "prepared_successor_summary" or
                not isinstance(value["transition_sha256"], str) or not HEX.fullmatch(value["transition_sha256"]) or
                self.transition_sha256 is not None and value["transition_sha256"] != self.transition_sha256):
            raise self.io.Error("compose_result_invalid", 409)
        self.validate_summary(value["predecessor"], profile=self.host["backend_profile"]["sha256"])
        if value["predecessor"]["plan_sha256"] != successor["predecessor_plan_sha256"]:
            raise self.io.Error("compose_plan_changed", 409)
        self.validate_summary(value["successor"], self.successor_raw, successor["backend_profile"]["sha256"])
        return value

    def retained(self):
        """Read only the canonical owner's projection; never interpret its store."""
        if self.plan is None and self.host["mode"] != "reopen":
            raise self.io.Error("compose_not_prepared", 409)
        args = self.command("selection")
        if self.transition_sha256 is not None:
            args += ["--transition-sha256", self.transition_sha256]
        value = self.runtime(args)
        self.io.keys(value, ("schema_version", "status", "selected", "selected_from_transition_sha256",
                             "pending_transition", "pending_operation", "proposal", "state_namespaces"))
        if (type(value["schema_version"]) is not int or value["schema_version"] != 1 or
                value["status"] != "selected_plan" or type(value["pending_operation"]) is not bool):
            raise self.io.Error("compose_result_invalid", 409)
        namespaces = value["state_namespaces"]
        if (not isinstance(namespaces, list) or len(namespaces) > 16 or
                any(not component_id(item) for item in namespaces) or
                namespaces != sorted(set(namespaces))):
            raise self.io.Error("compose_result_invalid", 409)
        selected, pending, proposal = value["selected"], value["pending_transition"], value["proposal"]
        origin = value["selected_from_transition_sha256"]
        if origin is not None and (not isinstance(origin, str) or not HEX.fullmatch(origin)):
            raise self.io.Error("compose_result_invalid", 409)
        self.validate_summary(selected)
        if proposal is not None:
            self.proposal(proposal)
            self.successor_plan = proposal["successor"]
        elif self.transition_sha256 is not None:
            raise self.io.Error("compose_result_invalid", 409)
        if self.plan is None:
            initial = selected if selected["plan_sha256"] == self.host["expected_plan_sha256"] else (proposal or {}).get("predecessor")
            self.validate_summary(initial)
            if initial["summary"]["profile_sha256"] != self.host["backend_profile"]["sha256"]:
                raise self.io.Error("compose_profile_changed", 409)
            if initial["plan_sha256"] != self.host["expected_plan_sha256"]:
                raise self.io.Error("compose_plan_changed", 409)
            self.plan = initial
        if selected != self.plan:
            if (self.transition_sha256 is None or selected != self.successor_plan or
                    value["selected_from_transition_sha256"] != self.transition_sha256):
                raise self.io.Error("compose_plan_changed", 409)
        if pending is not None:
            self.io.keys(pending, ("transition_sha256", "phase", "predecessor_plan_sha256", "successor_plan_sha256"))
            if (proposal is None or pending["transition_sha256"] != self.transition_sha256 or
                    pending["phase"] not in ("retire", "migrate", "activate") or
                    pending["phase"] == "migrate" and proposal["schema_version"] != 2 or
                    pending["predecessor_plan_sha256"] != proposal["predecessor"]["plan_sha256"] or
                    pending["successor_plan_sha256"] != proposal["successor"]["plan_sha256"] or
                    selected != proposal["successor" if pending["phase"] == "activate" else "predecessor"]):
                raise self.io.Error("compose_plan_changed", 409)
        # The current canonical inventory includes dormant state. It validates
        # the host's explicit ceiling; it never creates a deletion grant.
        if not set(self.host["delete_state_namespaces"]) <= set(namespaces):
            raise self.io.Error("invalid_compose_profile")
        self.selection_state = value
        return value

    def observation(self, value, plan=None):
        plan = plan or self.plan
        if (not isinstance(value, dict) or type(value.get("schema_version")) is not int or value["schema_version"] != 1 or
                value.get("plan_sha256") != plan["plan_sha256"] or
                not isinstance(value.get("observation_sha256"), str) or not HEX.fullmatch(value["observation_sha256"]) or
                type(value.get("pending_operation")) is not bool or
                any(not isinstance(value.get(key), dict) for key in ("containers", "volumes", "available")) or
                any(type(v) is not bool for v in value["available"].values()) or
                not (value.get("network") is None or isinstance(value["network"], dict))):
            raise self.io.Error("compose_result_invalid", 409)

    def bindings(self):
        if (self.plan is None or self.execution_uncertain or self.selection_state is None or
                self.selection_state["pending_transition"] is not None or self.selection_state["pending_operation"] or
                self.selection_state["selected"] != self.plan):
            return []
        return [{"scope": BINDING_SCOPE, "component": component, "plan_sha256": self.plan["plan_sha256"],
                 "command": self.host["python"]["path"],
                 "args": ["-I", "-B", str(self.session / "runtime.py"), "--workspace", str(self.graph),
                          "--planner", str(self.session / "planner"), "--planner-sha256", self.host["planner"]["sha256"],
                          "run-stdio", "--plan-sha256", self.plan["plan_sha256"], "--component", component],
                 "env": {}, "runtime_sha256": self.host["runtime"]["sha256"],
                 "planner_sha256": self.host["planner"]["sha256"]}
                for component in self.plan["summary"]["request_components"]]

    def prepared_summary(self, value=None):
        """Project metadata only; the runtime owns full canonical plan verification."""
        value = value or self.plan
        if value.get("status") == "prepared_summary":
            return {**copy.deepcopy(value), "status": "prepared"}
        plan = value["plan"]
        summary = {key: copy.deepcopy(plan[key]) for key in (
            "request_sha256", "package_sha256", "manifest_sha256", "manifest_source_sha256",
            "profile_sha256", "configuration_sha256", "policy_sha256", "host_owner", "project",
            "initialization_order", "shutdown_order", "deployment_order", "request_components")}
        summary["components"] = {name: {key: copy.deepcopy(item[key]) for key in (
            "service", "lifetime", "image", "image_id", "platform", "dependencies",
            "deployment_dependencies", "readiness", "stop_timeout_ms")}
            for name, item in plan["components"].items()}
        summary["files"] = {name: {"sha256": item["sha256"], "mode": item["mode"]}
                            for name, item in plan["files"].items()}
        return {"schema_version": 1, "status": "prepared",
                "plan_sha256": value["plan_sha256"], "summary": summary}

    def observed_summary(self, value, selection):
        selected = selection["selected"]
        if selection["pending_transition"] is not None:
            pending = selection["pending_transition"]
            if (not isinstance(value, dict) or value.get("status") != "successor_pending" or
                    value.get("transition_sha256") != pending["transition_sha256"] or
                    value.get("phase") != pending["phase"] or
                    not isinstance(value.get("observation_sha256"), str) or
                    not HEX.fullmatch(value["observation_sha256"]) or
                    not isinstance(value.get("custody_sha256"), str) or not HEX.fullmatch(value["custody_sha256"])):
                raise self.io.Error("compose_result_invalid", 409)
            self.observation(value.get("observation"), selected)
            return {key: copy.deepcopy(value[key]) for key in
                ("status", "transition_sha256", "phase", "observation", "observation_sha256", "custody_sha256")}
        self.observation(value, selected)
        return value

    def execute(self, action, request):
        self.admit(action)
        self.verify()
        path = raw = proposed = None
        if action == "prepare":
            path, raw = self.freeze(request)
            proposed = self.preflight(path, raw)
        try:
            if action != "prepare":
                selection = self.retained()
                if action not in SUCCESSOR_ACTIONS | {"observe"}:
                    if (selection["pending_transition"] is not None or selection["selected"] != self.plan or
                            request["plan_sha256"] != self.plan["plan_sha256"]):
                        raise self.io.Error("compose_plan_changed", 409)
            args = self.command(action.replace("_", "-"))
            if action == "prepare_successor":
                successor = self.host["successor"]
                if (request["predecessor_plan_sha256"] != successor["predecessor_plan_sha256"] or
                        request["inputs_sha256"] != successor["inputs"]["sha256"] or
                        selection["pending_transition"] is not None or
                        selection["selected"]["plan_sha256"] != successor["predecessor_plan_sha256"]):
                    raise self.io.Error("compose_plan_changed", 409)
                path, raw = self.session / "successor-request.json", self.successor_raw
                args += ["--predecessor-plan-sha256", request["predecessor_plan_sha256"]]
            elif action in ("adopt_successor", "reconcile_successor", "rebind"):
                if self.transition_sha256 is None or request["transition_sha256"] != self.transition_sha256:
                    raise self.io.Error("compose_plan_changed", 409)
                if action == "adopt_successor":
                    if (request["predecessor_plan_sha256"] != self.host["successor"]["predecessor_plan_sha256"] or
                            selection["pending_transition"] is not None or
                            selection["selected"]["plan_sha256"] != request["predecessor_plan_sha256"]):
                        raise self.io.Error("compose_plan_changed", 409)
                    args += ["--predecessor-plan-sha256", request["predecessor_plan_sha256"]]
                if action != "rebind":
                    args += ["--transition-sha256", request["transition_sha256"],
                             "--observation-sha256", request["observation_sha256"]]
            elif action not in ("prepare", "observe"):
                args += ["--plan-sha256", request["plan_sha256"]]
                if action in ("resume", "stop", "remove"):
                    args += ["--observation-sha256", request["observation_sha256"]]
                if action == "remove":
                    for namespace in sorted(request["delete_state"]):
                        args += ["--delete-state", namespace]
            if action == "rebind":
                if (selection["pending_transition"] is not None or selection["pending_operation"] or self.successor_plan is None or
                        selection["selected"] != self.successor_plan or
                        request["plan_sha256"] != self.successor_plan["plan_sha256"] or
                        selection["selected_from_transition_sha256"] != self.transition_sha256):
                    raise self.io.Error("compose_rebind_unavailable", 409)
                # This explicit host-enabled session mutation grants nothing
                # outside the same pinned target and existing action allowlist.
                self.plan = self.successor_plan
                value = {"status": "rebound", "transition_sha256": self.transition_sha256,
                         "selected": self.prepared_summary()}
            else:
                value = self.runtime(args, path=path, raw=raw)
                if action == "prepare":
                    if value != proposed:
                        raise self.io.Error("compose_result_invalid", 409)
                    self.plan = self.canonical_summary(proposed)
                    value = self.prepared_summary()
                elif action == "prepare_successor":
                    self.io.keys(value, ("schema_version", "status", "transition_sha256", "predecessor", "successor"))
                    if type(value["schema_version"]) is not int or value["schema_version"] not in (1, 2) or value["status"] != "prepared_successor":
                        raise self.io.Error("compose_result_invalid", 409)
                    value = {**value, "status": "prepared_successor_summary",
                        "predecessor": self.canonical_summary(value["predecessor"]),
                        "successor": self.canonical_summary(value["successor"])}
                    self.proposal(value)
                    self.transition_sha256 = value["transition_sha256"]
                    self.successor_plan = value["successor"]
                    value = {"schema_version": value["schema_version"], "status": "prepared_successor",
                        "transition_sha256": self.transition_sha256,
                        "predecessor": self.prepared_summary(value["predecessor"]),
                        "successor": self.prepared_summary(value["successor"])}
                current = self.retained()
                if action == "observe":
                    value = self.observed_summary(value, current)
                elif action not in ("prepare", "prepare_successor"):
                    if (not isinstance(value, dict) or value.get("plan_sha256") != current["selected"]["plan_sha256"] or
                            value.get("status") not in ("observed", "unconfirmed") or "observation" not in value or
                            (value["status"] == "observed" and value["observation"] is None)):
                        raise self.io.Error("compose_result_invalid", 409)
                    if action in ("adopt_successor", "reconcile_successor") and value.get("transition_sha256") != self.transition_sha256:
                        raise self.io.Error("compose_result_invalid", 409)
                    if value["observation"] is not None:
                        self.observation(value["observation"], current["selected"])
                    value = {key: copy.deepcopy(value[key]) for key in
                        ("status", "plan_sha256", "observation", "transition_sha256") if key in value}
        except BaseException:
            # Outer process loss cannot attest its separately-sessioned Docker
            # children. A fresh host must establish quiescent custody to reopen.
            self.execution_uncertain = True
            raise
        return {"scope": RESULT_SCOPE, "action": action, "backend": value,
                "runtime_bindings": [] if value.get("status") == "unconfirmed" else self.bindings()}
