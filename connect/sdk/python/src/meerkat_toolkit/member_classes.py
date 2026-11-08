"""Application-owned class admission on the runtime's existing private pipe.

Before Add, the application durably records an admitting member excluded from
its desired-roster projection. Listing belongs inside native held completion,
after Connect policy commits, without acquiring the application's reconcile
lock there. The retained async ``publish`` callback performs that one actual
application mutation on the application's existing event loop.

This adapter neither changes the roster nor registers callbacks. The ordinary
Meta agent customizer supplies callbacks. A timeout is unknown settlement and
never triggers an automatic retry or a desired-state mutation.
"""
from __future__ import annotations

import asyncio
import copy
import threading
import weakref
from dataclasses import asdict, dataclass
from typing import Any, Awaitable, Callable

from meerkat_meta.identity_first_models import DurableAgentSpec

from .host_control import HostControlError, HostReplyCode, HostReplyError, HostTransportError, PrivateHostClient, PrivateSourceEvidence, _fields, _source_identifier


def _object(value: Any, required: set[str], optional: set[str] = frozenset()) -> dict[str, Any]:
    if not isinstance(value, dict) or not required <= value.keys() or value.keys() - required - optional:
        raise HostControlError("invalid member class selection")
    return copy.deepcopy(value)


def _checkpoint(value: Any) -> dict[str, Any]:
    checkpoint = _object(value, {"admission", "revision"})
    _source_identifier(checkpoint["admission"])
    if type(checkpoint["revision"]) is not int or not 0 <= checkpoint["revision"] <= 2**64 - 1:
        raise HostControlError("invalid member class predecessor revision")
    return checkpoint


@dataclass(frozen=True)
class MemberClassAddRequest:
    """Trusted application inputs, with native types retaining their wire shape.

    ``instance`` carries class and exact member/destination facts; ``route`` is
    the requested local Console source without issued parents; ``controller``
    names its issuance intent. None is authority or a callback implementation.
    Native binds ordinary Add to the installed operator, ingress and Console
    domains, and to the actual SharedAgent identity with Principal(operator)
    audience. These requested facts do not authenticate the event that caused
    Add. Optional MCP attachment separately requires current Console enrollment
    and protected human review through their existing owners.
    ``id`` is the application's durable admitting-row correlation, chosen before
    writing that row. An explicit ``expected_predecessor`` is the exact prior
    checkpoint observed by the caller; no checkpoint lookup or retry is implicit.
    """
    id: str
    instance: dict[str, Any]
    spec: DurableAgentSpec
    route: dict[str, Any]
    controller: dict[str, Any]
    hold_timeout_ms: int = 60_000
    expected_predecessor: dict[str, Any] | None = None
    mcp: MemberMcpAddIntent | None = None

    def wire(self) -> dict[str, Any]:
        _source_identifier(self.id)
        if not isinstance(self.spec, DurableAgentSpec) or self.spec.initial_message is not None or self.spec.placement is not None:
            raise HostControlError("member class add requires an ordinary local held member")
        if type(self.hold_timeout_ms) is not int or not 1 <= self.hold_timeout_ms <= 900_000:
            raise HostControlError("invalid member class hold deadline")
        instance = _object(self.instance, {"class", "member"})
        _source_identifier(instance["class"])
        selected = _object(instance["member"], {"member", "logical_owner", "context_generation", "audience"})
        member = _object(selected["member"], {"mob_id", "member", "role"})
        for value in member.values():
            _source_identifier(value)
        if member["role"] != self.spec.profile or type(selected["context_generation"]) is not int or selected["context_generation"] <= 0:
            raise HostControlError("member class destination does not match the selected profile")
        route = _object(self.route, {"id", "origin", "logical_executor", "represented_subject", "source_authority", "authentication_domain", "event_domain"},
                        {"application_parent", "controller_parent"})
        _source_identifier(route["id"])
        if route.get("application_parent") is not None or route.get("controller_parent") is not None:
            raise HostControlError("member class route cannot supply issued parents")
        origin = _object(route["origin"], {"kind", "member", "requester", "ingress_actor"})
        if origin["kind"] != "local_console" or origin["member"] != member:
            raise HostControlError("member class route does not select this member")
        controller = _object(self.controller, {"target", "issuance_id"}, {"controller_route"})
        if controller["target"] != {"kind": "original_route", "route": route["id"]}:
            raise HostControlError("member class controller does not select this route")
        _source_identifier(controller["issuance_id"])
        result = {"kind": "add", "id": self.id, "instance": instance, "spec": self.spec.to_dict(),
                  "route": route, "controller": controller, "hold_timeout_ms": self.hold_timeout_ms}
        if self.expected_predecessor is not None:
            result["expected_predecessor"] = _checkpoint(self.expected_predecessor)
        if self.mcp is not None:
            if not isinstance(self.mcp, MemberMcpAddIntent):
                raise HostControlError("member MCP intent has invalid type")
            result["mcp"] = self.mcp.wire()
        return result


