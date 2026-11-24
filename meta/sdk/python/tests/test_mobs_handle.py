"""A mob handle names its mob on the wire; the handle of a launch's own mob names none.

``runtime.mob_handle()`` is the handle of the mob a launch names. A request it
sends carries a top-level ``mob_id`` only when the caller passed one to a
method that takes it; the handle adds none of its own. Its JSON-RPC requests
leave through one place, ``MetaRuntime._rpc``, except the two it sends as a
multipart upload over HTTP, and its two event streams are HTTP reads.

``runtime.mob(mob_id)`` is the same handle for a mob named by its id. It adds
that id in each of those three places: to every JSON-RPC request, to the
multipart payload, and to the two stream URLs. A gateway that does not
advertise ``mob_addressing`` would ignore the id and answer for its own mob,
so the handle reads the capability first (once per started transport) and
refuses locally without it. ``runtime.mobs`` creates, lists and reads the
mobs of the host and keeps nothing between calls.

A handle also offers the runtime's calls by identity for its mob: under the
runtime's names where the handle has none of that name, as ``send_identity``,
``status_identity`` and ``reconcile_identity`` where it has, and through
``agent(identity)`` and ``identities``.

These tests hold that for every public name of ``MobHandle``: a method added
later is driven here from its signature, or fails here until it is listed.
"""

from __future__ import annotations

import asyncio
import dataclasses
import inspect
import json
import sys
import time
from pathlib import Path
from typing import Any
from urllib.parse import urlsplit

import pytest

import meerkat_meta
import meerkat_meta.errors as errors_module
import meerkat_meta.runtime as runtime_module
import meerkat_meta.types as types_module
from meerkat_meta.errors import (
    CapabilityUnavailableError,
    ContractMismatchError,
    RpcError,
    StorageResolutionError,
)
from meerkat_meta.identities import AgentRef
from meerkat_meta.identity_first_models import CompletionCursor, DeliveryScope, DispatchInput
from meerkat_meta.live import (
    ActiveLiveChannelHandle,
    LiveAssistantOutputAddress,
    LiveExecutionIdentityV1,
    LivePlaybackOwnerReadiness,
    PendingLiveChannelHandle,
)
from meerkat_meta.models import DiscoverySpec
from meerkat_meta.runtime import (
    IdentitiesHandle,
    IdentityAgentHandle,
    MetaRuntime,
    MobHandle,
    _rpc_error_from_payload,
)
from meerkat_meta.types import EventQuery

from .test_identity_first_turn_tickets import _make_runtime

# Every wire method ``MobHandle`` sends. A method that sends a new one adds it
# here; the last test fails while this list and the handle differ.
WIRE_METHODS = frozenset(
    """
    live/webrtc/answer
    meta/agent_definitions/list
    meta/agent_memory/forget meta/agent_memory/forget_in_scope
    meta/agent_memory/manifest meta/agent_memory/purge meta/agent_memory/recall
    meta/agent_memory/remember meta/agent_memory/update
    meta/attach_existing_session
    meta/blob/get meta/blob/upload
    meta/call_tool meta/cancel_flow meta/capabilities meta/collect_completed
    meta/cross_mob/directory meta/cross_mob/peer_info
    meta/cross_mob/unwire_local meta/cross_mob/wire_local
    meta/decisions/decide meta/decisions/get meta/decisions/list
    meta/decisions/present meta/decisions/rendering meta/decisions/withdraw
    meta/delivery/history meta/delivery/send
    meta/ensure_member meta/events/subscribe meta/find_members meta/flow_status
    meta/force_cancel_member meta/fork_helper
    meta/gating/audit meta/gating/decide meta/gating/evaluate
    meta/gating/pending meta/gating/questions meta/gating/withdraw
    meta/get_member
    meta/identity/resolved_tools meta/identity/routing_status meta/identity/warm
    meta/list_flows meta/list_members meta/list_runs
    meta/live/close meta/live/commit_input meta/live/interrupt meta/live/open
    meta/live/playback_complete meta/live/playback_owner/register
    meta/live/playback_owner/revoke meta/live/refresh
    meta/live/replacement_required meta/live/send_input meta/live/status
    meta/live/truncate
    meta/member_health meta/member_status
    meta/memory/index meta/memory/query meta/memory/stores
    meta/mob_announcements/announce meta/mob_announcements/audit/list
    meta/mob_announcements/declarations/apply
    meta/mob_announcements/declarations/get
    meta/mob_announcements/deliveries/dismiss
    meta/mob_announcements/deliveries/list
    meta/mob_announcements/deliveries/observe
    meta/mob_announcements/deliveries/reconcile
    meta/mob_announcements/deliveries/redeliver
    meta/mob_announcements/escalations/list
    meta/mob_announcements/get
    meta/mob_announcements/members/get meta/mob_announcements/members/list
    meta/mob_announcements/sources/advance meta/mob_announcements/sources/end
    meta/mob_announcements/sources/get meta/mob_announcements/sources/list
    meta/mob_announcements/sources/pause meta/mob_announcements/sources/put
    meta/mob_announcements/sources/record_failure
    meta/mob_announcements/sources/resume
    meta/mob_announcements/subscriptions/cancel
    meta/mob_announcements/subscriptions/get
    meta/mob_announcements/subscriptions/list
    meta/mob_announcements/subscriptions/pause
    meta/mob_announcements/subscriptions/put
    meta/mob_announcements/subscriptions/resume
    meta/mob_events/query meta/mob_events/subscribe
    meta/mob_labels/delete meta/mob_labels/get meta/mob_labels/set
    meta/mobpacks/apply_operation meta/mobpacks/catalogs meta/mobpacks/create
    meta/mobpacks/delete meta/mobpacks/deploy meta/mobpacks/deploy_command
    meta/mobpacks/export meta/mobpacks/get meta/mobpacks/import
    meta/mobpacks/list meta/mobpacks/redo meta/mobpacks/save
    meta/mobpacks/source meta/mobpacks/templates meta/mobpacks/undo
    meta/mobpacks/validate
    meta/models/catalog meta/peer_pubkey meta/query_events
    meta/reconcile meta/reconcile_edges meta/rediscover
    meta/reload_member meta/respawn_member meta/retire_member
    meta/routing/resolve meta/routing/routes/add meta/routing/routes/delete
    meta/routing/routes/list
    meta/run_flow
    meta/run_labels/delete meta/run_labels/get meta/run_labels/set
    meta/schedule/tools meta/send_message meta/session_store/bigquery
    meta/skills/catalog meta/spawn_helper meta/spawn_member meta/status
    meta/stop_member_run meta/storage/doctor meta/tools/catalog meta/wait_ready
    meta/workgraph/attention/list meta/workgraph/attention/pause
    meta/workgraph/attention/prune meta/workgraph/attention/reassign
    meta/workgraph/attention/resume
    meta/workgraph/block meta/workgraph/claim meta/workgraph/close
    meta/workgraph/create meta/workgraph/events meta/workgraph/evidence/add
    meta/workgraph/get
    meta/workgraph/goal/confirm meta/workgraph/goal/create
    meta/workgraph/goal/request_close meta/workgraph/goal/status
    meta/workgraph/link meta/workgraph/list meta/workgraph/policy/escalate
    meta/workgraph/ready meta/workgraph/release meta/workgraph/snapshot
    meta/workgraph/update
    """.split()
)

# The wire methods of the runtime's calls by identity, which a handle sends
# for its mob as well.
IDENTITY_FIRST_WIRE_METHODS = frozenset(
    """
    meta/compact_member meta/completion_cursor meta/delete_identity meta/dispatch
    meta/inspect_identity meta/instruction_status
    meta/member_activate_instruction meta/member_instruction_activations
    meta/prune_member_system_rows meta/reconcile_identity meta/recover_delivery
    meta/registered_spec meta/reset meta/respawn meta/retire
    meta/revoke_instruction meta/send meta/send_status meta/set_instruction
    meta/status_identity meta/status_identity_bootstrap meta/subscribe
    meta/tool_grants/audit meta/tool_grants/grant meta/tool_grants/list
    meta/tool_grants/revoke meta/turn_result meta/wait_for_completion
    meta/wait_identity_bootstrap
    """.split()
)

# The runtime's calls by identity that a handle offers under the runtime's
# own name ...
FORWARDED = frozenset(
    """
    send_status dispatch dispatch_text recover_delivery turn_result subscribe
    registered_spec inspect_identity completion_cursor respawn retire reset
    delete_identity compact_member wait_until_ready identity_bootstrap_status
    wait_identity_bootstrap activate_member_instruction
    member_instruction_activations set_instruction revoke_instruction
    instruction_status prune_member_system_rows grant_tool revoke_tool_grant
    list_tool_grants tool_grant_audit
    """.split()
)
# ... and the three whose name the handle has already for another call: the
# handle's name for the runtime's call, the runtime's name, and the wire
# method of each of the two calls that share the runtime's name.
RENAMED = {
    "send_identity": ("send", "meta/send", "meta/send_message"),
    "status_identity": ("status", "meta/status_identity", "meta/status"),
    "reconcile_identity": ("reconcile", "meta/reconcile_identity", "meta/reconcile"),
}
# What stays the runtime's alone: its own lifecycle and wiring, and the calls
# that are the host's whichever mob is named.
RUNTIME_ONLY = frozenset(
    """
    connect serve shutdown wait_gateway_ended set_rust_http_base
    on_schedule_fire on_decision_notice sse_bridge asgi mob_handle mob
    list_identities
    realm_mcp_servers realm_mcp_install realm_mcp_set_secret
    realm_mcp_remove_secret
    """.split()
)

# The wire methods that leave as a multipart upload over HTTP: a blob upload
# always, a message when it carries attachments.
MULTIPART_ALWAYS = frozenset({"meta/blob/upload"})
MULTIPART_WITH_ATTACHMENTS = frozenset({"meta/send_message"})

# Public names that send nothing themselves.
SENDS_NOTHING = frozenset({"tool_caller", "agent"})

# The two event streams and the HTTP path each reads.
STREAM_PATHS = {"subscribe_agent": "/agents/x/events", "subscribe_mob": "/mob/events"}

# The methods with a ``mob_id`` argument of their own: the mob an announcement,
# a source or an audit page belongs to, and the mob an event query reads.
# ``announcement_get`` requires it; the others send it only when the caller
# passes one.
OWN_MOB_ID_REQUIRED = frozenset({"announcement_get"})
OWN_MOB_ID_OPTIONAL = frozenset(
    {
        "announce",
        "announcement_source_put",
        "announcement_source_pause",
        "announcement_source_resume",
        "announcement_source_end",
        "announcement_source_get",
        "announcement_source_list",
        "announcement_source_advance",
        "announcement_source_record_failure",
        "announcement_audit_list",
        "query_events",
    }
)
# Of those, the calls whose mob is the mob the request is answered for and
# nothing an earlier gateway reads so: the handle of the launch's own mob
# reads the capability before it sends one.
OWN_MOB_ID_NEEDS_THE_CAPABILITY = frozenset({"query_events"})

