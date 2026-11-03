"""Protected presentation of one actual same-host Toolkit controller review.

The trusted application supplies an already retained original, source evidence
and its existing private client. Browser data cannot start or select a review.
This owner does not enroll an operator, select policy, grant work, start a host
or close the shared host client. Its final native outcome is historical input
for the trusted compiler, never a current permission or readiness projection.
"""
from __future__ import annotations

import asyncio
import copy
from datetime import datetime, timezone
import inspect
import time

from meerkat_toolkit.host_control import (
    PrivateHostClient, PrivateOriginalRef, PrivateSourceEvidence,
    RemoteControllerLogin, RemoteControllerReview,
)

if __package__:
    from .console_connect import _prompt
else:
    from console_connect import _prompt

RETIRE_RESULT_GRACE_SECONDS = 5.0


class ControllerReviewError(RuntimeError):
    """An application presentation refusal with no native diagnostic body."""
    def __init__(self, code):
        self.code = code
        super().__init__("The protected review is unavailable.")


def _expired(prompt):
    expiry = prompt["expires_at"]
    return expiry is not None and datetime.fromisoformat(
        expiry.replace("z", "+00:00").replace("Z", "+00:00")) <= datetime.now(timezone.utc)


class NativeControllerReview:
    """One invocation, one claimed answer and one retained native result.

    All public owner methods run on the ConsoleFront event loop. Synchronous
    claims fence current application/login ownership before scheduling a write.
    Blocking SDK calls use workers, all retained and joined on retirement.
    """
    def __init__(self, client, original, content, transport_authentication, login, *, is_current):
        if (not isinstance(client, PrivateHostClient)
                or not isinstance(original, PrivateOriginalRef)
                or not isinstance(content, str)
                or not isinstance(transport_authentication, PrivateSourceEvidence)
                or not callable(is_current) or inspect.iscoroutinefunction(is_current)):
            raise ValueError("A controller review requires trusted same-host application inputs.")
        # This is invocation correlation only. Native policy chooses its operator.
        self._login = RemoteControllerLogin(login.login_scope_id, login.expires_at_ms)
        self._origin = login.origin
        self.is_current = is_current
        self._client, self._original, self._content = client, original, content
        self._authentication = transport_authentication
        self._begin_task = self._result_task = self._observe_task = None
        self._answer_task = self._retire_task = None
        self._review = None
        self._prompt = None
        self._retired = self._unknown = False
        self._answer_claim = None

    @property
    def login_scope_id(self):
        return self._login.scope_id

    @property
    def expires_at_ms(self):
        return self._login.expires_at_ms

    @property
    def origin(self):
        return self._origin

    def _current(self):
        try:
            return not self._retired and self.is_current() is True
        except Exception:
            return False

    @staticmethod
    def _consume(task):
        # Retain failures for the actual owner, without unattended diagnostics.
        task.add_done_callback(lambda done: done.exception() if not done.cancelled() else None)
        return task

    def start(self):
        """Called once by the trusted successful-login owner, never by a route."""
        if self._begin_task is not None or not self._current():
            raise ControllerReviewError("invalid_binding")
        self._begin_task = self._consume(asyncio.create_task(self._begin()))

    async def _begin(self):
        # start() already synchronously claimed this exact current invocation.
        try:
            review = await asyncio.to_thread(self._client.begin_remote_controller_review,
                self._original, self._content, self._authentication, self._login,
                # Native human eligibility still expires at the exact login
                # deadline. Only the retained result wait gets cleanup grace.
                timeout=max(0.001, self.expires_at_ms / 1000 - time.time()) + RETIRE_RESULT_GRACE_SECONDS)
            if not isinstance(review, RemoteControllerReview):
                raise ValueError("Invalid native review handle.")
            self._review = review
            self._result_task = self._consume(asyncio.create_task(self._result()))
        except BaseException:
            self._unknown = True
            raise ControllerReviewError("outcome_unknown") from None

    async def _result(self):
        try:
            return await asyncio.to_thread(self._review.result)
        except BaseException:
            self._unknown = True
            raise ControllerReviewError("outcome_unknown") from None
        finally:
            self._prompt = None

    def _finished(self):
        return self._result_task is not None and self._result_task.done()

    def _browser_state(self):
        if self._unknown:
            return {"status": "outcome_unknown"}
        if self._finished():
            # Even selected lineage remains private historical compiler data.
            result = self._result_task.result()
            return {"status": "review_completed" if result["state"] == "selected" else result["state"]}
        if self._answer_claim is not None:
            return {"status": "answer_forwarded" if self._answer_task.done() else "answer_submitted"}
        return {"status": "awaiting_prompt"}

    async def observe(self):
        if not self._current():
            raise ControllerReviewError("invalid_binding")
        if self._begin_task is None:
            raise ControllerReviewError("unavailable")
        if not self._begin_task.done():
            return {"status": "starting"}
        if self._unknown or self._finished() or self._answer_claim is not None:
            return self._browser_state()
        self._begin_task.result()
        if self._observe_task is None or self._observe_task.done():
            self._observe_task = self._consume(asyncio.create_task(self._observe()))
        await asyncio.shield(self._observe_task)
        if not self._current():
            raise ControllerReviewError("invalid_binding")
        if self._unknown or self._finished() or self._answer_claim is not None:
            return self._browser_state()
        if self._prompt is None:
            return {"status": "awaiting_prompt"}
        if _expired(self._prompt):
            self._prompt = None
            return {"status": "prompt_expired"}
        return {"status": "prompt", "prompt": copy.deepcopy(self._prompt)}

    async def _observe(self):
        try:
            state = await asyncio.to_thread(self._client.observe_controller_review, self._review.review_id)
            if self._finished() or self._answer_claim is not None or self._retired:
                return
            if state["state"] == "prompt":
                if state["prompt"].get("kind") != "approval":
                    raise ValueError("Unexpected native prompt kind.")
                # Reuse only projection validation, not ConsoleConnect frames.
                prompt = _prompt(state["prompt"], self.origin)
                if self._prompt is not None and self._prompt != prompt:
                    raise ValueError("The retained native prompt changed.")
                self._prompt = prompt
            elif state["state"] == "awaiting_prompt":
                if self._prompt is not None:
                    raise ValueError("The retained native prompt disappeared.")
            else:
                # No application answer was claimed, so another owner cannot
                # silently make this authenticated browser's decision current.
                raise ValueError("Unexpected native answer state.")
        except BaseException:
            self._prompt = None
            self._unknown = True
            # A control reply can race the final retained native response.
            # Preserve that result, but never retry this observation or answer.
            return

    def claim_answer(self, approval_id, decision):
        """Claim before any await; browser can name only the displayed approval."""
        if not self._current():
            raise ControllerReviewError("invalid_binding")
        if self._unknown or self._finished() or self._answer_claim is not None:
            raise ControllerReviewError("stale_prompt")
        if (not isinstance(approval_id, str) or not isinstance(decision, str)
                or self._prompt is None or approval_id != self._prompt["approval_id"]
                or decision not in self._prompt["allowed_decisions"] or _expired(self._prompt)):
            raise ControllerReviewError("stale_prompt")
        self._answer_claim = (approval_id, decision)
        self._prompt = None
        self._answer_task = self._consume(asyncio.create_task(self._answer()))

    async def _answer(self):
        try:
            await asyncio.to_thread(self._client.answer_controller_review,
                self._review.review_id, *self._answer_claim)
        except BaseException:
            self._unknown = True
            raise ControllerReviewError("outcome_unknown") from None

    async def historical_outcome(self):
        """Trusted caller only; survives revoked disclosure and never resends."""
        if self._begin_task is None:
            raise ControllerReviewError("unavailable")
        await asyncio.shield(self._begin_task)
        return copy.deepcopy(await asyncio.shield(self._result_task))

    def revoke(self):
        """Synchronous disclosure fence used before asynchronous retirement."""
        self._retired = True
        self._prompt = None

    async def retire(self, reason):
        self.revoke()
        if self._retire_task is None:
            self._retire_task = self._consume(asyncio.create_task(self._retire()))
        await asyncio.shield(self._retire_task)

    async def _retire(self):
        if self._begin_task is None:
            return
        await asyncio.shield(self._begin_task)
        entered = [task for task in (self._observe_task, self._answer_task) if task is not None]
        # Join the entered answer/control write before retiring its decision
        # invocation. Native retirement preserves a decision already accepted.
        await asyncio.gather(*(asyncio.shield(task) for task in entered), return_exceptions=True)
        if not self._finished():
            try:
                await asyncio.to_thread(self._client.retire_controller_review, self._review.review_id)
            except BaseException:
                self._unknown = True
        # Join the actual retained final response even after a lost retire ACK.
        joined = await asyncio.gather(asyncio.shield(self._result_task), return_exceptions=True)
        if any(isinstance(result, BaseException) for result in joined):
            raise ControllerReviewError("cleanup_unconfirmed")
        # A lost retirement ACK cannot prove that an undecided invocation was
        # retired, even if its separate final observation happened to arrive.
        if self._unknown:
            raise ControllerReviewError("cleanup_unconfirmed")

    async def close(self):
        await self.retire("application_stopping")