def _member_observation(value: Any, operation: dict[str, Any], identifier: int, states: set[str]) -> dict[str, Any]:
    if not isinstance(value, dict) or value.get("state") not in states:
        raise HostControlError("invalid member class observation")
    state = value["state"]
    keys = {"reservation_not_accepted": {"state", "error"},
            "preflight_refused": {"state", "error", "cleanup"},
            "settled": {"state", "native", "authorization", "cleanup"},
            "unsettled": {"state", "settlement_id"}, "native_refused": {"state", "message", "data"},
            "observed": {"state", "record", "checkpoint", "authorization"}, "retired": {"state"}}[state]
    _fields(value, keys)
    if state == "unsettled" and (type(value["settlement_id"]) is not int or value["settlement_id"] != identifier):
        raise HostControlError("native settlement names another private operation")
    if state in {"reservation_not_accepted", "preflight_refused"}:
        HostReplyCode(value["error"])
    if state == "preflight_refused":
        cleanup = value["cleanup"]
        if not isinstance(cleanup, dict):
            raise HostControlError("invalid preflight cleanup observation")
        if cleanup.get("state") == "succeeded":
            _fields(cleanup, {"state"})
        elif cleanup.get("state") == "refused":
            _fields(cleanup, {"state", "error"})
            HostReplyCode(cleanup["error"])
        else:
            raise HostControlError("invalid preflight cleanup observation")
    if state == "settled":
        native = value["native"]
        if not isinstance(native, dict):
            raise HostControlError("invalid native member outcome")
        if native.get("state") == "result":
            _fields(native, {"state", "result"})
            result = _fields(native["result"], {"outcomes", "managed_edges"})
            if not isinstance(result["outcomes"], dict) or not isinstance(result["managed_edges"], list):
                raise HostControlError("invalid native member result")
            if operation.get("kind") == "add" and set(result["outcomes"]) != {operation["spec"]["identity"]}:
                raise HostControlError("native member result names another identity")
            if "identity" in operation and set(result["outcomes"]) != {operation["identity"]}:
                raise HostControlError("native member result names another identity")
        elif native.get("state") == "refused":
            _fields(native, {"state", "message", "data"})
            if not isinstance(native["message"], str):
                raise HostControlError("invalid native member refusal")
        else:
            raise HostControlError("invalid native member outcome")
        for field in ("authorization", "cleanup"):
            control = value[field]
            if not isinstance(control, dict):
                raise HostControlError("invalid member authorization or cleanup observation")
            if control.get("state") == "succeeded":
                _fields(control, {"state"})
            elif control.get("state") == "refused":
                _fields(control, {"state", "error"})
                HostReplyCode(control["error"])
            else:
                raise HostControlError("invalid member authorization or cleanup observation")
    if state == "observed":
        record = value["record"]
        checkpoint = _checkpoint(value["checkpoint"])
        if (not isinstance(record, dict) or record.get("id") != operation["id"]
                or checkpoint["admission"] != operation["id"]
                or record.get("instance", {}).get("member", {}).get("member") != operation["member"]):
            raise HostControlError("native observation names another member or predecessor")
        authorization = value["authorization"]
        if not isinstance(authorization, dict):
            raise HostControlError("invalid member authorization observation")
        if authorization.get("state") == "succeeded":
            _fields(authorization, {"state"})
        elif authorization.get("state") == "refused":
            _fields(authorization, {"state", "error"})
            HostReplyCode(authorization["error"])
        else:
            raise HostControlError("invalid member authorization observation")
    return copy.deepcopy(value)