HTTP_BASE = "http://127.0.0.1:9"
LIVE_FIXTURE = (
    Path(__file__).parents[3] / "crates/meerkat-meta/tests/fixtures/live_contracts_v1.json"
)


def _public_names() -> list[str]:
    names = [
        name
        for name, member in vars(MobHandle).items()
        if not name.startswith("_") and inspect.isfunction(member)
    ]
    return sorted(names)


class _PlaybackOwner:
    """A local media owner that prepares an offer and does nothing else."""

    async def prepare(self, pending: Any) -> str:
        return "offer"

    async def accept_answer(self, answer_sdp: str) -> None:
        return None

    async def activate(self, active: Any) -> None:
        return None

    async def abort(self) -> None:
        return None


class _Recorder:
    """A transport that records every request and answers an empty result.

    With ``features`` it answers ``meta/capabilities`` as a gateway that
    advertises those feature capabilities; ``answers`` gives a method its
    own ``{"result": ...}`` or ``{"error": ...}``.
    """

    def __init__(
        self,
        features: tuple[str, ...] | None = None,
        answers: dict[str, dict[str, Any]] | None = None,
    ) -> None:
        self.requests: list[dict[str, Any]] = []
        # The transport timeout each request was given (None: the transport's own).
        self.timeouts: list[float | None] = []
        self.request_timeout = 60.0
        self.features = features
        self.answers = answers or {}

    def _answer(self, request: dict[str, Any]) -> dict[str, Any]:
        self.requests.append(request)
        method = request.get("method")
        if method in self.answers:
            return {"jsonrpc": "2.0", "id": request.get("id"), **self.answers[method]}
        result: dict[str, Any] = {}
        if method == "meta/member_status":
            # A final status ends a status poll at its first read.
            result = {"is_final": True}
        elif method == "meta/capabilities" and self.features is not None:
            result = {
                "contract_version": "0.9.15",
                "methods": [],
                "loaded_modules": [],
                "feature_capabilities": list(self.features),
            }
        return {"jsonrpc": "2.0", "id": request.get("id"), "result": result}

    def sent(self) -> list[tuple[str, dict[str, Any]]]:
        return [
            (str(request.get("method")), dict(request.get("params") or {}))
            for request in self.requests
        ]

    async def send_async(self, request: dict[str, Any], *, timeout: float | None = None):
        await asyncio.sleep(0)
        self.timeouts.append(timeout)
        return self._answer(request)

    def send_sync(self, request: dict[str, Any]):
        self.timeouts.append(None)
        return self._answer(request)


class _HttpResponse:
    def __init__(self, body: bytes) -> None:
        self._body = body

    def __enter__(self) -> _HttpResponse:
        return self

    def __exit__(self, *exc: Any) -> bool:
        return False

    def read(self, size: int = -1) -> bytes:
        body, self._body = self._body, b""
        return body

    def read1(self, size: int = -1) -> bytes:
        return self.read(size)

    def close(self) -> None:
        return None


@dataclasses.dataclass
class _Run:
    """What one call of one handle method sent."""

    # (method, params) of every request the runtime's transport was given.
    transport: list[tuple[str, dict[str, Any]]] = dataclasses.field(default_factory=list)
    # The transport timeout each of those requests was given (None: the transport's own).
    timeouts: list[float | None] = dataclasses.field(default_factory=list)
    # The method of every request that passed ``MetaRuntime._rpc``.
    seam: list[str] = dataclasses.field(default_factory=list)
    # (method, params) of every JSON-RPC request posted as a multipart upload.
    multipart: list[tuple[str, dict[str, Any]]] = dataclasses.field(default_factory=list)
    # (method, params) of every JSON-RPC request posted to the console route.
    console: list[tuple[str, dict[str, Any]]] = dataclasses.field(default_factory=list)
    # Every event stream URL opened.
    streams: list[str] = dataclasses.field(default_factory=list)
    # What the call raised, if it raised.
    error: BaseException | None = None

    def requests(self) -> list[tuple[str, dict[str, Any]]]:
        return self.transport + self.multipart


def _typed_values(handle: MobHandle) -> dict[str, Any]:
    """A value for each parameter type a signature cannot be given a plain one for."""
    live = json.loads(LIVE_FIXTURE.read_text(encoding="utf-8"))
    active = ActiveLiveChannelHandle.from_dict(live["active_channel_handle"])
    return {
        "DiscoverySpec": DiscoverySpec(role="worker", agent_identity="worker-1"),
        "PendingLiveChannelHandle": PendingLiveChannelHandle.from_dict(
            live["pending_channel_handle"]
        ),
        "ActiveLiveChannelHandle": active,
        "LivePlaybackOwnerReadiness": LivePlaybackOwnerReadiness.from_dict(
            live["playback_owner_readiness"]
        ),
        "LiveAssistantOutputAddress": LiveAssistantOutputAddress(
            channel_id=active.channel_id, output_id="output-1", content_index=0
        ),
        "LiveExecutionIdentityV1": LiveExecutionIdentityV1.from_dict(live["execution_identity"]),
        "LivePlaybackOwner": _PlaybackOwner(),
        "MobHandle": handle,
        "AnnouncementMember": {"mob_id": "mob-a", "identity": "worker-1"},
        "AnnouncementRoleSelector": {"mob_id": "mob-a", "role": "worker"},
        "bytes": b"png",
        "CompletionCursor": CompletionCursor(epoch=1, turns=1),
        "IdentityChangeRequest": AgentRef(
            name="worker-1", expected_spec_digest="digest-1", request_id="request-1"
        ),
    }


DELIVERY_SCOPE = DeliveryScope(
    identity="worker-1",
    agent_runtime_id="runtime-1",
    generation=1,
    lease_fencing_token=1,
    member={},
)

# Arguments a signature alone does not give: a value a method refuses to be
# without although its parameter has a default, or one its annotation does
# not name.
BY_METHOD: dict[str, dict[str, Any]] = {
    "send": {"message": "hello"},
    "send_message": {"message": "hello"},
    "send_cross_mob": {"message": "hello"},
    "wait_all": {"member_ids": ["worker-1"]},
    "dispatch": {"dispatch_input": DispatchInput(content="hello", origin="system")},
    "grant_tool": {"tool": "x"},
    "gating_answer": {"answers": [{"question_id": "q-1", "answer": "yes"}]},
    "wait_until_ready": {"identities": ["worker-1"]},
    "recover_delivery": {"scope": DELIVERY_SCOPE},
}

# The same for the methods of an agent handle and of the long-term agents.
AGENT_BY_METHOD: dict[str, dict[str, Any]] = {
    "recover_delivery": {"scope": DELIVERY_SCOPE},
    **{
        name: {"dispatch_input": DispatchInput(content="hello", origin="system")}
        for name in ("dispatch", "dispatch_and_wait", "dispatch_and_wait_outcome")
    },
}
IDENTITIES_BY_METHOD: dict[str, dict[str, Any]] = {
    "withdraw": {"request_id": "request-1"},
    "operation": {"request_id": "request-1"},
    "plan": {"change": "archive"},
}

# A second call of a method whose requests leave another way with other arguments.
VARIANTS: dict[str, dict[str, Any]] = {
    "send": {"message": "hello", "attachments": [b"png"]},
}


def _plain_value(annotation: str) -> Any:
    head = annotation.replace(" ", "").split("|")[0].strip("'\"")
    if head == "str":
        return "x"
    if head == "int":
        return 1
    if head == "float":
        return 1.0
    if head == "bool":
        return False
    if head.startswith(("dict", "Mapping")) or head in ("Any", "object"):
        return {}
    if head.startswith(("list", "Sequence", "tuple", "Iterable")):
        return []
    raise LookupError(head)


def _arguments(handle: MobHandle, name: str, given: dict[str, Any]) -> dict[str, Any]:
    """The arguments of one call: ``given``, and a value for every other required parameter."""
    typed = _typed_values(handle)
    arguments = dict(given)
    for parameter in inspect.signature(getattr(handle, name)).parameters.values():
        if parameter.name in arguments:
            continue
        if parameter.kind in (parameter.VAR_POSITIONAL, parameter.VAR_KEYWORD):
            continue
        if parameter.default is not parameter.empty:
            continue
        annotation = str(parameter.annotation)
        try:
            arguments[parameter.name] = _plain_value(annotation)
        except LookupError as unknown:
            head = str(unknown.args[0])
            assert head in typed, (
                f"MobHandle.{name}({parameter.name}: {annotation}): "
                "add a value for this parameter type to _typed_values"
            )
            arguments[parameter.name] = typed[head]
    return arguments


def _multipart_request(body: bytes) -> tuple[str, dict[str, Any]]:
    payload = json.loads(body.split(b"\r\n\r\n", 2)[1].split(b"\r\n--", 1)[0])
    return str(payload["method"]), dict(payload.get("params") or {})


def _runtime_over(
    monkeypatch: pytest.MonkeyPatch,
    run: _Run,
    features: tuple[str, ...] | None = None,
    answers: dict[str, dict[str, Any]] | None = None,
) -> tuple[MetaRuntime, _Recorder]:
    """A started runtime over a recording transport; ``run`` collects what leaves it."""
    transport = _Recorder(features, answers)
    runtime: MetaRuntime = _make_runtime(transport)
    runtime._rust_http_base = HTTP_BASE

    through_runtime = runtime._rpc

    async def seam(method: str, params: dict[str, Any] | None = None, **options: Any) -> Any:
        run.seam.append(method)
        return await through_runtime(method, params, **options)

    runtime._rpc = seam  # type: ignore[method-assign]

    def urlopen(request: Any, *args: Any, **kwargs: Any) -> _HttpResponse:
        url = request.full_url
        if urlsplit(url).path == "/console/rpc/multipart":
            run.multipart.append(_multipart_request(request.data))
            return _HttpResponse(b'{"jsonrpc": "2.0", "id": "1", "result": {}}')
        if urlsplit(url).path == "/console/rpc":
            payload = json.loads(request.data)
            run.console.append((str(payload["method"]), dict(payload.get("params") or {})))
            return _HttpResponse(b'{"jsonrpc": "2.0", "id": "1", "result": {}}')
        run.streams.append(url)
        return _HttpResponse(b"")

    monkeypatch.setattr(runtime_module.urllib_request, "urlopen", urlopen)
    return runtime, transport


async def _drive(
    monkeypatch: pytest.MonkeyPatch,
    name: str,
    given: dict[str, Any] | None = None,
    *,
    mob_id: str | None = None,
    features: tuple[str, ...] | None = None,
    of: Any = None,
) -> _Run:
    """Call ``MobHandle.<name>`` once and return what it sent.

    The handle is the default one, or with ``mob_id`` the handle of that mob.
    With ``of`` the call is made on what ``of(handle)`` gives (an agent
    handle, the long-term agents) instead of on the handle itself. The
    transport answers every request with an empty result, so a method may
    raise on the answer; what it sent before that is what is asked here.
    """
    run = _Run()
    runtime, transport = _runtime_over(monkeypatch, run, features)
    handle = runtime.mob_handle() if mob_id is None else runtime.mob(mob_id)
    target = handle if of is None else of(handle)
    arguments = _arguments(target, name, given if given is not None else BY_METHOD.get(name, {}))
    try:
        result = getattr(target, name)(**arguments)
        if inspect.isasyncgen(result):
            try:
                await asyncio.wait_for(result.__anext__(), timeout=0.2)
            finally:
                await result.aclose()
        elif inspect.isawaitable(result):
            await asyncio.wait_for(result, timeout=5)
    except Exception as error:  # noqa: BLE001 - an empty answer is refused after the request left
        run.error = error
    run.transport = transport.sent()
    run.timeouts = list(transport.timeouts)
    return run


