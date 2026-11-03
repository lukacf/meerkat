"""One live schema4 preparation and ordinary native runtime connection.

The application finalizer consumes an actual completed compiler artifact. It
owns candidate validation and activation publication; it does not stand in for
native member finish. Only the ordinary native init settlement establishes
readiness. This module never launches a replacement preparation automatically.
"""
from __future__ import annotations

import asyncio
import base64
import hashlib
import os
import stat
from collections.abc import Awaitable, Callable
from typing import Any

from .host_control import (
    ApplicationPreparation,
    ApplicationPreparationAttempt,
    ApplicationPreparationSettlement,
    HostBootstrapSelection,
    HostControlError,
    SelectedHostArtifact,
    ToolkitMobKitRuntime,
    ToolkitPersistentTransport,
)


class ToolkitApplicationRuntime(ToolkitMobKitRuntime):
    """Prepare, publish and activate one application in the same native child.

    ``finalize`` is the trusted application's existing installation transaction:
    validate the completed artifact, compile and validate the final candidate,
    publish its observed revision, then return that exact selection. It must
    retain a published selection if any later step fails. An entered finalizer
    remains joined on caller loss; this owner never cancels it or infers a
    rollback from transport failure.

    Supply the real provider/builder configuration without ``mob_config`` or a
    second tool-policy input. Native authenticates the selected plan carried by
    both the inherited private request and the ordinary opaque init field.
    """

    def __init__(
        self,
        config: Any,
        preparation: ApplicationPreparation,
        finalize: Callable[[SelectedHostArtifact], Awaitable[HostBootstrapSelection]],
    ):
        if not isinstance(preparation, ApplicationPreparation) or not callable(finalize):
            raise HostControlError("application preparation and finalizer are required")
        # Keep the exact bytes supplied on this attempt. Native independently
        # opens and authenticates the same selected local artifact.
        descriptor = os.open(preparation.plan.path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
        try:
            if not stat.S_ISREG(os.fstat(descriptor).st_mode):
                raise HostControlError("application preparation plan must be a regular file")
            with os.fdopen(descriptor, "rb", closefd=False) as source:
                plan = source.read(256 * 1024 + 1)
                if len(plan) > 256 * 1024:
                    raise HostControlError("application preparation plan exceeds its artifact limit")
        finally:
            os.close(descriptor)
        if hashlib.sha256(plan).hexdigest() != preparation.plan.sha256:
            raise HostControlError("application preparation plan changed")
        # The subclass supplies the preparation transport below. The parent's
        # ordinary lifecycle and host_control accessor remain the same owners.
        super().__init__(config, preparation)
        self._application_preparation = preparation
        self._application_plan = plan
        self._application_finalizer = finalize
        self._application_started = False
        self._application_closing = False
        self.preparation_attempt: ApplicationPreparationAttempt | None = None

    def _make_gateway_transport(self) -> ToolkitPersistentTransport:
        if self._application_started:
            raise HostControlError("application preparation already started; inspect the original attempt")
        if self._config.gateway_bin is None:
            raise HostControlError("application preparation requires its local gateway")
        self._application_started = True
        return ToolkitPersistentTransport(self._config.gateway_bin, self._application_preparation)

    def _build_init_params(self) -> dict[str, Any]:
        params = super()._build_init_params()
        if "mob_config" in params or "application_tool_policies" in params or "prepared_application" in params:
            raise HostControlError("application preparation conflicts with a second definition or policy")
        params["prepared_application"] = {
            "bundle_base64": base64.b64encode(self._application_plan).decode("ascii"),
            "digest": self._application_preparation.plan.sha256,
        }
        return params

    async def _publish_and_adopt(self, attempt: ApplicationPreparationAttempt) -> None:
        manifest = await asyncio.to_thread(attempt.observe)
        if self._application_closing:
            raise HostControlError("application preparation closed before publication")
        # No await separates the closing check from entering this actual
        # application-owned coroutine. Once entered it is always joined.
        selection = await self._application_finalizer(manifest)
        await asyncio.to_thread(attempt.adopt, selection)

    async def _close_preparation(self, transport: ToolkitPersistentTransport) -> None:
        self._application_closing = True
        if transport.host_control is not None:
            # Wake the same private request owner. Closing this channel is not
            # proof of native cleanup; the ordinary init/stop owners still join.
            await asyncio.to_thread(transport.host_control.close)

    async def _run_init(self, transport: ToolkitPersistentTransport) -> Any:
        attempt = transport.preparation
        if attempt is None:
            raise HostControlError("application preparation request was not retained")
        self.preparation_attempt = attempt
        native = asyncio.create_task(super()._run_init(transport))
        private = asyncio.create_task(self._publish_and_adopt(attempt))
        joined = asyncio.gather(native, private, return_exceptions=True)
        closing: asyncio.Task[None] | None = None
        try:
            done, _ = await asyncio.wait((native, private), return_when=asyncio.FIRST_EXCEPTION)
            if any(task.cancelled() or task.exception() is not None for task in done):
                self._application_closing = True
                closing = asyncio.create_task(self._close_preparation(transport))
                await asyncio.shield(closing)
            results = await asyncio.shield(joined)
        except asyncio.CancelledError:
            # Neither the accepted native init nor an entered publication is
            # cancelled. Retain both through their actual settlement or loss.
            self._application_closing = True
            if closing is None:
                closing = asyncio.create_task(self._close_preparation(transport))
            cleanup = asyncio.gather(closing, joined, return_exceptions=True)
            while not cleanup.done():
                try:
                    await asyncio.shield(cleanup)
                except asyncio.CancelledError:
                    continue
            close_result, results = cleanup.result()
            attempt.settlement = ApplicationPreparationSettlement(results[0], results[1], close_result)
            raise
        except BaseException:
            # A local close failure cannot discard either entered operation.
            # Preserve its exact result alongside both joined task outcomes.
            assert closing is not None
            cleanup = asyncio.gather(closing, joined, return_exceptions=True)
            while not cleanup.done():
                try:
                    await asyncio.shield(cleanup)
                except asyncio.CancelledError:
                    continue
            close_result, results = cleanup.result()
            attempt.settlement = ApplicationPreparationSettlement(results[0], results[1], close_result)
            raise
        attempt.settlement = ApplicationPreparationSettlement(
            results[0], results[1], closing.result() if closing is not None else None,
        )
        # Prefer the native typed failure, including durable_effects/unknown,
        # over the private EOF caused by that failure. Both actual outcomes
        # remain on the attempt; final owner adoption is not native Ready.
        if isinstance(results[0], BaseException):
            raise results[0]
        if isinstance(results[1], BaseException):
            raise results[1]
        return results[0]