class MemberClassClient:
    """Typed application calls on one already running Toolkit native child.

    ``unsettled`` is the native Abort decision with cleanup still owned by
    native. ``join`` transfers its actual observer into a pipe-owned join.
    ``observe`` reads one canonical record and checkpoint with authorization
    reported separately. A retired record remains inspectable but denied.
    Missing records are native refusals, never synthesized absence.
    Native identity status and
    ordinary input stay on the existing Meta runtime, including after restart.
    """
    def __init__(self, host_control: PrivateHostClient):
        if not isinstance(host_control, PrivateHostClient):
            raise HostControlError("member classes require the runtime's private host client")
        self._client = host_control

    def _call(self, operation: dict[str, Any], states: set[str], timeout: float | None) -> dict[str, Any]:
        identifier, deadline = self._client._start_call({"kind": "member_class", "operation": operation}, timeout=timeout)
        return self._await(operation, identifier, deadline, states)

    def _await(self, operation: dict[str, Any], identifier: int, deadline: float, states: set[str]) -> dict[str, Any]:
        reply = self._client._await_call(identifier, deadline, "member_class", {"kind", "observation"})
        try:
            return _member_observation(reply["observation"], operation, identifier, states)
        except (KeyError, TypeError, ValueError, HostControlError) as error:
            self._client._fail(error)
            raise HostTransportError("native member class response was rejected; settlement is unknown") from error

    async def add(self, request: MemberClassAddRequest, *, publish: Callable[[], Awaitable[None]],
                  timeout: float | None = None) -> dict[str, Any]:
        attempt = self.begin_add(request, publish=publish, timeout=timeout)
        return await asyncio.to_thread(attempt.result)

    def begin_add(self, request: MemberClassAddRequest, *, publish: Callable[[], Awaitable[None]],
                  timeout: float | None = None) -> MemberClassAddAttempt:
        """Write once and return its exact correlation before waiting for native.

        ``reservation_not_accepted`` changes no admission ownership: an earlier
        accepted operation may own that same correlation. Do not retire or void
        it because this call was refused. ``preflight_refused`` accepted ownership
        but entered no native Add; its separate cleanup is the joined retirement
        result. Neither state contains a native outcome or permits native Join.

        Do not prepublish the member to desired_roster. Application listing
        must occur inside native held completion after Connect policy commits;
        ``publish`` performs the actual durable listing without taking the
        application's reconcile lock. This method requires its running event
        loop. Before unlisting after a refusal or Unsettled result, await
        ``attempt.join_listing()`` so an entered mutation cannot re-list later.
        Losing an observation never repeats the command. The operation ID is
        only a private correlation, never evidence of activation or Unsettled.
        """
        if not isinstance(request, MemberClassAddRequest):
            raise HostControlError("member class request is required")
        if not callable(publish):
            raise HostControlError("the application's async listing callback is required")
        try:
            loop = asyncio.get_running_loop()
        except RuntimeError:
            raise HostControlError("application listing requires its running event loop") from None
        operation = request.wire()
        attempt: MemberClassAddAttempt | None = None

        def published(identifier: int, deadline: float) -> None:
            nonlocal attempt
            attempt = MemberClassAddAttempt(self, operation, identifier, deadline, publish, loop)
            self._client._member_listings[identifier] = attempt

        try:
            self._client._start_call({"kind": "member_class", "operation": operation}, timeout=timeout,
                                     _publish=published, _member_listing=True)
        except HostTransportError as error:
            if attempt is None:
                raise
            attempt._error = MemberClassObservationUnknown(attempt)
            attempt._observed = True
            raise attempt._error from error
        assert attempt is not None
        return attempt

    def join(self, settlement_id: int, *, timeout: float | None = None) -> dict[str, Any]:
        if type(settlement_id) is not int or settlement_id <= 0:
            raise HostControlError("invalid member class settlement reference")
        operation = {"kind": "join", "settlement_id": settlement_id}
        def retain_join(identifier: int, deadline: float) -> None:
            # The existing reader records a late native settlement even if this
            # Join observer times out. This is correlation metadata, not a task.
            attempt = self._client._member_listings.get(settlement_id)
            if attempt is not None:
                self._client._member_listing_join_ids[identifier] = attempt
        identifier, deadline = self._client._start_call({"kind": "member_class", "operation": operation},
                                                        timeout=timeout, _publish=retain_join)
        return self._await(operation, identifier, deadline, {"settled", "native_refused"})

    def observe(self, id: str, member: dict[str, str], *, timeout: float | None = None) -> dict[str, Any]:
        _source_identifier(id)
        member = _object(member, {"mob_id", "member", "role"})
        return self._call({"kind": "observe", "id": id, "member": member}, {"observed"}, timeout)

    def retire(self, id: str, member: dict[str, str], *, timeout: float | None = None) -> dict[str, Any]:
        """Narrow Connect authorization; does not delete desired/native membership."""
        _source_identifier(id)
        member = _object(member, {"mob_id", "member", "role"})
        return self._call({"kind": "retire", "id": id, "member": member}, {"retired"}, timeout)


