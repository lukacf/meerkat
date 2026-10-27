"""Optional wiring from an authenticated host step to Toolkit's existing UI.

The host supplies admission and a separately protected human presenter. This
module creates no user session, endpoint, credential store or setup lifecycle.
Never expose the binding, client or presenter through agent tools.
"""
from __future__ import annotations

import asyncio
from dataclasses import dataclass
from typing import Any, Awaitable, Callable, Mapping

from .connector_auth import (
    ConnectorCredentialObservation, ConnectorLoginCustodian, ConnectorTarget,
    Known, observe_connector_auth,
)


class ConnectorHostError(RuntimeError):
    """Fixed host-composition diagnostic, never native refusal classification."""


def _text(value: Any, limit: int = 4096) -> str:
    if (type(value) is not str or not value.strip() or len(value) > limit
            or any(ord(c) < 32 or ord(c) == 127 for c in value)):
        raise ConnectorHostError("invalid_host_control")
    return value


@dataclass(frozen=True)
class HostStepReference:
    owner: str
    reference: str
    revision: str

    def __post_init__(self):
        for value in (self.owner, self.reference, self.revision):
            _text(value)

    def to_view(self) -> dict[str, str]:
        return {"owner": self.owner, "reference": self.reference, "revision": self.revision}


@dataclass(frozen=True)
class SignInControlRequest:
    """Frozen presentation correlation; admission remains a host operation."""
    scope: str
    view_revision: int
    kind: str
    offer: str
    target: HostStepReference
    explanation: str | None = None

    @classmethod
    def from_view(cls, value: Mapping[str, Any]) -> SignInControlRequest:
        invalid = False
        try:
            if type(value) is not dict or set(value) != {"scope", "viewRevision", "control"}:
                raise ValueError
            control = value["control"]
            if (type(control) is not dict or not {"kind", "offer", "target"} <= set(control)
                    or set(control) - {"kind", "offer", "target", "explanation"}):
                raise ValueError
            target = control["target"]
            if type(target) is not dict or set(target) != {"owner", "reference", "revision"}:
                raise ValueError
            revision = value["viewRevision"]
            if type(revision) is not int or not 0 <= revision <= 9007199254740991:
                raise ValueError
            explanation = control.get("explanation")
            if explanation is not None:
                _text(explanation)
            result = cls(_text(value["scope"]), revision, _text(control["kind"]),
                         _text(control["offer"]), HostStepReference(**target), explanation)
        except Exception:
            invalid = True
        if invalid:
            raise ConnectorHostError("invalid_host_control")
        return result

    def control_view(self) -> dict[str, Any]:
        result = {"kind": self.kind, "offer": self.offer, "target": self.target.to_view()}
        if self.explanation is not None:
            result["explanation"] = self.explanation
        return result


# The host authenticates context and checks the current person, scope, offer,
# reference/revision, target and operation. It raises on denied/stale admission.
Revalidate = Callable[[Any, str, SignInControlRequest | None, ConnectorTarget], Awaitable[None]]


