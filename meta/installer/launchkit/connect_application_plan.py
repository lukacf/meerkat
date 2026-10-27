"""Assemble a trusted draft for Toolkit's existing schema4 preparation driver.

The input pin provides byte custody for an early draft, not approval. The caller
approves and pins the returned new plan through its existing transaction. This
module never rewrites an approved artifact, issues grants, constructs native
surfaces, runs a driver, or publishes an activation. Native code retains typed
principal, lineage, controller-key, roster-lowering and policy validation.
"""
from __future__ import annotations

from collections.abc import Callable
import json
from pathlib import Path

from .connect_installer_caller import (CustodyError, ExactFile, _preparation_setup_targets,
    _preparation_helper_catalog_request, _preparation_helper_catalog_capability,
    _preparation_helper_catalog_basis, _prepared_issuance_reference,
    _selected_service_parent_administration, _selected_standing_policy, _same_member_classes)


MAX_PLAN_BYTES = 256 * 1024
_PLAN_FIELDS = {"schema", "deployment_root", "owner", "startup", "authorities",
                "application", "acquisition", "output", "hold_timeout_ms"}
_MEMBER_FIELDS = {"member", "logical_owner", "context_generation", "audience"}
_ROUTE_FIELDS = {"id", "origin", "application_parent", "controller_parent",
                 "logical_executor", "represented_subject", "source_authority",
                 "authentication_domain", "event_domain"}
_CHANNEL_FIELDS = {"component", "channel_id", "service_subject", "provider_account",
                   "sender_id", "requester", "ingress_actor", "target_identity",
                   "member", "realm", "required_surfaces"}
_ACTIONS = {"find_and_install", "install_artifacts", "connect_account", "grant_service",
            "expose_frontend", "invoke_service", "deliver", "read_memory", "delegate"}


def _object(pairs):
    value = {}
    for key, item in pairs:
        if key in value:
            raise CustodyError("The preparation draft contains duplicate JSON fields.")
        value[key] = item
    return value


def _invalid_constant(_):
    raise CustodyError("The preparation draft contains a non-JSON constant.")


def _closed(value, required, label, optional=()):
    if (not isinstance(value, dict) or not required <= value.keys()
            or value.keys() - required - set(optional)):
        raise CustodyError(f"{label} requires its exact native input fields.")
    return value


def _text(value, label):
    if not isinstance(value, str) or not value or value.strip() != value:
        raise CustodyError(f"{label} requires exact nonempty text.")
    return value


def _array(value, label):
    if not isinstance(value, list):
        raise CustodyError(f"{label} requires an explicit list.")
    return value


def _positive(value, label, maximum=None):
    if type(value) is not int or value < 1 or maximum is not None and value > maximum:
        raise CustodyError(f"{label} requires a bounded positive integer.")


def _same(left, right):
    # Preserve JSON types: Python equality alone equates true and 1.
    return json.dumps(left, sort_keys=True, allow_nan=False) == json.dumps(
        right, sort_keys=True, allow_nan=False)


def _binding(value):
    _closed(value, {"mob_id", "role", "member"}, "Native member binding")
    for name, item in value.items():
        _text(item, f"Native member {name}")
    return value


def _source_binding(route, channels, realm):
    """Match CompiledNativeSourceRoute::source_binding_in, without inference."""
    origin = route["origin"]
    if not isinstance(origin, dict):
        raise CustodyError("An original route requires its selected native origin.")
    if origin.get("kind") == "local_console":
        _closed(origin, {"kind", "member", "requester", "ingress_actor"}, "Local console origin")
        return _binding(origin["member"])
    if origin.get("kind") == "provider_event":
        _closed(origin, {"kind", "channel_id", "sender_id", "target_identity"}, "Provider origin")
        names = ("channel_id", "sender_id", "target_identity")
        for name in names:
            _text(origin[name], name)
        matches = [row for row in channels if all(row[name] == origin[name] for name in names)]
        if len(matches) != 1 or matches[0]["realm"] != realm:
            raise CustodyError("A provider original requires one exact selected channel binding.")
        return _binding(matches[0]["member"])
    raise CustodyError("An original route requires the native local_console/provider_event tag.")


