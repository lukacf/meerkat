"""Lower application Add intent using the original installation's selections.

This constructs an unissued request, never an admission or authentication result.
The application owns its decision and durable admitting row; native owns Add,
controller issuance, held publication and the eventual outcome.
"""
from copy import deepcopy
import hashlib
import re

from .connect_installer_caller import CustodyError, _json, _same_member_classes


def _roster_member(identity):
    """Meta member_comms_id::mob_member_id_str public stability codec.

    Input is a validated durable identity, never a runtime alias or roster ID.
    Unicode escapes encode scalar values, not UTF-8 bytes.
    """
    if re.fullmatch(r"[A-Za-z_][A-Za-z0-9_-]*", identity) and not identity.startswith("mk--"):
        return identity
    body = []
    for char in identity:
        if char == "_":
            body.append("__")
        elif char == ":":
            body.append("_c")
        elif char.isascii() and (char.isalnum() or char == "-"):
            body.append(char)
        else:
            body.append("_x" + format(ord(char), "x") + "_")
    return "mk--" + "".join(body)


def _installation_inputs(callbacks):
    callbacks._current()
    plan = _json(callbacks._plan_bytes)
    owner = plan.get("owner")
    native = owner.get("native_host") if isinstance(owner, dict) else None
    application = plan["application"]
    realm = application.get("realm")
    if (not isinstance(native, dict) or not isinstance(realm, dict)
            or not isinstance(native.get("realm"), str) or not native["realm"]
            or native["realm"] != realm.get("realm_id")
            or not _same_member_classes(list(callbacks.member_classes.values()), native.get("member_classes", []))):
        raise CustodyError("Member Add requires the original installation's native class and realm selection.")
    human = native.get("remote_operator")
    ingress_domain = native.get("ingress_domain")
    ingress = ingress_domain.get("authority") if isinstance(ingress_domain, dict) else None
    if (not isinstance(human, dict) or human.get("kind") != "human"
            or not isinstance(ingress, dict) or ingress.get("kind") != "runtime_host"
            or not isinstance(ingress.get("qualification"), dict)
            or ingress["qualification"].get("kind") != "qualified"
            or not isinstance(ingress["qualification"].get("trust_domain_id"), str)
            or not ingress["qualification"]["trust_domain_id"]):
        raise CustodyError("Member Add requires the selected installation operator and qualified ingress.")
    routes, members = native.get("routes"), native.get("members")
    if not isinstance(routes, list) or not isinstance(members, list):
        raise CustodyError("Member Add requires the original installation's member routes.")
    selected = []
    for route in routes:
        if not isinstance(route, dict) or not isinstance(route.get("origin"), dict):
            raise CustodyError("The selected installation contains an invalid native route.")
        origin = route["origin"]
        if origin.get("kind") != "local_console":
            continue
        matching = [row for row in members if isinstance(row, dict) and row.get("member") == origin.get("member")]
        authentication, event = route.get("authentication_domain"), route.get("event_domain")
        if (len(matching) != 1 or origin.get("requester") != human
                or origin.get("ingress_actor") != ingress or route.get("source_authority") != ingress
                or route.get("represented_subject") != human
                or not isinstance(authentication, dict) or authentication.get("authority") != ingress
                or not isinstance(event, dict) or event.get("authority") != ingress
                or not isinstance(authentication.get("namespace"), str) or not authentication["namespace"]
                or not isinstance(event.get("namespace"), str) or not event["namespace"]
                or matching[0].get("logical_owner") != route.get("logical_executor")
                or not isinstance(route.get("logical_executor"), dict)
                or route["logical_executor"].get("kind") != "shared_agent"
                or route["logical_executor"].get("qualification") != ingress["qualification"]
                or matching[0].get("audience") != {"audience": "principal", "principal": human}):
            raise CustodyError("Member Add cannot reinterpret the installation's selected Console authority.")
        selected.append((authentication, event))
    if not selected or any(item != selected[0] for item in selected[1:]):
        raise CustodyError("Member Add needs unambiguous selected Console authentication and event domains.")
    return human, ingress, selected[0]


def member_class_add_request(callbacks, deployment, *, admission_id, class_id, spec,
                             hold_timeout_ms=60_000, expected_predecessor=None, mcp=None):
    """Prepare only; callers persist this exact request before native begin_add."""
    from meerkat_meta.identity_first_models import DurableAgentSpec
    from meerkat_toolkit.member_classes import MemberClassAddRequest

    callbacks._current()
    catalog = callbacks.dynamic_catalog(deployment)
    definition = callbacks.native_definition
    if not isinstance(spec, DurableAgentSpec):
        raise TypeError("Member Add requires the application's durable agent specification.")
    # Revalidate and detach mutable application values before retaining a request.
    exact = DurableAgentSpec.from_dict(deepcopy(spec.to_dict()))
    if (class_id not in catalog or exact.profile != deployment["applicationProfiles"][class_id]
            or exact.identity in callbacks._fixed or exact.initial_message is not None
            or exact.placement is not None):
        raise CustodyError("Member Add requires a new local identity in its exact ordinary declared class.")
    if (not isinstance(admission_id, str) or not admission_id.strip()
            or len(admission_id.encode("utf-8")) > 256):
        raise ValueError("Member Add requires a bounded durable admission ID.")
    human, ingress, (authentication, event) = _installation_inputs(callbacks)
    logical = {"kind": "shared_agent", "id": exact.identity, "qualification": deepcopy(ingress["qualification"])}
    member = {"mob_id": definition["id"], "member": _roster_member(exact.identity), "role": exact.profile}
    # Domain-separated deterministic names retain the original admission across restart.
    key = hashlib.sha256(admission_id.encode("utf-8")).hexdigest()
    route_id = "forge-member-route:" + key
    request = MemberClassAddRequest(id=admission_id, spec=exact,
        instance={"class": class_id, "member": {"member": member, "logical_owner": deepcopy(logical),
            "context_generation": 1, "audience": {"audience": "principal", "principal": deepcopy(human)}}},
        route={"id": route_id, "origin": {"kind": "local_console", "member": deepcopy(member),
            "requester": deepcopy(human), "ingress_actor": deepcopy(ingress)},
            "logical_executor": logical, "represented_subject": deepcopy(human),
            "source_authority": deepcopy(ingress), "authentication_domain": deepcopy(authentication),
            "event_domain": deepcopy(event)},
        controller={"target": {"kind": "original_route", "route": route_id},
            "issuance_id": "forge-member-controller:" + key},
        hold_timeout_ms=hold_timeout_ms, expected_predecessor=deepcopy(expected_predecessor), mcp=deepcopy(mcp))
    request.wire()
    callbacks._current()
    return request


def bind_member_class_requests(owner, application, runtime):
    """Install a request factory after successful Fresh/Restore connection."""
    if not application.dynamic:
        return
    callbacks, context = owner.callbacks, application.context

    def current():
        owner._require_current()
        if (owner.runtime is not runtime or context.runtime is not runtime
                or owner.application is not application or application._application_callbacks is not callbacks
                or owner._shutdown_task is not None or context.stop_event.is_set()):
            raise CustodyError("Member Add request mapping needs its current connected application owner.")

    def prepare(**intent):
        current()
        request = member_class_add_request(callbacks, application.deployment, **intent)
        current()
        return request

    context._member_class_requests = prepare