def _cases() -> list[tuple[str, dict[str, Any] | None]]:
    cases: list[tuple[str, dict[str, Any] | None]] = [(name, None) for name in _public_names()]
    cases.extend((name, given) for name, given in sorted(VARIANTS.items()))
    return cases


def _case_id(case: tuple[str, dict[str, Any] | None]) -> str:
    name, given = case
    return name if given is None else f"{name}[{','.join(sorted(given))}]"


def test_the_tables_name_only_methods_the_handle_has() -> None:
    names = set(_public_names())
    listed = (
        SENDS_NOTHING
        | set(STREAM_PATHS)
        | OWN_MOB_ID_REQUIRED
        | OWN_MOB_ID_OPTIONAL
        | set(BY_METHOD)
        | set(VARIANTS)
    )
    assert listed <= names, sorted(listed - names)
    # Every method that takes a ``mob_id`` of its own is listed, and no other.
    with_own_mob_id = {
        name
        for name in names
        if "mob_id" in inspect.signature(getattr(MobHandle, name)).parameters
    }
    assert with_own_mob_id == OWN_MOB_ID_REQUIRED | OWN_MOB_ID_OPTIONAL
    for name in OWN_MOB_ID_REQUIRED | OWN_MOB_ID_OPTIONAL:
        default = inspect.signature(getattr(MobHandle, name)).parameters["mob_id"].default
        assert (default is inspect.Parameter.empty) == (name in OWN_MOB_ID_REQUIRED), name


@pytest.mark.asyncio
@pytest.mark.parametrize("case", _cases(), ids=_case_id)
async def test_the_default_handle_adds_no_mob_id_of_its_own(
    monkeypatch: pytest.MonkeyPatch, case: tuple[str, dict[str, Any] | None]
) -> None:
    name, given = case
    run = await _drive(monkeypatch, name, given)

    if name in SENDS_NOTHING:
        assert run.requests() == [] and run.streams == []
        return
    if name in STREAM_PATHS:
        assert run.requests() == []
        assert run.streams == [HTTP_BASE + STREAM_PATHS[name]]
        # The stream's URL names no mob.
        assert urlsplit(run.streams[0]).query == ""
        return

    assert run.requests(), f"MobHandle.{name} sent nothing: give it arguments in BY_METHOD"
    assert run.streams == []
    for method, params in run.requests():
        if name in OWN_MOB_ID_REQUIRED:
            # The caller's own argument, as given.
            assert params.get("mob_id") == "x", (method, params)
        else:
            assert "mob_id" not in params, (method, params)


@pytest.mark.asyncio
@pytest.mark.parametrize("name", sorted(OWN_MOB_ID_REQUIRED | OWN_MOB_ID_OPTIONAL))
async def test_a_methods_own_mob_id_is_sent_as_the_caller_gave_it(
    monkeypatch: pytest.MonkeyPatch, name: str
) -> None:
    asks_first = name in OWN_MOB_ID_NEEDS_THE_CAPABILITY
    run = await _drive(
        monkeypatch,
        name,
        {"mob_id": "mob-b"},
        features=(MOB_ADDRESSING,) if asks_first else None,
    )

    sent = _addressed(run) if asks_first else run.requests()
    assert sent
    for method, params in sent:
        assert params.get("mob_id") == "mob-b", (method, params)