def _assemble(plan):
    _closed(plan, _PLAN_FIELDS - {"acquisition"}, "ApplicationPreparationPlan", {"acquisition"})
    if type(plan["schema"]) is not int or plan["schema"] != 1:
        raise CustodyError("ApplicationPreparationPlan requires schema 1.")
    for name in ("deployment_root", "output"):
        if not Path(_text(plan[name], name)).is_absolute():
            raise CustodyError(f"{name} requires an absolute selected path.")
    _positive(plan["hold_timeout_ms"], "Shared native hold budget", 900_000)
    startup = plan["startup"]
    if isinstance(startup, dict) and startup.get("kind") == "restore":
        _closed(startup, {"kind"}, "Restore startup")
    elif isinstance(startup, dict) and startup.get("kind") == "initialize":
        _closed(startup, {"kind", "namespace", "generation"}, "Explicit initialize startup")
        _text(startup["namespace"], "Native owner namespace")
        _positive(startup["generation"], "Native owner generation")
    else:
        raise CustodyError("The native owner startup must be explicitly selected.")

    owner = _closed(plan["owner"], {"deployment", "components", "channels", "native_host"}, "PreparedLocalPolicy")
    _text(owner["deployment"], "Deployment")
    _array(owner["components"], "Selected components")
    native = _closed(owner["native_host"], {"realm", "grant_administrator", "remote_operator",
        "ingress_domain", "members", "routes"}, "PreparedNativeHost", {"service_parent_administration", "member_classes"})
    realm = _text(native["realm"], "Native realm")
    members = _array(native["members"], "Prepared native members")
    by_member = {}
    for member in members:
        _closed(member, _MEMBER_FIELDS, "PreparedNativeMember", {"setup_capability"})
        binding = _binding(member["member"])
        key = binding["member"]
        if key in by_member:
            raise CustodyError("The native member inventory repeats a member string.")
        _positive(member["context_generation"], "Native member context generation")
        by_member[key] = member
    channels = _array(owner["channels"], "Compiled channels")
    for channel in channels:
        _closed(channel, _CHANNEL_FIELDS, "CompiledChannelBinding")
        _binding(channel["member"])
    routes = {}
    route_members = {}
    for route in _array(native["routes"], "Original routes"):
        _closed(route, _ROUTE_FIELDS, "CompiledNativeSourceRoute")
        key = _text(route["id"], "Original route ID")
        if key in routes:
            raise CustodyError("The selected original route ID is ambiguous.")
        binding = _source_binding(route, channels, realm)
        member = by_member.get(binding["member"])
        if member is None or not _same(member["member"], binding):
            raise CustodyError("The original route selects a foreign native member binding.")
        routes[key], route_members[key] = route, binding
    _selected_service_parent_administration(native.get("service_parent_administration"),
        routes=native["routes"], remote_operator=native["remote_operator"])

    helper_fields = {"helper", "helper_tools", "helper_profile"}
    application = _closed(plan["application"], {"schema", "realm", "definition", "roster",
        "members", "tool_policy"}, "ApplicationSetupCompilationInput", helper_fields | {"member_classes"})
    if type(application["schema"]) is not int or application["schema"] != 4:
        raise CustodyError("Application setup requires schema 4, without surface placeholders.")
    selected_helper_fields = helper_fields & application.keys()
    if selected_helper_fields and selected_helper_fields != helper_fields:
        raise CustodyError("Application setup requires all helper fields or none.")
    has_helper = bool(selected_helper_fields)
    if ("acquisition" in plan) != has_helper:
        raise CustodyError("Acquisition parameters require the same explicit helper selection.")
    declarations = _array(application.get("member_classes", []), "Native member-class declarations")
    if any(not isinstance(item, dict) for item in declarations):
        raise CustodyError("Native member-class declarations must retain their selected objects.")
    if not _same_member_classes(native.get("member_classes", []), declarations):
        raise CustodyError("Application and owner member-class selections differ.")
    if not members and (has_helper or not declarations):
        raise CustodyError("An empty initial inventory requires helperless native member classes.")
    if not _same(application["members"], members):
        raise CustodyError("Application and owner member selections differ.")
    selected_realm = _closed(application["realm"], {"realm_id", "context_root", "state_root"}, "ApplicationSetupRealm", {"user_config_root"})
    if selected_realm["realm_id"] != realm:
        raise CustodyError("Application and owner realms differ.")
    for name in ("context_root", "state_root", "user_config_root"):
        if selected_realm.get(name) is not None and not Path(_text(selected_realm[name], name)).is_absolute():
            raise CustodyError("Application storage roots must be absolute selected paths.")
    roster = _array(application["roster"], "Actual provider roster")
    identities = [_text(row.get("identity"), "Actual roster identity")
                  if isinstance(row, dict) else _text(None, "Actual roster identity") for row in roster]
    if len(roster) != len(members) or len(set(identities)) != len(identities):
        raise CustodyError("The actual roster must cover the selected member inventory once.")
    definition = application["definition"]
    if not isinstance(definition, dict) or definition.get("format") not in {"toml", "native"}:
        raise CustodyError("The original application definition must retain its native format.")
    _closed(definition, {"format", "text" if definition["format"] == "toml" else "definition"}, "ApplicationSetupDefinition")
    if definition["format"] == "toml" and not isinstance(definition["text"], str):
        raise CustodyError("The exact TOML definition must remain text.")
    if has_helper:
        helper = _closed(application["helper"], {"member", "ingress_actor", "logical_executor",
            "represented_subject", "controller_parent", "operation_basis"}, "InstalledSetupSelection",
            {"report_capability", "catalog_capability"})
        _preparation_helper_catalog_capability(helper.get("catalog_capability"), helper["operation_basis"])
        helper_binding = _binding(helper["member"])
        if not any(_same(helper_binding, member["member"]) for member in members):
            raise CustodyError("The helper must be an actual selected native member.")
        _array(helper["controller_parent"], "Selected helper controller parents")
        # Preserve the trusted application's explicit checked-install composition.
        # The actual native driver still requires its retained terminal or Console
        # presenter and owns review, tool admission and installation permission.
        if application["helper_tools"] not in {"preparation", "inert_acquisition", "checked_installation"} or application["helper_profile"] not in {"dedicated", "exclusive_application_role"}:
            raise CustodyError("The setup helper requires its exact selected native strategy.")
    policy = _closed(application["tool_policy"], {"provider_id", "policy_id", "revision", "source", "members"}, "ApplicationToolPolicyInput")
    policy_members = _array(policy["members"], "Explicit member tool policy")
    policy_names = []
    for row in policy_members:
        _closed(row, {"member_identity", "grants"}, "Explicit member tool grants")
        policy_names.append(_text(row["member_identity"], "Policy native member identity"))
        _array(row["grants"], "Explicit tool grants")
    if len(policy_names) != len(by_member) or set(policy_names) != set(by_member):
        raise CustodyError("Tool policy must select every actual native member exactly once.")

    authorities = _closed(plan["authorities"], set(), "PreparationAuthorityIntent",
        {"controllers", "agent_setup", "helper_report", "helper_mcp", "helper_catalog"}
        if has_helper else {"controllers"})
    issuance_ids, controller_targets = set(), set()
    for name in ("helper_mcp", "helper_catalog"):
        request = authorities.get(name)
        if request is None:
            continue
        if name == "helper_catalog":
            scope = _preparation_helper_catalog_request(request)
            if scope == "exact_session":
                # Match native's default-scope serializer; legacy wire stays exact.
                request.pop("scope", None)
                if helper.get("catalog_capability") is not None:
                    raise CustodyError("An exact-session request cannot select a successor parent.")
            else:
                _preparation_helper_catalog_basis(helper["operation_basis"])
                capability = helper.get("catalog_capability")
                if capability is not None:
                    if len(capability["parent"]) != 1:
                        raise CustodyError("The catalog request differs from its retained parent.")
                    _prepared_issuance_reference(capability["parent"][0], request["issuance_id"],
                        native["remote_operator"])
        else:
            _closed(request, {"issuance_id"}, f"Prepared {name} request")
        issue = _text(request["issuance_id"], f"Retained {name} issuance ID")
        if issue in issuance_ids:
            raise CustodyError("Authority requests repeat an issuance ID.")
        issuance_ids.add(issue)
    for request in _array(authorities.get("controllers", []), "Controller requests"):
        _closed(request, {"target", "issuance_id"}, "PreparedControllerIntent", {"controller_route"})
        target = request["target"]
        if isinstance(target, dict) and target.get("kind") == "helper":
            if not has_helper:
                raise CustodyError("A helper controller request requires an actual helper.")
            _closed(target, {"kind"}, "Helper controller target")
            key = ("helper",)
        elif isinstance(target, dict) and target.get("kind") == "original_route":
            _closed(target, {"kind", "route"}, "Original controller target")
            if target["route"] not in routes:
                raise CustodyError("A controller request selects an unknown original route.")
            key = ("original_route", target["route"])
        else:
            raise CustodyError("A controller request requires an explicit native target.")
        issue = _text(request["issuance_id"], "Retained controller issuance ID")
        # An explicit key constrains the actual held native report. Omitting
        # it selects no key here, and never invents a client or session fact.
        if request.get("controller_route") is not None:
            _text(request["controller_route"], "Selected native controller route key")
        if key in controller_targets or issue in issuance_ids:
            raise CustodyError("Authority requests repeat a target or issuance ID.")
        controller_targets.add(key)
        issuance_ids.add(issue)
    if has_helper and not helper["controller_parent"] and ("helper",) not in controller_targets:
        raise CustodyError("An empty helper controller selection requires its explicit request.")

    selected_targets, requested_members = set(), set()
    # Restore retains only explicitly configured capability routes, never an
    # automatic acquisition selection for every member in the finite roster.
    for member in members:
        capability = member.get("setup_capability")
        if capability is None:
            continue
        if not has_helper:
            raise CustodyError("A configured setup capability requires an actual helper.")
        _closed(capability, {"route", "parent"}, "Configured agent setup capability", {"helper_targets"})
        _preparation_setup_targets(capability.get("helper_targets", []), compiled=True)
        route = _text(capability["route"], "Configured setup original route")
        if route not in route_members or not _same(route_members[route], member["member"]):
            raise CustodyError("A configured setup capability differs from its native original route.")
        if startup["kind"] == "restore":
            selected_targets.add(member["member"]["member"])
    for request in _array(authorities.get("agent_setup", []), "Named agent setup requests"):
        _closed(request, {"member", "route", "issuance_id"}, "AgentSetupCapabilityRequest", {"helper_targets"})
        binding = _binding(request["member"])
        route = _text(request["route"], "Configured agent original route")
        issue = _text(request["issuance_id"], "Retained setup issuance ID")
        if route not in route_members or not _same(route_members[route], binding):
            raise CustodyError("A setup request differs from its exact native original route binding.")
        key = binding["member"]
        if key in requested_members or issue in issuance_ids:
            raise CustodyError("Authority requests repeat a member or issuance ID.")
        capability = by_member[key].get("setup_capability")
        if capability is not None and capability["route"] != route:
            raise CustodyError("A setup request changed the retained configured original route.")
        requested_members.add(key)
        issuance_ids.add(issue)
        helper_targets = _preparation_setup_targets(request.get("helper_targets", []))
        for selected_target in helper_targets:
            target_issue = _text(selected_target["issuance_id"], "Retained helper target issuance ID")
            if target_issue in issuance_ids:
                raise CustodyError("Authority requests repeat an issuance ID.")
            issuance_ids.add(target_issue)
        if capability is not None and not _same(
                [row["target"] for row in helper_targets],
                [row["target"] for row in capability.get("helper_targets", [])]):
            raise CustodyError("A setup request changed the retained configured helper targets.")
        selected_targets.add(key)
    report = authorities.get("helper_report")
    if report is not None:
        _closed(report, {"member", "issuance_id"}, "HelperReportCapabilityRequest",
                {"account_issuance_id", "public_issuance_id"})
        if not _same(_binding(report["member"]), helper_binding):
            raise CustodyError("The independent report request must select the actual helper.")
        for name in ("issuance_id", "account_issuance_id", "public_issuance_id"):
            if name != "issuance_id" and report.get(name) is None:
                continue
            issue = _text(report[name], f"Retained helper {name}")
            if issue in issuance_ids:
                raise CustodyError("Authority requests repeat an issuance ID.")
            issuance_ids.add(issue)

    if not has_helper:
        # This path has no acquisition service or helper authority. Preserve
        # the original class and controller declarations for native validation.
        return plan

    acquisition = _closed(plan["acquisition"], {"backend_generation", "proposal_ceiling", "output_audience", "limits"}, "ApplicationAcquisitionParameters", {"console_setup", "standing_policy"})
    _selected_standing_policy(acquisition.get("standing_policy"), helper_tools=application["helper_tools"])
    _text(acquisition["backend_generation"], "Retained WorkGraph generation")
    ceiling = _closed(acquisition["proposal_ceiling"], {"actions", "services", "frontends", "output_audiences"}, "Proposal ceiling")
    actions, seen = _array(ceiling["actions"], "Exact action bounds"), set()
    for permission in actions:
        _closed(permission, {"action", "resource"}, "ActionPermission")
        action, resource = permission["action"], _text(permission["resource"], "Exact proposal resource")
        if action not in _ACTIONS or (action, resource) in seen:
            raise CustodyError("Proposal actions must retain unique native action/resource pairs.")
        if action == "find_and_install" and resource not in selected_targets:
            raise CustodyError("Acquisition bounds require an explicitly configured original member.")
        seen.add((action, resource))
    for name in ("services", "frontends", "output_audiences"):
        _array(ceiling[name], f"Selected {name}")
    audience = _array(acquisition["output_audience"], "Selected output audience")
    if not audience or any(value not in ceiling["output_audiences"] for value in audience):
        raise CustodyError("Acquisition output audience exceeds its selected proposal ceiling.")
    limits = _closed(acquisition["limits"], {"setup_timeout_ms", "max_workers", "max_effect_attempts", "admission_timeout_ms"}, "InstalledSetupLimits")
    for name, value in limits.items():
        _positive(value, name, 60_000 if name == "admission_timeout_ms" else None)
    for target in sorted(selected_targets):
        for action in ("find_and_install", "delegate"):
            if (action, target) not in seen:
                actions.append({"action": action, "resource": target})
                seen.add((action, target))
    return plan


