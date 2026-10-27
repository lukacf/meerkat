"""HTTP presentation for already registered, original native provider owners.

No HTTP field selects an executable, principal, realm, credential scope or new
owner. The installation launcher registers logout/expiry before calling add.
"""
from __future__ import annotations

import asyncio
import inspect
import json

from starlette.responses import JSONResponse

from .provider_setup_host import ForgeProviderSetupSession, ProviderSetupUnavailable, _retain


def response(value, status=200):
    return JSONResponse(value, status_code=status, headers={
        "Cache-Control": "no-store", "Pragma": "no-cache", "Referrer-Policy": "no-referrer",
        "X-Content-Type-Options": "nosniff"})


def unavailable(status=503):
    return response({"detail": "Native provider setup is unavailable for this login."}, status)


def _object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError("Duplicate field")
        result[key] = value
    return result


def offered_answer(prompt, answer):
    """Validate wire shape against this prompt, never a provider capability table."""
    if type(answer) is not dict:
        return False
    kind, offered = answer.get("kind"), prompt.get("kind")
    if answer == {"kind": "cancel"}:
        return True
    if offered == "target_review":
        return answer == {"kind": "confirm"}
    if offered == "choices" and set(answer) == {"kind", "choice_id"} and kind == "choice":
        return any(row.get("choice_id") == answer["choice_id"] and row.get("selectable") is True
                   for row in prompt.get("choices", []) if type(row) is dict)
    if offered == "action" and set(answer) == {"kind", "action"} and kind == "action":
        return type(answer["action"]) is str and answer["action"] in prompt.get("actions", [])
    field, limit = ("key", 8192) if offered == "key" else ("url", 16384)
    expected = "key" if offered == "key" else "callback" if offered == "browser_manual" else None
    value = answer.get(field)
    return (expected is not None and kind == expected and set(answer) == {"kind", field}
            and type(value) is str and bool(value.strip()) and len(value.encode("utf-8")) <= limit
            and not any(ord(c) < 32 or 127 <= ord(c) <= 159 for c in value))


class ProviderSetupRoute:
    def __init__(self, owner, authenticated_context):
        if not callable(authenticated_context) or inspect.iscoroutinefunction(authenticated_context):
            raise ProviderSetupUnavailable()
        self.authenticated_context = authenticated_context
        self._loop = asyncio.get_running_loop()
        self._owners = {}
        self._closed = False
        self._shutdown_task = None
        self.add(owner)

    @property
    def retired(self):
        return (self._closed and self._shutdown_task is not None and self._shutdown_task.done()
                and not self._shutdown_task.cancelled() and self._shutdown_task.exception() is None
                and all(owner.retired for owner in self._owners.values()))

    def add(self, owner):
        if (self._closed or self._loop is not asyncio.get_running_loop()
                or type(owner) is not ForgeProviderSetupSession or owner._loop is not None or owner._closed):
            raise ProviderSetupUnavailable()
        context = owner._login
        if any(context is other._login or
               (context.principal, context.login_scope_id) == (other._login.principal, other._login.login_scope_id)
               for other in self._owners.values()):
            raise ProviderSetupUnavailable()
        self._owners[id(context)] = owner

    async def handle(self, request, action="status"):
        try:
            if self._closed or self._loop is not asyncio.get_running_loop():
                raise ProviderSetupUnavailable()
            context = self.authenticated_context(request)
            if inspect.isawaitable(context):
                if inspect.iscoroutine(context):
                    context.close()
                raise ProviderSetupUnavailable()
            owner = self._owners.get(id(context))
            if owner is None or owner._login is not context:
                raise ProviderSetupUnavailable()
            owner._admit(context)
            origins = request.headers.getlist("origin")
            if origins != [context.origin] and not (action == "status" and not origins):
                raise ProviderSetupUnavailable()
        except Exception:
            return unavailable(403)
        if request.scope.get("query_string"):
            return unavailable(400)
        if action == "status":
            try:
                return response(owner.snapshot(context))
            except ProviderSetupUnavailable:
                return unavailable(403)
        if request.headers.getlist("content-type") != ["application/json"]:
            return unavailable(400)
        try:
            body = bytearray()
            async for part in request.stream():
                if len(body) + len(part) > 65536:
                    return unavailable(413)
                body.extend(part)
            value = json.loads(body, object_pairs_hook=_object)
            if type(value) is not dict:
                return unavailable(400)
            # Authenticate again after the asynchronous body read. Request
            # lifetime does not extend the retained original login deadline.
            owner._admit(context)
            if action == "begin" and value == {}:
                return response(await owner.begin(context))
            if action == "cancel" and value == {}:
                return response(await owner.cancel(context))
            if action == "answer" and set(value) == {"prompt_id", "answer"}:
                snapshot = owner.snapshot(context)
                if (snapshot.get("prompt_id") != value["prompt_id"] or
                        not offered_answer(snapshot.get("prompt", {}), value["answer"])):
                    return unavailable(409)
                return response(await owner.answer(context, value["prompt_id"], value["answer"]))
            return unavailable(400)
        except ProviderSetupUnavailable:
            return unavailable(409)
        except Exception:
            # Never echo credential answers, callback URLs or arbitrary errors.
            return unavailable(400)

    async def shutdown(self):
        if self._loop is not asyncio.get_running_loop():
            raise ProviderSetupUnavailable()
        self._closed = True
        for owner in self._owners.values():
            owner.revoke()
        if self._shutdown_task is None:
            self._shutdown_task = _retain(asyncio.create_task(self._shutdown()))
        interrupted = False
        while not self._shutdown_task.done():
            try:
                await asyncio.shield(self._shutdown_task)
            except asyncio.CancelledError:
                interrupted = True
            except Exception:
                break
        self._shutdown_task.result()
        if interrupted:
            raise asyncio.CancelledError("Provider owners joined.")

    async def _shutdown(self):
        results = await asyncio.gather(*(owner.close() for owner in self._owners.values()), return_exceptions=True)
        if any(isinstance(result, BaseException) or result is not True for result in results):
            raise ProviderSetupUnavailable()