class MemberClassObservationUnknown(HostTransportError):
    """The actual submitted attempt remains addressable; no native state inferred."""
    def __init__(self, attempt: MemberClassAddAttempt):
        super().__init__("member class observation is unavailable; native settlement is unknown")
        self.attempt = attempt
        self.operation_id = attempt.operation_id


class MemberClassObservationTimeout(MemberClassObservationUnknown, TimeoutError):
    """The caller's observation deadline elapsed without cancelling native."""


class MemberClassListingUnknown(HostControlError):
    """The application must reconcile its durable row; listing is not proven."""


class MemberClassAddAttempt:
    """One written Add and one retained observation, without automatic replay.

    After an unknown result, native identity status remains on Meta and Connect
    authorization remains on ``observe``. If native retained an Unsettled
    observer, ``client.join(operation_id)`` explicitly consumes that observer;
    an unknown ID or a result that was not Unsettled is refused.
    """
    def __init__(self, client: MemberClassClient, operation: dict[str, Any], identifier: int, deadline: float,
                 publish: Callable[[], Awaitable[None]], loop: asyncio.AbstractEventLoop,
                 *, listing_selection: dict[str, str] | None = None):
        self._client = client
        self._operation = operation
        self._listing_selection = listing_selection or {"admission_id": operation["id"],
            "identity": operation["spec"]["identity"], "class": operation["instance"]["class"]}
        self.operation_id = identifier
        self._deadline = deadline
        self._lock = threading.Lock()
        self._observed = False
        self._value: dict[str, Any] | None = None
        self._error: BaseException | None = None
        self._publish = publish
        self._loop = loop
        self._listing_requested = False
        self._native_terminal = False
        self._native_unsettled = False
        self._listing_task: asyncio.Task[None] | None = None
        self._listing_done: asyncio.Future[None] = loop.create_future()
        # Retrieve an unobserved exception without changing what join_listing
        # raises. The application may lose its observer while custody continues.
        self._listing_done.add_done_callback(lambda done: None if done.cancelled() else done.exception())

    def _listing_required(self) -> None:
        # Called by the pipe reader under its condition, never application code.
        if self._listing_requested or self._native_terminal:
            raise HostControlError("duplicate or late native listing request")
        self._listing_requested = True
        self._loop.call_soon_threadsafe(self._start_listing)

    def _native_received(self, reply: dict[str, Any]) -> None:
        if reply.get("kind") == "member_class" and reply.get("observation", {}).get("state") == "unsettled":
            self._native_unsettled = True
        else:
            # A terminal Add reply also includes an unaccepted reservation or
            # joined preflight refusal. It releases only this pipe attempt's
            # callback; it says nothing about another caller's same correlation.
            self._native_terminal = True
        self._loop.call_soon_threadsafe(self._finish_without_listing)

    def _pipe_lost(self) -> None:
        try:
            self._loop.call_soon_threadsafe(self._finish_without_listing)
        except RuntimeError:
            # A closed application event loop cannot prove its mutation joined.
            pass

    def _finish_without_listing(self) -> None:
        if self._listing_done.done():
            self._forget_settled_listing()
            return
        if self._listing_task is not None:
            return
        if self._client._client._failure is not None:
            self._finish_listing(MemberClassListingUnknown("private pipe lost; application listing is unknown"))
        elif self._native_terminal and not self._listing_requested:
            self._finish_listing(None)

    def _start_listing(self) -> None:
        if not self._listing_done.done() and self._listing_task is None:
            self._listing_task = self._loop.create_task(self._run_listing())

    async def _run_listing(self) -> None:
        failure: BaseException | None = None
        accepted = False
        try:
            # No await between this check and entry into the application callback.
            if self._client._client._failure is not None:
                raise MemberClassListingUnknown("private pipe lost before application listing")
            if not self._native_terminal and not self._native_unsettled:
                await self._publish()
                accepted = True
        except BaseException:
            failure = MemberClassListingUnknown("application listing did not settle successfully; reconcile its durable row")
        try:
            await asyncio.to_thread(self._ack_listing, accepted)
        except BaseException as error:
            failure = MemberClassListingUnknown("application listing acknowledgment is unknown; reconcile its durable row")
            if not isinstance(error, HostReplyError):
                self._client._client._fail(failure)
        self._finish_listing(failure)

    def _ack_listing(self, accepted: bool) -> None:
        client = self._client._client
        identifier, deadline = client._start_call({"kind": "member_class", "operation": {
            "kind": "listing_result", "operation_id": self.operation_id, "accepted": accepted,
            "admission_id": self._listing_selection["admission_id"], "identity": self._listing_selection["identity"],
            "class": self._listing_selection["class"]}},
            _listing_ack=True)
        reply = client._await_call(identifier, deadline, "member_class_listing_acknowledged", {"kind", "operation_id", "admission_id", "identity", "class"})
        if (type(reply["operation_id"]) is not int or reply["operation_id"] != self.operation_id
                or reply["admission_id"] != self._listing_selection["admission_id"]
                or reply["identity"] != self._listing_selection["identity"]
                or reply["class"] != self._listing_selection["class"]):
            error = HostControlError("native listing acknowledgment names another Add")
            client._fail(error)
            raise error

    def _finish_listing(self, failure: BaseException | None) -> None:
        if not self._listing_done.done():
            if failure is None:
                self._listing_done.set_result(None)
            else:
                self._listing_done.set_exception(failure)
        self._forget_settled_listing()

    def _forget_settled_listing(self) -> None:
        client = self._client._client
        with client._condition:
            if self._listing_done.done() and (self._native_terminal or client._failure is not None):
                client._member_listings.pop(self.operation_id, None)

    async def join_listing(self) -> None:
        """Join the retained callback and ACK before application unlisting.

        Cancellation stops only this observer. Pipe loss cannot cancel an
        entered application mutation or establish its rollback. This result
        never establishes native activation, per-member Created, or recovery.
        For Unsettled, also perform the explicit native join: it may finish
        without ever requesting listing, and only that outcome releases this
        retained callback in that case.
        """
        if asyncio.get_running_loop() is not self._loop:
            raise HostControlError("join listing on the owning application event loop")
        await asyncio.shield(self._listing_done)

    def _await_result(self) -> dict[str, Any]:
        return self._client._await(self._operation, self.operation_id, self._deadline,
                                  {"reservation_not_accepted", "preflight_refused", "settled", "unsettled", "native_refused"})

    def result(self) -> dict[str, Any]:
        with self._lock:
            if not self._observed:
                try:
                    self._value = self._await_result()
                except TimeoutError:
                    self._error = MemberClassObservationTimeout(self)
                except HostTransportError:
                    self._error = MemberClassObservationUnknown(self)
                except BaseException as error:
                    self._error = error
                self._observed = True
            if self._error is not None:
                raise self._error
            assert self._value is not None
            return copy.deepcopy(self._value)


