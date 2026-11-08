"""In-process Forge exercise for one existing Connect-owned native runtime.

This is executable consumer code, not a standalone launcher or native test
result. Forge supplies its actual generated Application, connected runtime,
packaged class, and host-selected MemberClassAddRequest.

The required ordering is to durably record an admitting member excluded from
desired_roster,
then list it ONLY inside native held completion after Connect policy commits.
The application must not acquire _reconcile_lock inside that finalizer: normal
reconciliation may already hold it while waiting for the native controller.
Prepublishing the member permits ordinary reconciliation to bypass held Add.
Pass that actual admitting row's spec and the application's async callback to
begin_admitting_add(application, runtime, request, publish=publish_actual_admitting_row).
That callback must commit the application's existing admitting-to-listed
transaction and refresh its desired projection without _reconcile_lock. This
example accepts the real Forge domain transaction from its caller. No substitute
durable mutation API is invented here.

For an explicitly authorized retry after retiring an earlier correlation,
Observe returns one canonical record/checkpoint and a separate authorization
result. The caller may supply that exact checkpoint as request.expected_predecessor.
A retired record remains inspectable with Denied authorization; inspecting it
is neither permission nor a retry. This example never fetches a predecessor,
changes a correlation or retries Add automatically.

The application chooses request.id before writing admitting(c); that exact ID
is its durable correlation c. The callback closes over c, request.spec.identity
and request.instance['class']. attempt.operation_id is only the inherited pipe's
correlation, not a durable application ID. The real transaction must compare c
with the row's pending correlation and reject a voided c. It returns to this SDK
callback only after durable listing and projection publication; a refused
comparison must raise, not return False as though publication succeeded.

verify_zero_start can inspect the existing desired projection. observe_restored
can inspect an already restored application, its authorization, native status
and resolved tools. These observations neither perform nor prove cold recovery.
ordinary_callback_read remains a normal authorized task through Meta.

Retain attempt before waiting. MemberClassObservationUnknown carries its
operation_id, including a failed partial write. It neither means admission
nor allows automatic replay, retirement or desired-roster rollback.
await observe_admitting_add(runtime, attempt) performs the explicit native Join
for Unsettled, then joins listing before returning both observations and an
optional listing_error. That error describes the listing acknowledgment only;
known terminal native, authorization and cleanup facts are retained.
A reservation_not_accepted result changed no ownership and may refer to an
already accepted operation's correlation: do not void or retire that operation
because this duplicate was refused. Observe it through its original owner.
A preflight_refused result entered no native Add; inspect its separate joined
cleanup result. Neither result warrants native Join or a fabricated native
refusal. For an accepted operation, the caller uses its existing application
mutation to unlist/reconcile on refusal or
failed authorization/cleanup, including an initial Unsettled result. Record a
sticky void for c: every later listing attempt for that c must refuse, even if a
late ACK arrives. An unknown result also requires the real owner's durable-row
reconciliation, not a blind retry. Inspect terminal['native']['result']['outcomes']
for the selected identity: restore.stage 'minted' is native Created. A listing
ACK or terminal state 'settled' alone does not establish Created or authority.
If either observation or listing join is unknown, retain the attempt and
reconcile the actual durable row; do not replay Add or assume rollback.
After handling an admission result, the application performs its ordinary
reconcile once. This example returns the evidence before that caller-owned step.

The callback is a local, fast transaction. Its authorization budget is only the
remaining native held deadline selected by request.hold_timeout_ms; Python
observation timeouts do not extend it. A user prompt, provider authentication or
other unbounded work does not belong inside this callback.

Forge's actual durable admitting/listing mutation remains a required consumer
input; the SDK and private-pipe finalizer are wired but require native acceptance. Production Add and cold recovery are not established
by this example.
"""
from __future__ import annotations

import asyncio
import copy
import dataclasses
from typing import Any, Awaitable, Callable

from meerkat_toolkit.host_control import HostReplyCode, HostReplyError, ToolkitMobKitRuntime
from meerkat_toolkit.member_classes import MemberClassAddAttempt, MemberClassAddRequest, MemberClassListingUnknown


def _same_runtime(application: Any, runtime: ToolkitMobKitRuntime) -> None:
    if (not isinstance(runtime, ToolkitMobKitRuntime) or not runtime.is_running
            or application.context.runtime is not runtime or not application.dynamic):
        raise ValueError("Use this generated application's connected Toolkit runtime")
    # The existing Meta dispatcher must own these actual callbacks. Registering
    # another dispatcher or invoking tool handlers directly would bypass the
    # production composition this exercise is intended to demonstrate.
    if (runtime._config.roster_provider is not application
            or runtime._config.agent_customizer is not application):
        raise ValueError("Use the existing application roster and identity customizer")


def _reusable_profile(application: Any, class_id: str) -> str:
    profile = application.deployment["applicationProfiles"][class_id]
    seeds = application.deployment["agents"]
    if profile in {seed["profile"] for seed in seeds}:
        raise ValueError("Reusable class role must not reuse seed-specific instructions")
    return profile