@pytest.mark.asyncio
async def test_every_wire_method_leaves_through_the_one_seam(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    through_seam: set[str] = set()
    by_multipart: set[str] = set()
    for name, given in _cases():
        run = await _drive(monkeypatch, name, given)
        # Whatever the transport was given passed MetaRuntime._rpc, in order.
        assert [method for method, _ in run.transport] == run.seam, name
        through_seam.update(run.seam)
        by_multipart.update(method for method, _ in run.multipart)

    assert by_multipart == MULTIPART_ALWAYS | MULTIPART_WITH_ATTACHMENTS
    assert through_seam == (WIRE_METHODS | IDENTITY_FIRST_WIRE_METHODS) - MULTIPART_ALWAYS
    assert len(WIRE_METHODS) == 163
    assert len(IDENTITY_FIRST_WIRE_METHODS) == 29
    assert not WIRE_METHODS & IDENTITY_FIRST_WIRE_METHODS


# -- A handle for a mob named by its id ---------------------------------------

# The feature capability of a gateway that honours ``mob_id``, and the one of
# a gateway that serves the mobs of its host.
MOB_ADDRESSING = "mob_addressing"
HOSTED_MOBS = "hosted_mobs"
# The capability read: the host's own answer, so it names no mob.
CAPABILITY_READ = ("meta/capabilities", {})


def _addressed(run: _Run) -> list[tuple[str, dict[str, Any]]]:
    """The requests of a named handle's call, after its capability read."""
    assert run.transport[:1] == [CAPABILITY_READ], run.transport[:1]
    return run.transport[1:] + run.multipart


@pytest.mark.asyncio
async def test_obtaining_a_handle_sends_nothing(monkeypatch: pytest.MonkeyPatch) -> None:
    run = _Run()
    runtime, transport = _runtime_over(monkeypatch, run, (MOB_ADDRESSING, HOSTED_MOBS))

    handle = runtime.mob("mob-b")
    mobs = runtime.mobs

    assert transport.sent() == [] and run.multipart == [] and run.streams == []
    assert mobs is not None
    # The handle of a named mob is the handle a launch's own mob has, with an id.
    assert type(handle) is MobHandle
    assert handle.mob_id == "mob-b"
    assert runtime.mob_handle().mob_id is None
    # A mob is named by text.
    for not_an_id in (None, 7, b"mob-b"):
        with pytest.raises(TypeError):
            runtime.mob(not_an_id)  # type: ignore[arg-type]


@pytest.mark.asyncio
@pytest.mark.parametrize("case", _cases(), ids=_case_id)
async def test_a_named_handle_sends_its_mob_id_on_every_call(
    monkeypatch: pytest.MonkeyPatch, case: tuple[str, dict[str, Any] | None]
) -> None:
    name, given = case
    run = await _drive(monkeypatch, name, given, mob_id="mob-b", features=(MOB_ADDRESSING,))

    if name in SENDS_NOTHING:
        assert run.requests() == [] and run.streams == []
        return
    sent = _addressed(run)
    if name in STREAM_PATHS:
        assert sent == []
        assert run.streams == [HTTP_BASE + STREAM_PATHS[name] + "?mob_id=mob-b"]
        return

    assert sent, f"MobHandle.{name} sent nothing: give it arguments in BY_METHOD"
    assert run.streams == []
    # ``announcement_get`` is given a mob of its own here; that one is sent.
    expected = "x" if name in OWN_MOB_ID_REQUIRED else "mob-b"
    for method, params in sent:
        assert params.get("mob_id") == expected, (method, params)


@pytest.mark.asyncio
@pytest.mark.parametrize("name", sorted(OWN_MOB_ID_REQUIRED | OWN_MOB_ID_OPTIONAL))
async def test_a_named_handle_sends_a_callers_own_mob_id_as_given(
    monkeypatch: pytest.MonkeyPatch, name: str
) -> None:
    run = await _drive(
        monkeypatch, name, {"mob_id": "mob-c"}, mob_id="mob-b", features=(MOB_ADDRESSING,)
    )

    sent = _addressed(run)
    assert sent
    for method, params in sent:
        assert params.get("mob_id") == "mob-c", (method, params)


@pytest.mark.asyncio
@pytest.mark.parametrize(
    ("name", "given", "expected"),
    [
        # An event query names the mob whose ledger is read.
        ("query_mob_events", {"query": EventQuery(mob_id="mob-c")}, "mob-c"),
        ("query_mob_events", {"query": EventQuery(run_id="run-1")}, "mob-b"),
        ("query_mob_events", {"query": {"mob_id": "mob-c"}}, "mob-c"),
        # A method that passes keyword arguments through passes a mob_id too.
        ("resolve_routing", {"recipient": "x", "mob_id": "mob-c"}, "mob-c"),
    ],
)
async def test_a_named_handle_keeps_a_mob_the_caller_named_another_way(
    monkeypatch: pytest.MonkeyPatch, name: str, given: dict[str, Any], expected: str
) -> None:
    run = await _drive(monkeypatch, name, given, mob_id="mob-b", features=(MOB_ADDRESSING,))

    sent = _addressed(run)
    assert [params.get("mob_id") for _, params in sent] == [expected]


@pytest.mark.asyncio
@pytest.mark.parametrize(
    ("name", "given"),
    [
        # A method's own mob_id, given as none.
        ("announcement_get", {"mob_id": None}),
        # Params the caller wrote with a null mob_id.
        ("resolve_routing", {"recipient": "x", "mob_id": None}),
        ("query_mob_events", {"query": {"mob_id": None}}),
    ],
)
async def test_a_named_handle_names_its_mob_where_the_caller_wrote_none(
    monkeypatch: pytest.MonkeyPatch, name: str, given: dict[str, Any]
) -> None:
    # On the wire a null mob_id addresses the launch's own mob, so a named
    # handle never sends one: where the caller wrote none it names its own.
    run = await _drive(monkeypatch, name, given, mob_id="mob-b", features=(MOB_ADDRESSING,))

    sent = _addressed(run)
    assert [params.get("mob_id", "absent") for _, params in sent] == ["mob-b"]


# Every character a URL reserves, a space and a percent sign, then the four
# that need no escaping.
ID_TO_ESCAPE = "m b/c?d&e=f#g+h!*'()%-_.~"
ID_ESCAPED = "m%20b%2Fc%3Fd%26e%3Df%23g%2Bh%21%2A%27%28%29%25-_.~"


@pytest.mark.asyncio
@pytest.mark.parametrize("name", sorted(STREAM_PATHS))
async def test_a_stream_url_carries_the_mob_id_escaped(
    monkeypatch: pytest.MonkeyPatch, name: str
) -> None:
    run = await _drive(monkeypatch, name, mob_id=ID_TO_ESCAPE, features=(MOB_ADDRESSING,))

    # The same bytes from both SDKs: every reserved character percent-encoded.
    assert run.streams == [HTTP_BASE + STREAM_PATHS[name] + "?mob_id=" + ID_ESCAPED]


@pytest.mark.asyncio
@pytest.mark.parametrize("case", _cases(), ids=_case_id)
async def test_without_the_capability_a_named_handle_refuses_locally(
    monkeypatch: pytest.MonkeyPatch, case: tuple[str, dict[str, Any] | None]
) -> None:
    name, given = case
    # A gateway that serves the hosting calls and does not honour mob_id.
    run = await _drive(monkeypatch, name, given, mob_id="mob-b", features=(HOSTED_MOBS,))

    if name in SENDS_NOTHING:
        assert run.requests() == [] and run.streams == []
        return
    assert isinstance(run.error, CapabilityUnavailableError), repr(run.error)
    assert run.error.data == {"capability": MOB_ADDRESSING}
    # Nothing but the capability read left: such a gateway would ignore the id
    # and answer for its own mob.
    assert run.transport == [CAPABILITY_READ]
    assert run.multipart == [] and run.streams == []


@pytest.mark.asyncio
async def test_a_gateway_that_lists_no_feature_capabilities_is_refused_too(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    # An earlier gateway's answer: no ``feature_capabilities`` member at all.
    run = await _drive(monkeypatch, "list_members", mob_id="mob-b", features=None)

    assert isinstance(run.error, CapabilityUnavailableError), repr(run.error)
    assert run.transport == [CAPABILITY_READ]


@pytest.mark.asyncio
async def test_the_capability_is_read_once_per_started_transport(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    run = _Run()
    runtime, transport = _runtime_over(monkeypatch, run, (MOB_ADDRESSING,))

    one, other = runtime.mob("mob-b"), runtime.mob("mob-c")
    await one.get_blob("blob-1")
    await one.get_blob("blob-2")
    await other.get_blob("blob-3")
    await runtime.mob_handle().get_blob("blob-4")

    assert transport.sent() == [
        CAPABILITY_READ,
        ("meta/blob/get", {"blob_id": "blob-1", "mob_id": "mob-b"}),
        ("meta/blob/get", {"blob_id": "blob-2", "mob_id": "mob-b"}),
        ("meta/blob/get", {"blob_id": "blob-3", "mob_id": "mob-c"}),
        ("meta/blob/get", {"blob_id": "blob-4"}),
    ]

    # A transport started later is another gateway process: it is asked again,
    # and what the first one answered is not carried over.
    later = _Recorder(features=())
    runtime._transport = later
    with pytest.raises(CapabilityUnavailableError):
        await one.get_blob("blob-5")
    assert later.sent() == [CAPABILITY_READ]


@pytest.mark.asyncio
async def test_an_answer_is_kept_for_the_transport_that_was_asked(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    run = _Run()
    runtime, asked = _runtime_over(monkeypatch, run, (MOB_ADDRESSING,))
    # Another gateway process takes the first one's place while its
    # capability read is under way; it does not honour mob_id.
    later = _Recorder(features=())
    answer = asked._answer

    def answer_and_be_replaced(request: dict[str, Any]) -> dict[str, Any]:
        if request.get("method") == "meta/capabilities":
            runtime._transport = later
        return answer(request)

    asked._answer = answer_and_be_replaced  # type: ignore[method-assign]
    handle = runtime.mob("mob-b")

    # Where this first request goes is recorded here, not promised: the swap
    # is made inside the capability answer, which the runtime itself never
    # does (it replaces a transport only after that transport's stop).
    await handle.get_blob("blob-1")
    # What the first process answered is not the second one's answer: the
    # next call asks the transport it is about to use, and is refused.
    with pytest.raises(CapabilityUnavailableError):
        await handle.get_blob("blob-2")

    assert asked.sent() == [CAPABILITY_READ]
    assert later.sent()[1:] == [CAPABILITY_READ]


@pytest.mark.asyncio
async def test_the_default_handle_never_reads_the_capability(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    run = _Run()
    runtime, transport = _runtime_over(monkeypatch, run, features=())

    await runtime.mob_handle().get_blob("blob-1")

    assert transport.sent() == [("meta/blob/get", {"blob_id": "blob-1"})]


@pytest.mark.asyncio
async def test_the_view_names_its_mob_on_the_console_route_and_has_no_sync_send(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    # No call of a handle takes these two paths of the runtime today; the view
    # still answers for them, so a call that takes one later names its mob or
    # fails loudly.
    run = _Run()
    runtime, transport = _runtime_over(monkeypatch, run, (MOB_ADDRESSING,))
    view = runtime.mob("mob-b")._runtime

    await view._console_rpc("meta/console/list_identities", {})
    with pytest.raises(NotImplementedError):
        view._rpc_sync("meta/status")

    assert run.console == [("meta/console/list_identities", {"mob_id": "mob-b"})]
    assert transport.sent() == [CAPABILITY_READ]


def _without_mob_id(
    requests: list[tuple[str, dict[str, Any]]],
) -> list[tuple[str, dict[str, Any]]]:
    return [
        (method, {key: value for key, value in params.items() if key != "mob_id"})
        for method, params in requests
    ]


def _hold_the_sdks_clock(monkeypatch: pytest.MonkeyPatch) -> None:
    """Give the SDK's own reads of the clock one instant.

    A wait sends what is left of its timeout, which it reads from the clock,
    so two calls made one after the other can differ by a millisecond. The
    event loop keeps the real clock.
    """
    real = time.monotonic
    held = real()

    def monotonic() -> float:
        reader = str(sys._getframe(1).f_globals.get("__name__"))
        return held if reader.startswith("meerkat_meta.") else real()

    monkeypatch.setattr(time, "monotonic", monotonic)


@pytest.mark.asyncio
@pytest.mark.parametrize("case", _cases(), ids=_case_id)
async def test_a_named_handle_sends_what_the_default_handle_sends_and_its_id(
    monkeypatch: pytest.MonkeyPatch, case: tuple[str, dict[str, Any] | None]
) -> None:
    name, given = case
    _hold_the_sdks_clock(monkeypatch)
    default = await _drive(monkeypatch, name, given, features=(MOB_ADDRESSING,))
    named = await _drive(monkeypatch, name, given, mob_id="mob-b", features=(MOB_ADDRESSING,))

    if name in SENDS_NOTHING:
        assert named.requests() == default.requests() == []
        return
    # The same requests in the same order, the id aside, after the one read.
    assert named.transport[:1] == [CAPABILITY_READ]
    assert _without_mob_id(named.transport[1:]) == _without_mob_id(default.transport)
    assert _without_mob_id(named.multipart) == _without_mob_id(default.multipart)
    assert named.streams == [url + "?mob_id=mob-b" for url in default.streams]
    # And each request is given the transport timeout the default handle's is.
    assert named.timeouts[1:] == default.timeouts


@pytest.mark.asyncio
async def test_a_wait_on_a_named_handle_is_given_its_transport_timeout(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    # A wait has the gateway hold its request for the wait's own timeout, so
    # the transport is told to wait longer than its default (60 s here). A
    # named handle hands that on as the default handle does: without it the
    # wait would end at the transport's default.
    given = {"target": "startup_ready", "timeout": 300.0}
    longer = 300.0 + runtime_module._IDENTITY_BOOTSTRAP_WAIT_TRANSPORT_HEADROOM_SECONDS
    for mob_id in (None, "mob-b"):
        run = await _drive(
            monkeypatch, "wait_identity_bootstrap", given, mob_id=mob_id, features=(MOB_ADDRESSING,)
        )
        waits = [
            timeout
            for (method, _), timeout in zip(run.transport, run.timeouts)
            if method == "meta/wait_identity_bootstrap"
        ]
        assert waits == [longer], mob_id


@pytest.mark.asyncio
async def test_a_named_handle_reaches_the_wire_through_a_view_that_holds_no_state(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    run = _Run()
    runtime, transport = _runtime_over(monkeypatch, run, (MOB_ADDRESSING,))
    held_before = set(vars(runtime))

    handle = runtime.mob("mob-b")
    view = handle._runtime
    await handle.get_blob("blob-1")
    await handle.upload_blob(b"png")
    assert [event async for event in handle.subscribe_mob()] == []

    # The handle of the launch's own mob talks to the runtime itself; a named
    # one talks to a view of it.
    assert runtime.mob_handle()._runtime is runtime
    assert view is not runtime and isinstance(view, MetaRuntime)
    # Whatever it sent, the view keeps its runtime and its mob's id, nothing
    # else; what was learned of the gateway is the runtime's.
    assert set(vars(view)) == {"_viewed", "_mob_id"}
    assert set(vars(runtime)) - held_before == {"_feature_capabilities_read"}
    # It reads the runtime's state as it is now ...
    runtime._rust_http_base = "http://127.0.0.1:10"
    assert view.rust_http_base_url == "http://127.0.0.1:10"
    # ... and takes no write, by assignment or through a method of the runtime.
    with pytest.raises(AttributeError):
        view._running = False
    with pytest.raises(AttributeError):
        view.set_rust_http_base("http://127.0.0.1:11")
    assert runtime._running is True
    assert runtime.rust_http_base_url == "http://127.0.0.1:10"
    assert set(vars(view)) == {"_viewed", "_mob_id"}


# -- The runtime's calls by identity, for a handle's mob ----------------------


def _public_functions(owner: type) -> list[str]:
    return sorted(
        name
        for name, member in vars(owner).items()
        if not name.startswith("_") and inspect.isfunction(member)
    )


def test_the_handle_offers_each_identity_first_call_of_the_runtime() -> None:
    # Every public call of the runtime is one of four: offered by the handle
    # under its name, offered under the wire's words, offered as agent(), or
    # the runtime's alone. A call added to the runtime fails here until it is
    # placed.
    assert set(_public_functions(MetaRuntime)) == (
        FORWARDED | {runtime_name for runtime_name, _, _ in RENAMED.values()} | {"agent"}
    ) | RUNTIME_ONLY
    # The handle takes each call exactly as the runtime does.
    for name in sorted(FORWARDED | {"agent"}):
        assert inspect.signature(getattr(MobHandle, name)) == inspect.signature(
            getattr(MetaRuntime, name)
        ), name
    for handle_name, (runtime_name, _, _) in RENAMED.items():
        assert inspect.signature(getattr(MobHandle, handle_name)) == inspect.signature(
            getattr(MetaRuntime, runtime_name)
        ), handle_name


@pytest.mark.asyncio
@pytest.mark.parametrize("handle_name", sorted(RENAMED))
async def test_a_name_the_handle_had_keeps_its_call_beside_the_runtimes(
    monkeypatch: pytest.MonkeyPatch, handle_name: str
) -> None:
    runtime_name, runtime_wire, handle_wire = RENAMED[handle_name]

    for mob_id, expected in ((None, None), ("mob-b", "mob-b")):
        own = await _drive(monkeypatch, runtime_name, mob_id=mob_id, features=(MOB_ADDRESSING,))
        runtimes = await _drive(monkeypatch, handle_name, mob_id=mob_id, features=(MOB_ADDRESSING,))
        first = 0 if mob_id is None else 1
        # ``send`` is the mob-member send and ``send_identity`` the send by
        # identity, on the default handle and on a named one.
        assert [method for method, _ in own.transport[first:]] == [handle_wire]
        assert [method for method, _ in runtimes.transport[first:]] == [runtime_wire]
        for _, params in own.transport[first:] + runtimes.transport[first:]:
            assert params.get("mob_id") == expected


@pytest.mark.asyncio
# The recording gateway answers no turn ticket; the *_and_wait calls say so.
@pytest.mark.filterwarnings("ignore::meerkat_meta.errors.TurnTrackingUnavailableWarning")
@pytest.mark.parametrize("name", _public_functions(IdentityAgentHandle))
async def test_an_agent_handle_names_its_handles_mob_on_every_call(
    monkeypatch: pytest.MonkeyPatch, name: str
) -> None:
    def agent(handle: MobHandle) -> IdentityAgentHandle:
        return handle.agent("worker-1")

    given = AGENT_BY_METHOD.get(name, {})
    by_default = await _drive(monkeypatch, name, given, features=(MOB_ADDRESSING,), of=agent)
    named = await _drive(
        monkeypatch, name, given, mob_id="mob-b", features=(MOB_ADDRESSING,), of=agent
    )

    assert by_default.transport, f"IdentityAgentHandle.{name} sent nothing"
    assert all("mob_id" not in params for _, params in by_default.transport)
    sent = _addressed(named)
    assert [method for method, _ in sent] == [method for method, _ in by_default.transport]
    assert all(params.get("mob_id") == "mob-b" for _, params in sent), sent


@pytest.mark.asyncio
@pytest.mark.parametrize("name", _public_functions(IdentitiesHandle))
async def test_the_long_term_agents_of_a_handle_name_its_mob_on_every_call(
    monkeypatch: pytest.MonkeyPatch, name: str
) -> None:
    def identities(handle: MobHandle) -> IdentitiesHandle:
        return handle.identities

    given = IDENTITIES_BY_METHOD.get(name, {})
    by_default = await _drive(monkeypatch, name, given, features=(MOB_ADDRESSING,), of=identities)
    named = await _drive(
        monkeypatch, name, given, mob_id="mob-b", features=(MOB_ADDRESSING,), of=identities
    )

    assert by_default.transport, f"IdentitiesHandle.{name} sent nothing"
    assert all("mob_id" not in params for _, params in by_default.transport)
    sent = _addressed(named)
    assert [method for method, _ in sent] == [method for method, _ in by_default.transport]
    assert all(params.get("mob_id") == "mob-b" for _, params in sent), sent


@pytest.mark.asyncio
async def test_without_the_capability_the_agents_of_a_named_handle_refuse_locally(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    run = _Run()
    runtime, transport = _runtime_over(monkeypatch, run, features=())
    handle = runtime.mob("mob-b")

    with pytest.raises(CapabilityUnavailableError):
        await handle.agent("worker-1").send("hello")
    with pytest.raises(CapabilityUnavailableError):
        await handle.identities.list()

    assert transport.sent() == [CAPABILITY_READ]


@pytest.mark.asyncio
async def test_the_runtimes_own_calls_by_identity_name_no_mob(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    run = _Run()
    runtime, transport = _runtime_over(monkeypatch, run, features=(MOB_ADDRESSING,))

    await runtime.send("worker-1", "hello")
    await runtime.agent("worker-1").send("hello")
    await IdentitiesHandle(runtime).directory()

    # No capability read and no mob: the runtime's calls are the launch's mob's.
    assert transport.sent() == [
        ("meta/send", {"identity": "worker-1", "content": "hello"}),
        ("meta/send", {"identity": "worker-1", "content": "hello"}),
        ("meta/identity/directory", {}),
    ]


# -- The mobs of a host --------------------------------------------------------

MOB_CONFIG = '[mob]\nid = "mob-b"\n'
DEFAULT_ENTRY = {"mob_id": "mob-a", "default": True, "state": "running", "provenance": "host"}
SECOND_ENTRY = {"mob_id": "mob-b", "default": False, "state": "dormant"}
CREATED_ENTRY = {**SECOND_ENTRY, "state": "running", "provenance": "host"}
MOBS_ANSWERS = {
    "meta/mobs/create": {"result": {"mob": CREATED_ENTRY}},
    "meta/mobs/list": {"result": {"mobs": [DEFAULT_ENTRY, SECOND_ENTRY]}},
    "meta/mobs/get": {"result": {"mob": SECOND_ENTRY}},
}


@pytest.mark.asyncio
async def test_mobs_create_list_and_get_ask_the_gateway_each_time(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    run = _Run()
    runtime, transport = _runtime_over(monkeypatch, run, (HOSTED_MOBS,), MOBS_ANSWERS)

    created = await runtime.mobs.create(MOB_CONFIG)
    listed = await runtime.mobs.list()
    await runtime.mobs.list()
    one = await runtime.mobs.get("mob-b")
    await runtime.mobs.get("mob-b")

    # One capability read, then every call is a request: nothing is remembered.
    assert transport.sent() == [
        CAPABILITY_READ,
        ("meta/mobs/create", {"mob_config": MOB_CONFIG}),
        ("meta/mobs/list", {}),
        ("meta/mobs/list", {}),
        ("meta/mobs/get", {"mob_id": "mob-b"}),
        ("meta/mobs/get", {"mob_id": "mob-b"}),
    ]
    assert type(created) is meerkat_meta.HostedMob
    assert (created.mob_id, created.default, created.state, created.provenance) == (
        "mob-b",
        False,
        "running",
        "host",
    )
    assert [(mob.mob_id, mob.default, mob.state) for mob in listed] == [
        ("mob-a", True, "running"),
        ("mob-b", False, "dormant"),
    ]
    # A mob whose creation record was not read has no provenance.
    assert listed[1].provenance is None
    assert one == listed[1]


@pytest.mark.asyncio
async def test_create_and_get_take_text_or_send_nothing(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    run = _Run()
    runtime, transport = _runtime_over(monkeypatch, run, (HOSTED_MOBS,), MOBS_ANSWERS)
    mobs: Any = runtime.mobs

    for not_text in (None, 7, b"mob-b", {"id": "mob-b"}):
        with pytest.raises(TypeError):
            await mobs.create(not_text)
        with pytest.raises(TypeError):
            await mobs.get(not_text)

    assert transport.sent() == []


def test_a_hosted_mob_is_the_default_only_when_the_gateway_says_true() -> None:
    entry = {"mob_id": "mob-b", "state": "running"}
    read = meerkat_meta.HostedMob.from_dict

    assert read({**entry, "default": True}).default is True
    # Anything else is not the launch's own mob, whatever its truth value.
    for other in (False, None, 1, "true", "yes", [True], {"default": True}):
        assert read({**entry, "default": other}).default is False, other
    assert read(entry).default is False


@pytest.mark.asyncio
async def test_an_entry_carries_the_revision_of_a_loaded_mob(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    # The revision of a mob's definition: what a redefine is made against.
    # ``get`` of a loaded mob and the entry of a create, lifecycle or redefine
    # answer carry it; a listed entry and a mob that is not loaded do not.
    loaded = {**CREATED_ENTRY, "revision": 4}
    answers = {
        "meta/mobs/get": {"result": {"mob": loaded}},
        "meta/mobs/list": {"result": {"mobs": [DEFAULT_ENTRY, SECOND_ENTRY]}},
        "meta/mobs/redefine": {
            "result": {"mob": {**loaded, "revision": 5}, "receipt": RECEIPT}
        },
    }
    run = _Run()
    runtime, transport = _runtime_over(monkeypatch, run, (HOSTED_MOBS,), answers)

    one = await runtime.mobs.get("mob-b")
    listed = await runtime.mobs.list()
    redefined = await runtime.mobs.redefine("mob-b", MOB_CONFIG, expected_revision=one.revision)

    assert one.revision == 4
    assert [mob.revision for mob in listed] == [None, None]
    assert redefined.mob.revision == 5
    assert transport.sent()[-1] == (
        "meta/mobs/redefine",
        {"mob_id": "mob-b", "mob_config": MOB_CONFIG, "expected_revision": 4},
    )
    # Only a whole number is a revision; anything else reads as none.
    read = meerkat_meta.HostedMob.from_dict
    for other in (None, "4", 4.0, True, [4]):
        assert read({**SECOND_ENTRY, "revision": other}).revision is None, other
    assert read({**SECOND_ENTRY, "revision": 0}).revision == 0


@pytest.mark.asyncio
@pytest.mark.parametrize("call", ["create", "list", "get"])
async def test_without_the_capability_the_hosting_calls_refuse_locally(
    monkeypatch: pytest.MonkeyPatch, call: str
) -> None:
    run = _Run()
    # A gateway that honours mob_id for its own mob and hosts no other.
    runtime, transport = _runtime_over(monkeypatch, run, (MOB_ADDRESSING,), MOBS_ANSWERS)
    arguments = {"create": (MOB_CONFIG,), "list": (), "get": ("mob-b",)}[call]

    with pytest.raises(CapabilityUnavailableError) as refused:
        await getattr(runtime.mobs, call)(*arguments)

    assert refused.value.data == {"capability": HOSTED_MOBS}
    assert refused.value.method == f"meta/mobs/{call}"
    assert transport.sent() == [CAPABILITY_READ]


@pytest.mark.asyncio
async def test_a_listed_mobs_unknown_state_is_kept_as_text(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    answers = {
        "meta/mobs/list": {
            "result": {
                "mobs": [
                    DEFAULT_ENTRY,
                    {"mob_id": "mob-b", "default": False, "state": "hibernating", "later": 1},
                ]
            }
        }
    }
    run = _Run()
    runtime, _ = _runtime_over(monkeypatch, run, (HOSTED_MOBS,), answers)

    listed = await runtime.mobs.list()

    assert listed[0].known_state is True
    assert (listed[1].state, listed[1].known_state) == ("hibernating", False)


def test_a_hosted_mob_reads_every_state_and_its_optional_members() -> None:
    hosted_mob = meerkat_meta.HostedMob
    assert types_module.HOSTED_MOB_STATES == frozenset(
        {
            "creating",
            "running",
            "stopped",
            "completed",
            "destroyed",
            "dormant",
            "quarantined",
            "held",
        }
    )
    for state in sorted(types_module.HOSTED_MOB_STATES):
        entry = hosted_mob.from_dict({"mob_id": "mob-b", "default": False, "state": state})
        assert (entry.state, entry.known_state) == (state, True)
        assert (entry.provenance, entry.failure, entry.quarantine) == (None, None, None)

    quarantine = {"class": "store_unreadable", "retryable": True, "attempts": 2}
    quarantined = hosted_mob.from_dict(
        {"mob_id": "mob-b", "default": False, "state": "quarantined", "quarantine": quarantine}
    )
    assert quarantined.quarantine == quarantine
    stopped = hosted_mob.from_dict(
        {
            "mob_id": "mob-b",
            "default": False,
            "state": "running",
            "provenance": "operator_session",
            "failure": {"message": "actor stopped"},
        }
    )
    # A creation kind this SDK does not list is kept as text too.
    assert stopped.provenance == "operator_session"
    assert stopped.failure == {"message": "actor stopped"}
    # An entry without its id or its state is not an entry: an answer this
    # SDK does not read, not a mistake of the caller.
    for not_an_entry in (
        {"default": False, "state": "running"},
        {"mob_id": "mob-b", "default": False},
        {"mob_id": 7, "default": False, "state": "running"},
        None,
        ["mob-b"],
    ):
        with pytest.raises(ContractMismatchError):
            hosted_mob.from_dict(not_an_entry)


def test_the_capability_predicates_read_a_result_or_a_list() -> None:
    result = types_module.CapabilitiesResult.from_dict(
        {
            "contract_version": "0.9.15",
            "methods": [],
            "loaded_modules": [],
            "feature_capabilities": [MOB_ADDRESSING],
        }
    )
    assert (types_module.MOB_ADDRESSING, types_module.HOSTED_MOBS) == (MOB_ADDRESSING, HOSTED_MOBS)
    assert types_module.supports_mob_addressing(result) is True
    assert types_module.supports_hosted_mobs(result) is False
    assert types_module.supports_hosted_mobs([HOSTED_MOBS]) is True
    assert types_module.supports_mob_addressing([]) is False


# -- The refusal of the hosting calls ------------------------------------------


def _hosting_error(data: Any, code: int = -32059) -> RpcError:
    payload: dict[str, Any] = {"code": code, "message": "refused"}
    if data is not None:
        payload["data"] = data
    return _rpc_error_from_payload(
        payload, request_id="meta/mobs/create:1", method="meta/mobs/create"
    )


def test_the_hosting_refusal_is_one_error_with_its_kind() -> None:
    assert errors_module.MOB_HOSTING_REFUSED_CODE == -32059
    assert errors_module.MOB_HOSTING_REFUSAL_KINDS == frozenset(
        {"mob_exists", "invalid_mob_id", "default_mob", "mob_not_stopped", "mob_busy"}
    )
    # The one kind whose request can succeed unchanged later.
    busy = _hosting_error(
        {
            "kind": "mob_busy",
            "mob_id": "mob-b",
            "retryable": True,
            "detail": "'mob-b' is stopped and does not close for a new definition; held: owner",
        }
    )
    assert type(busy) is errors_module.MobHostingRefusedError
    assert (busy.kind, busy.known_kind, busy.retryable) == ("mob_busy", True, True)
    assert busy.detail.endswith("held: owner")
    for kind in sorted(errors_module.MOB_HOSTING_REFUSAL_KINDS):
        error = _hosting_error(
            {"kind": kind, "mob_id": "mob-b", "retryable": False, "detail": "why it was refused"}
        )
        assert type(error) is errors_module.MobHostingRefusedError
        assert isinstance(error, RpcError)
        assert (error.code, error.method) == (-32059, "meta/mobs/create")
        assert (error.kind, error.known_kind) == (kind, True)
        assert (error.mob_id, error.retryable, error.detail) == (
            "mob-b",
            False,
            "why it was refused",
        )
    for name in (
        "MOB_HOSTING_REFUSED_CODE",
        "MOB_HOSTING_REFUSAL_KINDS",
        "MobHostingRefusal",
        "MobHostingRefusedError",
        "HostedMob",
        "HOSTED_MOB_STATES",
        "MobsHandle",
        "MOB_ADDRESSING",
        "HOSTED_MOBS",
        "supports_mob_addressing",
        "supports_hosted_mobs",
    ):
        assert name in meerkat_meta.__all__, name
        assert getattr(meerkat_meta, name) is not None


def test_a_hosting_refusal_kind_this_sdk_does_not_list_is_still_the_refusal() -> None:
    error = _hosting_error({"kind": "mob_frozen", "mob_id": "mob-b", "since_ms": 5})

    # Never a success and never another error: the refusal, with its token.
    assert type(error) is errors_module.MobHostingRefusedError
    assert (error.kind, error.known_kind) == ("mob_frozen", False)
    assert error.retryable is None
    # The gateway sent no sentence: the error's message stands in.
    assert error.detail == "refused"
    assert error.details == {"since_ms": 5}


@pytest.mark.parametrize("data", [None, "text", {}, {"kind": 3}, {"mob_id": "mob-b"}])
def test_the_hosting_code_without_a_kind_is_a_plain_rpc_error(data: Any) -> None:
    error = _hosting_error(data)

    assert type(error) is RpcError
    assert error.code == -32059


def test_a_mob_the_host_does_not_have_stays_a_plain_rpc_error() -> None:
    data = {"kind": "foreign_mob_target", "requested_mob_id": "mob-z", "gateway_mob_id": "mob-a"}
    error = _hosting_error(data, code=-32602)

    assert type(error) is RpcError
    assert error.data == data


@pytest.mark.asyncio
async def test_a_refused_create_raises_the_hosting_refusal(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    refusal = {
        "code": -32059,
        "message": "a mob with the id 'mob-b' exists on this host",
        "data": {"kind": "mob_exists", "mob_id": "mob-b", "retryable": False},
    }
    run = _Run()
    runtime, _ = _runtime_over(
        monkeypatch, run, (HOSTED_MOBS,), {"meta/mobs/create": {"error": refusal}}
    )

    with pytest.raises(errors_module.MobHostingRefusedError) as refused:
        await runtime.mobs.create(MOB_CONFIG)

    assert (refused.value.kind, refused.value.mob_id) == ("mob_exists", "mob-b")


# -- Stop, resume, redefine and destroy a mob of the host ------------------------
#
# The requests are held to the wire's own text: ``meta/mobs/lifecycle`` takes
# ``{"mob_id", "action"}``, ``meta/mobs/redefine`` takes ``{"mob_id",
# "mob_config", "expected_revision"}`` and ``meta/mobs/destroy`` takes
# ``{"mob_id"}``, every member required.

STOPPED_ENTRY = {"mob_id": "mob-b", "default": False, "state": "stopped", "provenance": "host"}
STOP_REPORT = {"members_stopped": 2, "inflight": []}
RECEIPT = {
    "mob_id": "mob-b",
    "previous_revision": 4,
    "committed_revision": 5,
    "projection_revision": 9,
    "event_cursor": 120,
    "declared_fields": ["profiles.worker.model"],
}
DESTROY_REPORT = {"storage_removed": True}
CHANGE_ANSWERS = {
    "meta/mobs/lifecycle": {"result": {"mob": STOPPED_ENTRY, "stop_report": STOP_REPORT}},
    "meta/mobs/redefine": {"result": {"mob": CREATED_ENTRY, "receipt": RECEIPT}},
    "meta/mobs/destroy": {"result": {"mob_id": "mob-b", "destroy_report": DESTROY_REPORT}},
}


def _params_as_sent(transport: _Recorder) -> list[tuple[str, str]]:
    """Each request's method and its params as JSON text, members in the order sent."""
    return [
        (str(request["method"]), json.dumps(request.get("params") or {}))
        for request in transport.requests
    ]


@pytest.mark.asyncio
async def test_mobs_lifecycle_redefine_and_destroy_send_the_wires_requests(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    run = _Run()
    runtime, transport = _runtime_over(monkeypatch, run, (HOSTED_MOBS,), CHANGE_ANSWERS)

    stopped = await runtime.mobs.lifecycle("mob-b", "stop")
    await runtime.mobs.lifecycle("mob-b", "resume")
    redefined = await runtime.mobs.redefine("mob-b", MOB_CONFIG, expected_revision=4)
    destroyed = await runtime.mobs.destroy("mob-b")

    assert _params_as_sent(transport) == [
        ("meta/capabilities", "{}"),
        ("meta/mobs/lifecycle", '{"mob_id": "mob-b", "action": "stop"}'),
        ("meta/mobs/lifecycle", '{"mob_id": "mob-b", "action": "resume"}'),
        (
            "meta/mobs/redefine",
            json.dumps({"mob_id": "mob-b", "mob_config": MOB_CONFIG, "expected_revision": 4}),
        ),
        ("meta/mobs/destroy", '{"mob_id": "mob-b"}'),
    ]
    # Each answer is typed; what Core reports is handed on as it came.
    assert type(stopped) is types_module.MobLifecycleResult
    assert (stopped.mob.mob_id, stopped.mob.state) == ("mob-b", "stopped")
    assert stopped.stop_report == STOP_REPORT
    assert type(redefined) is types_module.MobRedefineResult
    assert redefined.mob == types_module.HostedMob.from_dict(CREATED_ENTRY)
    assert redefined.receipt == types_module.MobRedefineReceipt(
        mob_id="mob-b",
        previous_revision=4,
        committed_revision=5,
        projection_revision=9,
        event_cursor=120,
        declared_fields=("profiles.worker.model",),
    )
    assert type(destroyed) is types_module.MobDestroyResult
    assert (destroyed.mob_id, destroyed.destroy_report) == ("mob-b", DESTROY_REPORT)
    for name in (
        "MobLifecycleResult",
        "MobRedefineResult",
        "MobRedefineReceipt",
        "MobDestroyResult",
    ):
        assert name in meerkat_meta.__all__, name


@pytest.mark.asyncio
async def test_a_resume_answers_no_stop_report(monkeypatch: pytest.MonkeyPatch) -> None:
    answers = {"meta/mobs/lifecycle": {"result": {"mob": CREATED_ENTRY}}}
    run = _Run()
    runtime, _ = _runtime_over(monkeypatch, run, (HOSTED_MOBS,), answers)

    resumed = await runtime.mobs.lifecycle("mob-b", "resume")

    assert (resumed.mob.state, resumed.stop_report) == ("running", None)


@pytest.mark.asyncio
@pytest.mark.parametrize("call", ["lifecycle", "redefine", "destroy"])
async def test_without_the_capability_the_changes_of_a_hosted_mob_refuse_locally(
    monkeypatch: pytest.MonkeyPatch, call: str
) -> None:
    run = _Run()
    runtime, transport = _runtime_over(monkeypatch, run, (MOB_ADDRESSING,), CHANGE_ANSWERS)
    arguments = {
        "lifecycle": ("mob-b", "stop"),
        "redefine": ("mob-b", MOB_CONFIG, 4),
        "destroy": ("mob-b",),
    }[call]

    with pytest.raises(CapabilityUnavailableError) as refused:
        await getattr(runtime.mobs, call)(*arguments)

    assert refused.value.data == {"capability": HOSTED_MOBS}
    assert refused.value.method == f"meta/mobs/{call}"
    assert transport.sent() == [CAPABILITY_READ]


@pytest.mark.asyncio
async def test_a_change_of_a_hosted_mob_names_its_mob_and_its_input_or_sends_nothing(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    run = _Run()
    runtime, transport = _runtime_over(monkeypatch, run, (HOSTED_MOBS,), CHANGE_ANSWERS)
    mobs: Any = runtime.mobs

    # The wire requires every member: a call that lacks one is refused here.
    with pytest.raises(ValueError):
        await mobs.lifecycle("mob-b", "pause")
    for not_an_id in (None, 7):
        with pytest.raises(TypeError):
            await mobs.lifecycle(not_an_id, "stop")
        with pytest.raises(TypeError):
            await mobs.redefine(not_an_id, MOB_CONFIG, expected_revision=4)
        with pytest.raises(TypeError):
            await mobs.destroy(not_an_id)
    with pytest.raises(TypeError):
        await mobs.redefine("mob-b", None, expected_revision=4)
    for not_a_revision in (None, "4", 4.0, True):
        with pytest.raises(TypeError):
            await mobs.redefine("mob-b", MOB_CONFIG, expected_revision=not_a_revision)
    # A revision is a count: a negative one is refused here, as in TypeScript.
    with pytest.raises(ValueError):
        await mobs.redefine("mob-b", MOB_CONFIG, expected_revision=-1)

    assert transport.sent() == []


@pytest.mark.asyncio
async def test_a_destroy_of_the_launchs_own_mob_raises_the_hosting_refusal(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    refusal = {
        "code": -32059,
        "message": "the mob 'mob-a' is the mob this launch names",
        "data": {"kind": "default_mob", "mob_id": "mob-a", "retryable": False},
    }
    run = _Run()
    runtime, _ = _runtime_over(
        monkeypatch, run, (HOSTED_MOBS,), {"meta/mobs/destroy": {"error": refusal}}
    )

    with pytest.raises(errors_module.MobHostingRefusedError) as refused:
        await runtime.mobs.destroy("mob-a")

    assert (refused.value.kind, refused.value.known_kind, refused.value.mob_id) == (
        "default_mob",
        True,
        "mob-a",
    )


@pytest.mark.asyncio
@pytest.mark.parametrize(
    ("call", "arguments", "error"),
    [
        # A mob the host does not have, on every call that names one.
        (
            "lifecycle",
            ("mob-z", "stop"),
            {
                "code": -32602,
                "message": "request targets mob 'mob-z' but this gateway serves 'mob-a'",
                "data": {
                    "kind": "foreign_mob_target",
                    "requested_mob_id": "mob-z",
                    "gateway_mob_id": "mob-a",
                },
            },
        ),
        # A mob whose store did not load keeps the answer it has elsewhere.
        (
            "destroy",
            ("mob-b",),
            {
                "code": -32603,
                "message": "mob 'mob-b' is quarantined",
                "data": {"kind": "mob_quarantined", "mob_id": "mob-b", "retryable": True},
            },
        ),
        # A read whose load of the mob failed answers what a change of that
        # mob would answer.
        (
            "get",
            ("mob-b",),
            {
                "code": -32603,
                "message": "mob 'mob-b' is quarantined",
                "data": {"kind": "mob_quarantined", "mob_id": "mob-b", "retryable": True},
            },
        ),
    ],
)
async def test_answers_that_are_not_the_hosting_refusal_stay_plain_rpc_errors(
    monkeypatch: pytest.MonkeyPatch, call: str, arguments: tuple, error: dict[str, Any]
) -> None:
    run = _Run()
    runtime, _ = _runtime_over(
        monkeypatch, run, (HOSTED_MOBS,), {f"meta/mobs/{call}": {"error": error}}
    )

    with pytest.raises(RpcError) as raised:
        await getattr(runtime.mobs, call)(*arguments)

    assert type(raised.value) is RpcError
    assert (raised.value.code, raised.value.data) == (error["code"], error.get("data"))


@pytest.mark.asyncio
async def test_a_storage_refusal_without_data_keeps_its_error_class(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    # Pin the SDK's existing code translation, not when a gateway sends it.
    error = {"code": -32014, "message": "the storage cannot serve this call"}
    runtime, transport = _runtime_over(
        monkeypatch, _Run(), (HOSTED_MOBS,), {"meta/mobs/get": {"error": error}}
    )

    with pytest.raises(StorageResolutionError) as raised:
        await runtime.mobs.get("mob-b")

    assert type(raised.value) is StorageResolutionError
    assert raised.value.code == error["code"]
    assert str(raised.value) == error["message"]
    assert raised.value.data is None
    assert transport.sent() == [
        ("meta/capabilities", {}),
        ("meta/mobs/get", {"mob_id": "mob-b"}),
    ]


def test_an_answer_without_its_parts_is_not_read_as_a_result() -> None:
    lifecycle, redefine, destroy = (
        types_module.MobLifecycleResult,
        types_module.MobRedefineResult,
        types_module.MobDestroyResult,
    )
    for malformed in (None, {}, {"mob": {"default": False}}):
        with pytest.raises(ContractMismatchError):
            lifecycle.from_dict(malformed)
    without_receipt = {"mob": CREATED_ENTRY}
    without_a_revision = {key: value for key, value in RECEIPT.items() if key != "event_cursor"}
    for receipt in (
        None,
        {"mob_id": "mob-b"},
        # Every member of the receipt is required, each of its kind.
        without_a_revision,
        {**RECEIPT, "committed_revision": "5"},
        {**RECEIPT, "previous_revision": True},
        {**RECEIPT, "declared_fields": "profiles.worker.model"},
        {**RECEIPT, "mob_id": None},
    ):
        malformed = without_receipt if receipt is None else {**without_receipt, "receipt": receipt}
        with pytest.raises(ContractMismatchError):
            redefine.from_dict(malformed)
    for malformed in (None, {}, {"mob_id": 7}):
        with pytest.raises(ContractMismatchError):
            destroy.from_dict(malformed)
    # A report Core did not send is none, not an error.
    assert destroy.from_dict({"mob_id": "mob-b"}).destroy_report is None
    assert destroy.from_dict({"mob_id": "mob-b", "destroy_report": None}).destroy_report is None


@pytest.mark.asyncio
@pytest.mark.parametrize(
    ("method", "answer", "call"),
    [
        ("meta/mobs/create", {}, lambda mobs: mobs.create(MOB_CONFIG)),
        ("meta/mobs/create", "created", lambda mobs: mobs.create(MOB_CONFIG)),
        ("meta/mobs/list", {}, lambda mobs: mobs.list()),
        ("meta/mobs/list", {"mobs": "mob-a"}, lambda mobs: mobs.list()),
        ("meta/mobs/list", {"mobs": [{"mob_id": "mob-a"}]}, lambda mobs: mobs.list()),
        ("meta/mobs/get", {"mob": None}, lambda mobs: mobs.get("mob-b")),
        ("meta/mobs/lifecycle", {}, lambda mobs: mobs.lifecycle("mob-b", "resume")),
        (
            "meta/mobs/redefine",
            {"mob": CREATED_ENTRY},
            lambda mobs: mobs.redefine("mob-b", MOB_CONFIG, expected_revision=4),
        ),
        ("meta/mobs/destroy", {}, lambda mobs: mobs.destroy("mob-b")),
        ("meta/mobs/destroy", {"destroy_report": DESTROY_REPORT}, lambda mobs: mobs.destroy("mob-b")),
    ],
)
async def test_an_answer_the_sdk_cannot_read_is_not_the_callers_mistake(
    monkeypatch: pytest.MonkeyPatch, method: str, answer: Any, call: Any
) -> None:
    # TypeError and ValueError are for the caller's arguments, refused before
    # anything is sent. Here the request left and the gateway acted on it:
    # the caller must be able to tell the two apart, most of all for a
    # destroy, so an unreadable answer has a class of its own.
    run = _Run()
    runtime, transport = _runtime_over(
        monkeypatch, run, (HOSTED_MOBS,), {method: {"result": answer}}
    )

    with pytest.raises(ContractMismatchError) as raised:
        await call(runtime.mobs)

    assert not isinstance(raised.value, (TypeError, ValueError))
    assert [sent_method for sent_method, _ in transport.sent()] == ["meta/capabilities", method]


@pytest.mark.asyncio
async def test_a_stop_without_its_report_is_not_read_as_a_stop(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    # The wire answers a stop with the mob's stop report. An answer without
    # one is no answer to a stop: it is refused, typed, not read as "none".
    for answer in ({"mob": STOPPED_ENTRY}, {"mob": STOPPED_ENTRY, "stop_report": None}):
        run = _Run()
        runtime, _ = _runtime_over(
            monkeypatch, run, (HOSTED_MOBS,), {"meta/mobs/lifecycle": {"result": answer}}
        )
        with pytest.raises(ContractMismatchError):
            await runtime.mobs.lifecycle("mob-b", "stop")
        # A resume has no report to bring.
        assert (await runtime.mobs.lifecycle("mob-b", "resume")).stop_report is None


@pytest.mark.parametrize("report", ["stopped", 7, True, ["members_stopped"]])
def test_a_report_that_is_not_an_object_is_not_read_as_none(report: Any) -> None:
    with pytest.raises(ContractMismatchError):
        types_module.MobLifecycleResult.from_dict({"mob": STOPPED_ENTRY, "stop_report": report})
    with pytest.raises(ContractMismatchError):
        types_module.MobDestroyResult.from_dict({"mob_id": "mob-b", "destroy_report": report})


# -- An agent's memory and the long-term agents, mob by mob ---------------------
#
# The handle of a named mob is the one way to name a mob on these calls: none
# of them takes a ``mob_id`` of its own. The same agent name under two handles
# is two agents, each with its own memory.

# The seven memory calls: what each is given here, its wire method, and the
# params that leave besides the mob.
MEMORY_CALLS: dict[str, tuple[dict[str, Any], str, dict[str, Any]]] = {
    "remember_agent_memory": (
        {"identity": "scribe", "title": "t", "body": "b", "tags": ["x"]},
        "meta/agent_memory/remember",
        {"identity": "scribe", "title": "t", "body": "b", "tags": ["x"]},
    ),
    "recall_agent_memory": (
        {"identity": "scribe", "selection": "always"},
        "meta/agent_memory/recall",
        {"identity": "scribe", "selection": "always"},
    ),
    "update_agent_memory": (
        {"identity": "scribe", "memory_id": "m-1", "title": "t2", "body": "b2"},
        "meta/agent_memory/update",
        {"identity": "scribe", "memory_id": "m-1", "title": "t2", "body": "b2"},
    ),
    "forget_agent_memory": (
        {"identity": "scribe", "memory_id": "m-1"},
        "meta/agent_memory/forget",
        {"identity": "scribe", "memory_id": "m-1"},
    ),
    "forget_agent_memory_in_scope": (
        {"scope_kind": "mob", "memory_id": "m-1", "scope_key": "shared"},
        "meta/agent_memory/forget_in_scope",
        {"scope": {"kind": "mob", "key": "shared"}, "memory_id": "m-1"},
    ),
    "purge_agent_memory": (
        {"scope_kind": "identity", "memory_id": "m-1", "scope_key": "scribe"},
        "meta/agent_memory/purge",
        {"identity": "scribe", "memory_id": "m-1"},
    ),
    "manifest_agent_memory": (
        {"identity": "scribe", "tier": "full"},
        "meta/agent_memory/manifest",
        {"identity": "scribe", "tier": "full"},
    ),
}
MEMORY_RECORD = {
    "memory_id": "m-1",
    "title": "t",
    "body": "b",
    "tags": [],
    "created_at_ms": 1,
    "updated_at_ms": 1,
}
# What a host answers for a mob that has no long-term agents of its own yet.
NO_IDENTITY_CONTEXT = {
    "kind": "foreign_mob_target",
    "requested_mob_id": "mob-b",
    "gateway_mob_id": "mob-a",
    "hosted": True,
    "identity_context": False,
}


def test_the_memory_calls_are_the_handles_seven_and_take_no_mob_of_their_own() -> None:
    on_the_handle = {name for name in _public_names() if "_agent_memory" in name}

    assert on_the_handle == set(MEMORY_CALLS)
    for name in MEMORY_CALLS:
        assert "mob_id" not in inspect.signature(getattr(MobHandle, name)).parameters, name


@pytest.mark.asyncio
@pytest.mark.parametrize("name", sorted(MEMORY_CALLS))
async def test_a_memory_call_of_a_named_handle_names_its_mob_and_no_realm(
    monkeypatch: pytest.MonkeyPatch, name: str
) -> None:
    given, method, params = MEMORY_CALLS[name]
    by_default = await _drive(monkeypatch, name, given, features=(MOB_ADDRESSING,))
    named = await _drive(monkeypatch, name, given, mob_id="mob-b", features=(MOB_ADDRESSING,))

    # The launch's own mob: the call's params and nothing else, no capability read.
    assert by_default.transport == [(method, params)]
    # A named mob: the same params and the mob, after the one capability read.
    # No realm leaves unless the caller gave one: the gateway fills the mob's own.
    assert _addressed(named) == [(method, {**params, "mob_id": "mob-b"})]


@pytest.mark.asyncio
@pytest.mark.parametrize("name", sorted(MEMORY_CALLS))
@pytest.mark.parametrize("features", [None, (), (HOSTED_MOBS,)], ids=["none", "empty", "other"])
async def test_without_the_capability_a_memory_call_of_a_named_handle_sends_nothing(
    monkeypatch: pytest.MonkeyPatch, name: str, features: tuple[str, ...] | None
) -> None:
    # A gateway that does not honour ``mob_id`` would act for the agent of
    # that name in its own mob: the call is refused here and never leaves.
    run = await _drive(monkeypatch, name, MEMORY_CALLS[name][0], mob_id="mob-b", features=features)

    assert isinstance(run.error, CapabilityUnavailableError), repr(run.error)
    assert run.error.data == {"capability": MOB_ADDRESSING}
    assert run.transport == [CAPABILITY_READ]
    assert run.multipart == [] and run.console == [] and run.streams == []


@pytest.mark.asyncio
async def test_one_agent_name_under_two_mobs_is_two_memories(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    run = _Run()
    runtime, transport = _runtime_over(
        monkeypatch,
        run,
        (MOB_ADDRESSING,),
        {
            "meta/agent_memory/remember": {"result": MEMORY_RECORD},
            "meta/agent_memory/recall": {"result": {"records": [MEMORY_RECORD]}},
            "meta/agent_memory/forget": {"result": {"memory_id": "m-1", "deleted": True}},
        },
    )
    first, second = runtime.mob("mob-a"), runtime.mob("mob-b")

    kept = await first.remember_agent_memory("scribe", title="t", body="b")
    await second.remember_agent_memory("scribe", title="t", body="b")
    recalled = await second.recall_agent_memory("scribe", selection="always")
    await first.forget_agent_memory("scribe", kept.memory_id)

    assert [record.memory_id for record in recalled] == ["m-1"]
    # One capability read for both handles, then each request under its handle's
    # mob: the name is the same text, the mob is what tells the two agents apart.
    remember = {"identity": "scribe", "title": "t", "body": "b"}
    assert transport.sent() == [
        CAPABILITY_READ,
        ("meta/agent_memory/remember", {**remember, "mob_id": "mob-a"}),
        ("meta/agent_memory/remember", {**remember, "mob_id": "mob-b"}),
        ("meta/agent_memory/recall", {"identity": "scribe", "selection": "always", "mob_id": "mob-b"}),
        ("meta/agent_memory/forget", {"identity": "scribe", "memory_id": "m-1", "mob_id": "mob-a"}),
    ]


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "error",
    [
        {"code": -32602, "message": "Invalid params", "data": NO_IDENTITY_CONTEXT},
        {"code": -32601, "message": "agent memory is not configured"},
        {"code": -32602, "message": "agent memory is not configured"},
    ],
    ids=["no-identity-context", "not-configured", "not-configured-params"],
)
async def test_a_mob_that_keeps_no_memory_answers_a_plain_rpc_error(
    monkeypatch: pytest.MonkeyPatch, error: dict[str, Any]
) -> None:
    run = _Run()
    runtime, _ = _runtime_over(
        monkeypatch, run, (MOB_ADDRESSING,), {"meta/agent_memory/recall": {"error": error}}
    )

    with pytest.raises(RpcError) as raised:
        await runtime.mob("mob-b").recall_agent_memory("scribe")

    # No new error and no new kind: the answer as the gateway gave it.
    assert type(raised.value) is RpcError
    assert (raised.value.code, raised.value.data) == (error["code"], error.get("data"))


@pytest.mark.asyncio
async def test_one_agent_name_in_two_mobs_is_two_agents(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    agent = {
        "name": "scribe",
        "profile": "writer",
        "spec_digest": "sha256:" + "a" * 64,
        "archived": False,
        "readiness": "dormant",
    }
    run = _Run()
    runtime, transport = _runtime_over(
        monkeypatch,
        run,
        (MOB_ADDRESSING,),
        {
            "meta/identity/create": {
                "result": {
                    "outcome": "done",
                    "operation_id": "op-1",
                    "agents": [{"name": "scribe", "state": "committed"}],
                }
            },
            "meta/identity/list": {"result": {"agents": [agent]}},
        },
    )
    spec = meerkat_meta.AgentSpec(name="scribe", profile="writer")

    for handle in (runtime.mob_handle(), runtime.mob("mob-b")):
        created = await handle.identities.create([spec], request_id="create-scribe")
        assert [row.name for row in created.agents] == ["scribe"]
        assert [row.name for row in (await handle.identities.list()).agents] == ["scribe"]
        await handle.send_identity("scribe", "hello")

    # The same calls with the same request id, once for the launch's own mob
    # and once for the named one: the mob is the only difference, and the
    # caller's request id leaves as written.
    create = {"request_id": "create-scribe", "agents": [{"name": "scribe", "profile": "writer"}]}
    send = {"identity": "scribe", "content": "hello"}
    assert transport.sent() == [
        ("meta/identity/create", create),
        ("meta/identity/list", {"include_archived": False}),
        ("meta/send", send),
        CAPABILITY_READ,
        ("meta/identity/create", {**create, "mob_id": "mob-b"}),
        ("meta/identity/list", {"include_archived": False, "mob_id": "mob-b"}),
        ("meta/send", {**send, "mob_id": "mob-b"}),
    ]


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "data",
    [
        # The host has the mob and does not serve the long-term agents' calls for it.
        {key: value for key, value in NO_IDENTITY_CONTEXT.items() if key != "identity_context"},
        # The host has the mob and the mob has no identity context.
        NO_IDENTITY_CONTEXT,
    ],
    ids=["not-served", "no-identity-context"],
)
async def test_a_mob_without_long_term_agents_of_its_own_answers_a_plain_rpc_error(
    monkeypatch: pytest.MonkeyPatch, data: dict[str, Any]
) -> None:
    error = {"code": -32602, "message": "Invalid params", "data": data}
    run = _Run()
    runtime, transport = _runtime_over(
        monkeypatch, run, (MOB_ADDRESSING,), {"meta/identity/list": {"error": error}}
    )

    with pytest.raises(RpcError) as raised:
        await runtime.mob("mob-b").identities.list()

    # Not a refusal of the family and not one of the hosting calls: the data
    # says the host has the mob, and is kept whole for the caller.
    assert type(raised.value) is RpcError
    assert (raised.value.code, raised.value.data) == (-32602, data)
    # The request left for the named mob: the answer is the gateway's.
    assert transport.sent()[1] == ("meta/identity/list", {"include_archived": False, "mob_id": "mob-b"})


@pytest.mark.asyncio
async def test_a_refusal_of_the_family_for_a_named_mob_is_the_familys_error(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    error = {"code": -32058, "message": "refused: name_taken", "data": {"kind": "name_taken"}}
    run = _Run()
    runtime, transport = _runtime_over(
        monkeypatch, run, (MOB_ADDRESSING,), {"meta/identity/create": {"error": error}}
    )
    spec = meerkat_meta.AgentSpec(name="scribe", profile="writer")

    with pytest.raises(meerkat_meta.IdentityRefusedError) as refused:
        await runtime.mob("mob-b").identities.create([spec], request_id="create-scribe")

    assert refused.value.kind == "name_taken"
    assert transport.sent()[1][1]["mob_id"] == "mob-b"


# -- The mob of an event query --------------------------------------------------


@pytest.mark.asyncio
async def test_an_event_query_sends_the_mob_it_is_given_and_the_handles_otherwise(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    query = {"member_id": "worker-1", "after_seq": 7, "limit": 5}
    run = _Run()
    runtime, transport = _runtime_over(monkeypatch, run, (MOB_ADDRESSING,))

    # The handle of the launch's own mob: the caller's mob, or none.
    await runtime.mob_handle().query_events(**query, mob_id="mob-c")
    await runtime.mob_handle().query_events(**query)
    # A named handle: the caller's mob when one is given, its own otherwise.
    await runtime.mob("mob-b").query_events(**query, mob_id="mob-c")
    await runtime.mob("mob-b").query_events(**query)

    # A query that names a mob asks the gateway first, once, whichever handle
    # sends it; a query of the launch's own mob that names none asks nothing.
    assert transport.sent() == [
        CAPABILITY_READ,
        ("meta/query_events", {**query, "mob_id": "mob-c"}),
        ("meta/query_events", query),
        ("meta/query_events", {**query, "mob_id": "mob-c"}),
        ("meta/query_events", {**query, "mob_id": "mob-b"}),
    ]


@pytest.mark.asyncio
@pytest.mark.parametrize("features", [None, (), (HOSTED_MOBS,)], ids=["none", "empty", "other"])
async def test_without_the_capability_the_launchs_handle_sends_no_event_query_that_names_a_mob(
    monkeypatch: pytest.MonkeyPatch, features: tuple[str, ...] | None
) -> None:
    # A gateway that does not honour ``mob_id`` would answer such a query
    # without an error and for its own mob: it is refused here instead.
    run = _Run()
    runtime, transport = _runtime_over(monkeypatch, run, features)
    handle = runtime.mob_handle()

    with pytest.raises(CapabilityUnavailableError) as refused:
        await handle.query_events(member_id="worker-1", mob_id="mob-c")

    assert refused.value.data == {"capability": MOB_ADDRESSING}
    assert transport.sent() == [CAPABILITY_READ]

    # A query that names no mob is the launch's own mob's, as before.
    assert await handle.query_events(member_id="worker-1") == []
    assert transport.sent()[1:] == [("meta/query_events", {"member_id": "worker-1"})]


@pytest.mark.asyncio
async def test_an_event_query_of_the_launchs_handle_that_names_no_mob_reads_no_capability(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    run = _Run()
    runtime, transport = _runtime_over(monkeypatch, run, features=())

    await runtime.mob_handle().query_events(member_id="worker-1")

    assert transport.sent() == [("meta/query_events", {"member_id": "worker-1"})]