@dataclass(frozen=True)
class MemberMcpServiceIntent:
    """A current own server and requested subset, not account/grant authority."""
    server: str
    operations: tuple[str, ...]
    lifetime: dict[str, Any]

    def wire(self) -> dict[str, Any]:
        _source_identifier(self.server)
        if (not self.operations or len(self.operations) > 256 or len(set(self.operations)) != len(self.operations)
                or any(not isinstance(name, str) or not name or len(name) > 512 or name.strip() != name
                       or any(ord(char) < 32 or ord(char) == 127 for char in name) for name in self.operations)):
            raise HostControlError("invalid member MCP operation set")
        lifetime = _object(self.lifetime, {"bound", "unresolved"})
        bound = _object(lifetime["bound"], {"bound"}, {"window"})
        if bound["bound"] not in {"unrestricted", "window", "empty"}:
            raise HostControlError("invalid member MCP lifetime")
        if bound["bound"] == "window":
            window = _object(bound.get("window"), {"not_before_ms", "expires_at_ms"})
            if any(type(value) is not int or not 0 <= value <= 2**64 - 1 for value in window.values()):
                raise HostControlError("invalid member MCP lifetime window")
        elif "window" in bound:
            raise HostControlError("invalid member MCP lifetime payload")
        if not isinstance(lifetime["unresolved"], list):
            raise HostControlError("invalid member MCP unresolved constraints")
        return {"server": self.server, "operations": sorted(self.operations), "lifetime": lifetime}