async def verify_zero_start(application: Any, runtime: ToolkitMobKitRuntime,
                            class_id: str) -> dict[str, Any]:
    """Require no desired instances of this class; retain other seed agents."""
    _same_runtime(application, runtime)
    profile = _reusable_profile(application, class_id)
    async with application._reconcile_lock:
        # This is the existing Forge projection from integration.desired_roster
        # and desired_edges, without invoking reconcile or creating a member.
        await application._prepare_snapshot(runtime=runtime)
        if any(agent["classId"] == class_id for agent in application._snapshot[0].values()):
            raise ValueError("The selected class already has a desired instance")
    return {"class": class_id, "reusable_profile": profile,
            "native_identities": await runtime.list_identities()}


def begin_admitting_add(application: Any, runtime: ToolkitMobKitRuntime,
                        request: MemberClassAddRequest, *,
                        publish: Callable[[], Awaitable[None]]) -> MemberClassAddAttempt:
    """Begin one held Add for the caller's already durable admitting row.

    The row must be excluded from desired_roster, while the actual identity
    customizer can still build it. These checks inspect the existing projection;
    they do not establish durability or refresh/publish that projection. Native
    calls publish only at its held-completion barrier after policy commits.
    """
    _same_runtime(application, runtime)
    request.wire()
    if request.spec.profile != _reusable_profile(application, request.instance["class"]):
        raise ValueError("Later members must select the packaged reusable class profile")
    if any(row.identity == request.spec.identity for row in application._snapshot[1]):
        raise ValueError("The admitting member must be excluded from the desired projection")
    return runtime.member_classes.begin_add(request, publish=publish)


async def observe_admitting_add(runtime: ToolkitMobKitRuntime,
                                attempt: MemberClassAddAttempt) -> dict[str, Any]:
    """Join actual native settlement and listing before caller-owned rollback.

    No application write occurs here. Expected listing uncertainty is returned
    separately without discarding the known native result. It requires checking
    the application's durable row, not another native Join or Add. Cancellation,
    unknown native observation and unexpected errors still propagate; none is
    evidence of rollback or recovery.
    """
    initial = await asyncio.to_thread(attempt.result)
    terminal = initial
    if initial["state"] == "unsettled":
        terminal = await asyncio.to_thread(runtime.member_classes.join, initial["settlement_id"])
    listing_error = None
    try:
        await attempt.join_listing()
    except MemberClassListingUnknown as error:
        listing_error = error
    return {"initial": initial, "terminal": terminal, "listing_error": listing_error}


async def _validate_existing_desired(application: Any, runtime: ToolkitMobKitRuntime,
                                     request: MemberClassAddRequest) -> None:
    """Compare a spec already present in the existing application's projection.

    Reading desired_roster does not prove its backing store is durable. The
    caller supplies an already restored application. This read-only validation
    is not an Add preparation step and must not be used as a held finalizer.
    No profile, owner, route, grant or callback is synthesized here.
    """
    _same_runtime(application, runtime)
    request.wire()
    profile = _reusable_profile(application, request.instance["class"])
    if request.spec.profile != profile:
        raise ValueError("Later members must select the packaged reusable class profile")
    async with application._reconcile_lock:
        await application._prepare_snapshot(runtime=runtime)
        selected = next((row for row in application._snapshot[1]
                         if row.identity == request.spec.identity), None)
        if selected is None or selected.to_dict() != request.spec.to_dict():
            raise ValueError("Requested member differs from the existing desired projection")


async def ordinary_callback_read(runtime: ToolkitMobKitRuntime, identity: str,
                                  prompt: str, *, timeout: float = 90) -> Any:
    """Send once through Meta; inspect the real callback trace with its answer.

    Supply an authorized read task using a declared callback. Returned text by
    itself is not evidence that the callback ran. Keep the native tool event
    and the application's existing callback evidence for that exact turn.
    """
    return await runtime.agent(identity).send_and_wait_outcome(
        prompt, timeout=timeout, require_attribution=True)


async def observe_restored(application: Any, runtime: ToolkitMobKitRuntime,
                           request: MemberClassAddRequest) -> dict[str, Any]:
    """Read an already-restored owner without replaying Add or proving recovery."""
    await _validate_existing_desired(application, runtime, request)
    member = request.instance["member"]["member"]
    authorization = await asyncio.to_thread(runtime.member_classes.observe, request.id, member)
    status = await runtime.status(request.spec.identity)
    tools = await runtime.identity_resolved_tools_detail(request.spec.identity)
    return {"authorization": authorization, "native_status": status, "native_tools": tools}


async def observe_unknown_class_refusal(runtime: ToolkitMobKitRuntime,
                                        request: MemberClassAddRequest,
                                        unknown_class: str) -> dict[str, Any]:
    """A native refusal control using an explicitly absent packaged class.

    Run before the valid Add, with a fresh admission ID supplied in request.
    It does not mutate the desired store. The returned native refusal must be
    reviewed; neither a timeout nor an arbitrary exception counts as refusal.
    """
    instance = copy.deepcopy(request.instance)
    instance["class"] = unknown_class
    refused = dataclasses.replace(request, instance=instance)
    async def unexpected_listing() -> None:
        raise AssertionError("An absent class must not reach application listing")
    try:
        attempt = runtime.member_classes.begin_add(refused, publish=unexpected_listing)
        result = await asyncio.to_thread(attempt.result)
    except HostReplyError as error:
        if error.code == HostReplyCode.DENIED:
            return {"native_reply": error.code.value}
        raise
    raise AssertionError(f"Unknown class did not return a native refusal: {result!r}")