class ConnectorSignInBinding:
    """Retain this optional adapter on an existing authenticated host step.

    A callback's presence does not establish protection. Only the embedding
    host can supply and attest a real presenter outside agent observation and
    control. None disables opening. Reads remain explicitly host-admitted.

    ControlRequest parsing is not authorization. revalidate must check each
    call, including reads, replay and the second check before presentation.
    The host owns shared-waiter decisions and whole-setup stop semantics.
    """

    def __init__(self, client: Any, target: ConnectorTarget, *,
                 reference: HostStepReference, revalidate: Revalidate,
                 protected_presenter: Callable[[str], Awaitable[None]] | None = None,
                 callback_timeout: float = 240, rpc_timeout: float = 10):
        if (type(reference) is not HostStepReference or not callable(revalidate)
                or (protected_presenter is not None and not callable(protected_presenter))):
            raise ConnectorHostError("invalid_host_composition")
        self._client, self._target = client, target
        self._reference, self._revalidate = reference, revalidate
        self._presenter, self._rpc_timeout = protected_presenter, rpc_timeout
        self._custodian = ConnectorLoginCustodian(client, target,
            callback_timeout=callback_timeout, rpc_timeout=rpc_timeout)
        self._handle = None

    def __repr__(self):
        return "ConnectorSignInBinding(<host-private>)"

    def _request(self, value: Mapping[str, Any]) -> SignInControlRequest:
        request = SignInControlRequest.from_view(value)
        if request.target != self._reference:
            raise ConnectorHostError("host_reference_mismatch")
        return request

    async def _admit(self, context: Any, operation: str,
                     request: SignInControlRequest | None = None) -> None:
        failed, cancelled = False, False
        try:
            await self._revalidate(context, operation, request, self._target)
        except asyncio.CancelledError:
            cancelled = True
        except Exception:
            failed = True
        if cancelled:
            raise asyncio.CancelledError("connector_host_admission_wait_cancelled")
        if failed:
            raise ConnectorHostError("host_admission_unconfirmed")

    async def dispatch(self, context: Any, value: Mapping[str, Any]) -> dict[str, Any]:
        """Wire the existing UI dispatcher; returns acknowledgement, never readiness.

        A stop_setup control must already be admitted by the host as stopping
        this sign-in transport without harming other waiters. It does not stop
        the whole task, disconnect an account or roll back any completed effect.
        """
        request = self._request(value)
        if request.kind not in ("open_authorization", "reconnect", "stop_setup"):
            raise ConnectorHostError("unsupported_host_control")
        if request.kind == "reconnect" and not isinstance(self._target.account_selection, Known):
            raise ConnectorHostError("reconnect_requires_known_target")
        if request.kind != "stop_setup" and self._presenter is None:
            raise ConnectorHostError("protected_presenter_unavailable")
        await self._admit(context, request.kind, request)
        if request.kind == "stop_setup":
            cleanup = await self._custodian.close()
            return {"kind": "cleanup_observed", "callbackClosed": cleanup.callback_closed,
                    "nativeCancel": cleanup.native_cancel}
        handle = await self._custodian.begin()
        self._handle = handle
        await self._admit(context, "present", request)
        presented = await handle.present(self._presenter)
        return {"kind": "presentation_returned" if presented else "presentation_unconfirmed"}

    async def wait(self, context: Any) -> ConnectorCredentialObservation:
        """Wait for an already started callback. Never starts or resumes a login."""
        await self._admit(context, "wait")
        if self._handle is None:
            raise ConnectorHostError("login_not_started")
        observation = await self._handle.wait()
        await self._admit(context, "read_result")
        return observation

    async def observe(self, context: Any) -> ConnectorCredentialObservation:
        """One authorized native status read, with no refresh or retry.

        At the reviewed native source, status still awaits the owner's
        correction. This return value is a native report, not usable authority.
        """
        await self._admit(context, "status")
        observation = await observe_connector_auth(self._client, self._target,
                                                   rpc_timeout=self._rpc_timeout)
        await self._admit(context, "read_result")
        return observation

    async def project_action(self, context: Any, *, action_id: str, integration: str,
                             state: str, instructions: str,
                             control_request: Mapping[str, Any] | None = None) -> dict[str, Any]:
        """Build an existing UserActionView using the host's explicit action state.

        The host owns state/instructions and filters this view to its audience.
        No state is computed from a credential phase, timer or local callback.
        """
        request = self._request(control_request) if control_request is not None else None
        await self._admit(context, "project", request)
        if state not in ("queued", "action_required", "verifying", "complete",
                         "expired", "no_longer_needed"):
            raise ConnectorHostError("invalid_action_projection")
        controls = []
        if request is not None:
            if request.kind not in ("open_authorization", "reconnect", "stop_setup"):
                raise ConnectorHostError("unsupported_host_control")
            if request.kind == "reconnect" and not isinstance(self._target.account_selection, Known):
                raise ConnectorHostError("reconnect_requires_known_target")
            if request.kind == "stop_setup" or self._presenter is not None:
                controls.append(request.control_view())
        selection = self._target.account_selection
        result = {"id": _text(action_id), "integration": _text(integration),
                  "kind": "browser_authorization", "state": state,
                  "instructions": _text(instructions), "controls": controls,
                  "requestedAccountLabel": (selection.account
                      if isinstance(selection, Known) else "Account chosen during sign-in")}
        if self._presenter is None:
            result["instructions"] = "This host has not supplied a protected sign-in presenter."
        if self._handle is not None:
            result["authorizationOrigin"] = self._handle.presentation.authorization_origin
        return result

    async def shutdown(self):
        """Explicit host shutdown only. Never call from panel/request disposal."""
        return await self._custodian.close()


def credential_view_fields(observation: ConnectorCredentialObservation) -> dict[str, Any]:
    """Inert ConnectionView fields from a report already admitted for this viewer.

    The host supplies connection identity, grants, pending tasks and controls.
    This source-contract profile deliberately cannot project ready or grant use.
    """
    account = observation.verified_account
    label = (f"Native reports {account.subject} ({account.issuer}; {account.strategy_id})"
             if account is not None else "No account reported")
    scopes = ("Not reported" if observation.scopes is None
              else ", ".join(observation.scopes) or "No scopes reported")
    return {"account": label, "credentialScope": scopes,
            "status": {"tone": "attention", "text": "Native report; host verification pending."}}