@dataclass(frozen=True)
class MemberMcpAddIntent:
    """Explicit operator entry. Enrollment is only an actual login locator."""
    origin: str | dict[str, Any]
    console_enrollment_id: int
    services: tuple[MemberMcpServiceIntent, ...]

    def wire(self) -> dict[str, Any]:
        origin = self.origin
        if origin != "operator_self":
            selected = _object(origin, {"accepted_application_requirement"})
            selected = _object(selected["accepted_application_requirement"], {"source"})
            source = _requirement_source(selected["source"])
            origin = {"accepted_application_requirement": {"source": asdict(source)}}
        if (type(self.console_enrollment_id) is not int
                or not 1 <= self.console_enrollment_id <= 2**64 - 1
                or (not self.services and origin == "operator_self") or len(self.services) > 32
                or any(not isinstance(service, MemberMcpServiceIntent) for service in self.services)):
            raise HostControlError("invalid explicit member MCP operator intent")
        values = [service.wire() for service in self.services]
        if len({value["server"] for value in values}) != len(values):
            raise HostControlError("duplicate member MCP server selection")
        return {"origin": origin, "console_enrollment_id": self.console_enrollment_id, "services": values}


@dataclass(frozen=True)
class MemberMcpAdmissionSelection:
    """Exact private row key. admission_id is the original Add request.id."""
    admission_id: str
    identity: str
    class_id: str

    @classmethod
    def from_wire(cls, value: Any) -> MemberMcpAdmissionSelection:
        value = _object(value, {"admission_id", "identity", "class"})
        for item in value.values():
            _source_identifier(item)
        return cls(value["admission_id"], value["identity"], value["class"])

    def wire(self) -> dict[str, str]:
        return {"admission_id": self.admission_id, "identity": self.identity, "class": self.class_id}


def _requirement_source(value: Any) -> PrivateSourceEvidence:
    value = _object(value, {"resource_id", "revision", "sha256"})
    return PrivateSourceEvidence(**value)


@dataclass(frozen=True)
class ObservedMemberMcpRequirement:
    """Remote owner observation, never a synchronous current-owner assertion.

    Preserve the full source and ordered services from the actual nonvoided
    application row. Native approval and grants remain separate requirements.
    """
    selection: MemberMcpAdmissionSelection
    source: PrivateSourceEvidence
    services: tuple[MemberMcpServiceIntent, ...]

    @classmethod
    def from_wire(cls, value: Any) -> ObservedMemberMcpRequirement:
        value = _object(value, {"contract", "selection", "source", "services"})
        if value["contract"] != "observed_requirement" or not isinstance(value["services"], list) or len(value["services"]) > 32:
            raise HostControlError("invalid observed requirement")
        services = []
        for item in value["services"]:
            item = _object(item, {"server", "operations", "lifetime"})
            if not isinstance(item["operations"], list):
                raise HostControlError("invalid observed requirement operations")
            service = MemberMcpServiceIntent(item["server"], tuple(item["operations"]), item["lifetime"])
            service.wire()
            services.append(service)
        if len({item.server for item in services}) != len(services):
            raise HostControlError("duplicate observed requirement server")
        return cls(MemberMcpAdmissionSelection.from_wire(value["selection"]),
                   _requirement_source(value["source"]), tuple(services))

    def wire(self) -> dict[str, Any]:
        if not isinstance(self.selection, MemberMcpAdmissionSelection) or not isinstance(self.source, PrivateSourceEvidence):
            raise HostControlError("invalid observed requirement identity")
        if (not isinstance(self.services, tuple) or len(self.services) > 32
                or any(not isinstance(item, MemberMcpServiceIntent) for item in self.services)):
            raise HostControlError("invalid observed requirement services")
        result = {"contract": "observed_requirement", "selection": self.selection.wire(),
                  "source": asdict(self.source), "services": [item.wire() for item in self.services]}
        self.from_wire(result)
        return result