def application_preparation_plan(selection: ExactFile, *, require_current: Callable[[], None]) -> bytes:
    """Return a new bounded plan from actual trusted pre-approval selected facts.

    The existing install/configure owner retains all request IDs and supplies its
    currentness check. The input and every opaque native fact are preserved,
    except native's omitted exact-session scope and missing proposal pairs for requests or configured
    Restore capabilities. The output is not approved, executed or published.
    """
    if not isinstance(selection, ExactFile) or not callable(require_current):
        raise TypeError("Plan assembly requires an exact draft pin and its existing owner check.")
    require_current()
    original = selection.read()
    if len(original) > MAX_PLAN_BYTES:
        raise CustodyError("The preparation draft exceeds 256 KiB.")
    try:
        plan = json.loads(original, object_pairs_hook=_object, parse_constant=_invalid_constant)
        assembled = _assemble(plan)
        result = (json.dumps(assembled, ensure_ascii=True, allow_nan=False, separators=(",", ":")) + "\n").encode()
    except CustodyError:
        raise
    except (UnicodeError, ValueError, TypeError, RecursionError) as error:
        raise CustodyError("The preparation draft is not bounded ordinary JSON.") from error
    if len(result) > MAX_PLAN_BYTES:
        raise CustodyError("The assembled preparation plan exceeds 256 KiB.")
    require_current()
    if selection.read() != original:
        raise CustodyError("The preparation draft source selection changed during assembly.")
    return result