@dataclass(frozen=True)
class RetainedMemberMcpBindings:
    """Opaque owner-minted locators, never grants or public roster labels."""
    selection: MemberMcpAdmissionSelection
    bindings: tuple[str, ...]
    accepted_requirement: ObservedMemberMcpRequirement | None = None

    @classmethod
    def from_wire(cls, value: Any) -> RetainedMemberMcpBindings:
        value = _object(value, {"selection", "bindings"}, {"accepted_requirement"})
        refs = value["bindings"]
        if not isinstance(refs, list) or len(refs) > 32 or any(not isinstance(ref, str) for ref in refs):
            raise HostControlError("invalid member MCP binding set")
        for ref in refs:
            _source_identifier(ref)
        if len(set(refs)) != len(refs):
            raise HostControlError("duplicate member MCP binding")
        selected = MemberMcpAdmissionSelection.from_wire(value["selection"])
        requirement = None
        if "accepted_requirement" in value:
            requirement = ObservedMemberMcpRequirement.from_wire(value["accepted_requirement"])
            if requirement.selection != selected or len(requirement.services) != len(refs):
                raise HostControlError("binding requirement association mismatch")
        return cls(selected, tuple(refs), requirement)

    def wire(self) -> dict[str, Any]:
        result = {"selection": self.selection.wire(), "bindings": list(self.bindings)}
        if self.accepted_requirement is not None:
            result["accepted_requirement"] = self.accepted_requirement.wire()
        self.from_wire(result)
        return result


class MemberMcpApplication:
    """Callbacks to the application's SAME durable hidden admission row.

    Construct on the application's event loop before starting its runtime.
    retain(value) must CAS the exact nonvoided admitting row, persist all refs,
    and return only after commit. Exact replay may ACK; mismatched refs, class,
    identity or admission must refuse. lookup(selection) returns the exact
    retained value for an admitting/listed row; missing/void/retired/unreadable
    ordinary rows refuse, never imply []. For observed requirements, retain the
    locators and full association after void/retirement for cleanup only. This
    retained value cannot authorize publication. Neither callback publishes
    the member. With observed_requirement, retain must also compare the full
    expected source/ordered intent with the nonvoided row before committing refs.
    The optional observed_requirement(selection, source) callback re-reads that
    actual row before and after native issue. Void/mismatch refuses publication;
    exact grant cleanup is retained and retried at the next native startup.
    This is explicitly an observed requirement, not synchronous currentness.
    Construct this over the loaded store before native runtime startup, since
    cold registration can call lookup before integration.start. Await join()
    before stopping/closing that store; runtime shutdown repeats the same
    idempotent join without cancelling entered writes.
    """
    def __init__(self, retain: Callable[[RetainedMemberMcpBindings], Awaitable[None]],
                 lookup: Callable[[MemberMcpAdmissionSelection], Awaitable[RetainedMemberMcpBindings]],
                 *, observed_requirement: Callable[[MemberMcpAdmissionSelection, PrivateSourceEvidence], Awaitable[ObservedMemberMcpRequirement | None]] | None = None):
        if not callable(retain) or not callable(lookup) or (observed_requirement is not None and not callable(observed_requirement)):
            raise HostControlError("member MCP retention and lookup callbacks are required")
        self._retain = retain
        self._lookup = lookup
        self._observed_requirement = observed_requirement
        self._loop = asyncio.get_running_loop()
        self._lock = threading.Lock()
        self._pending: set[tuple[int, int]] = set()
        self._issued: weakref.WeakKeyDictionary[PrivateHostClient, int] = weakref.WeakKeyDictionary()
        self._tasks: set[asyncio.Task[None]] = set()
        self._failures: list[BaseException] = []
        self._closed = False
        self._join_task: asyncio.Task[None] | None = None

    @property
    def requirement_contract(self) -> str | None:
        """Advertise only the explicitly selected remote observed contract."""
        return "observed_requirement" if self._observed_requirement is not None else None

    def _request(self, client: PrivateHostClient, reply: dict[str, Any]) -> None:
        reply = _object(reply, {"kind", "exchange_id", "selection", "bindings"},
                        {"accepted_requirement", "requirement_source"})
        if type(reply["exchange_id"]) is not int or not 1 <= reply["exchange_id"] <= 2**64 - 1:
            raise HostControlError("invalid member MCP exchange")
        selection = MemberMcpAdmissionSelection.from_wire(reply["selection"])
        source = _requirement_source(reply["requirement_source"]) if "requirement_source" in reply else None
        if source is not None and (reply["bindings"] is not None or "accepted_requirement" in reply):
            raise HostControlError("ambiguous member MCP requirement request")
        value = {"selection": selection.wire(), "bindings": reply["bindings"]}
        if "accepted_requirement" in reply:
            if reply["bindings"] is None:
                raise HostControlError("requirement association without retained bindings")
            value["accepted_requirement"] = reply["accepted_requirement"]
        proposed = None if reply["bindings"] is None else RetainedMemberMcpBindings.from_wire(value)
        key = (id(client), reply["exchange_id"])
        with self._lock:
            if self._closed or key[1] <= self._issued.get(client, 0) or len(self._pending) >= 16:
                raise HostControlError("member MCP callback is unavailable or replayed")
            self._pending.add(key)
            self._issued[client] = key[1]
            # Scheduling is inside the same close fence. Shutdown cannot
            # finish before this already queued start has been observed.
            try:
                self._loop.call_soon_threadsafe(self._start, client, key, selection, proposed, source)
            except BaseException:
                self._pending.discard(key)
                raise

    def _start(self, client: PrivateHostClient, key: tuple[int, int],
               selection: MemberMcpAdmissionSelection, proposed: RetainedMemberMcpBindings | None,
               source: PrivateSourceEvidence | None = None) -> None:
        task = self._loop.create_task(self._run(client, key, selection, proposed, source))
        self._tasks.add(task)
        task.add_done_callback(self._tasks.discard)

    async def _run(self, client: PrivateHostClient, key: tuple[int, int],
                   selection: MemberMcpAdmissionSelection, proposed: RetainedMemberMcpBindings | None,
                   source: PrivateSourceEvidence | None = None) -> None:
        retained = None
        requirement = None
        try:
            with self._lock:
                closed = self._closed
            if closed or client._failure is not None:
                raise HostTransportError("member MCP callback closed before application entry")
            if source is not None:
                if self._observed_requirement is None:
                    raise HostControlError("observed requirement capability is unavailable")
                requirement = await self._observed_requirement(selection, source)
                if requirement is not None:
                    if not isinstance(requirement, ObservedMemberMcpRequirement) or requirement.selection != selection or requirement.source != source:
                        raise HostControlError("observed requirement returned another source")
                    requirement.wire()
            elif proposed is not None:
                await self._retain(proposed)
                retained = proposed
            else:
                retained = await self._lookup(selection)
                if not isinstance(retained, RetainedMemberMcpBindings) or retained.selection != selection:
                    raise HostControlError("member MCP lookup returned another admission")
                retained.wire()
        except BaseException as error:
            # Refuse native continuation, but retain application uncertainty for
            # shutdown/join. An exception is never evidence of rollback.
            self._failures.append(error)
            retained = None
            requirement = None
        try:
            await asyncio.to_thread(self._ack, client, key[1], selection, retained, requirement)
        except HostReplyError as error:
            # Native observed this exact ACK and returned a typed terminal
            # refusal, for example after cancellation. No waiter is stranded.
            self._failures.append(error)
        except BaseException as error:
            self._failures.append(error)
            # An ACK that was never written leaves native waiting on this
            # exchange. Existing transport failure closes the command half,
            # waking its waiter without pretending the app write rolled back.
            await asyncio.to_thread(client._fail, error)
        finally:
            with self._lock:
                self._pending.discard(key)

    @staticmethod
    def _ack(client: PrivateHostClient, exchange_id: int, selection: MemberMcpAdmissionSelection,
             retained: RetainedMemberMcpBindings | None,
             requirement: ObservedMemberMcpRequirement | None = None) -> None:
        operation = {"kind": "mcp_result", "exchange_id": exchange_id, "selection": selection.wire(),
                     "retained": None if retained is None else retained.wire()}
        if requirement is not None:
            operation["requirement"] = requirement.wire()
        identifier, deadline = client._start_call({"kind": "member_class", "operation": operation}, _listing_ack=True)
        result = client._await_call(identifier, deadline, "member_class_mcp_acknowledged", {"kind", "exchange_id"})
        if type(result["exchange_id"]) is not int or result["exchange_id"] != exchange_id:
            raise HostControlError("member MCP acknowledgement changed correlation")

    def _close_entry(self) -> None:
        with self._lock:
            self._closed = True

    async def join(self) -> None:
        """Close entry and join writes before the application closes its store.

        Idempotent with runtime shutdown, including retained cleanup errors.
        """
        self._close_entry()
        if self._join_task is None:
            self._join_task = self._loop.create_task(self._drain())
            self._join_task.add_done_callback(lambda done: None if done.cancelled() else done.exception())
        await asyncio.shield(self._join_task)

    async def _drain(self) -> None:
        # A reader may have scheduled _start just before the close fence.
        await asyncio.sleep(0)
        while self._tasks:
            await asyncio.shield(asyncio.gather(*tuple(self._tasks)))
        with self._lock:
            pending = bool(self._pending)
        if pending or self._failures:
            raise HostTransportError("member MCP application callback outcome requires reconciliation")
