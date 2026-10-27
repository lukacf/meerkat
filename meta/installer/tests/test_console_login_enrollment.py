"""Real ASGI login custody with inert enrollment owners, not native proof."""
import asyncio
from contextlib import asynccontextmanager
from types import SimpleNamespace
import sys

import httpx
import jwt
import pytest

from launchkit.templates_v2 import console_auth
from launchkit.templates_v2.console_auth import AUDIENCE, ISSUER, ConsoleFront
from test_console_auth import login


class EnrollmentOwner:
    """Hold one original acknowledgement and its retirement under test control."""
    def __init__(self, context):
        self.context = context
        self.issuer = ISSUER
        self.subject = context.principal
        self.login_scope_id = context.login_scope_id
        self.expires_at_ms = context.expires_at_ms
        self.source_current = True
        self.revoked = False
        self.fail_start = self.fail_ack = False
        self.start_calls = self.ready_calls = self.close_calls = 0
        self.retire_reasons = []
        self.entered = asyncio.Event()
        self.release = asyncio.Event()
        self.ready_entered = asyncio.Event()
        self.revoke_called = asyncio.Event()
        self.retire_entered = asyncio.Event()
        self.retire_finished = asyncio.Event()
        self.original = object()
        self.task = self.retire_task = None

    def is_current(self):
        return not self.revoked and self.source_current and self.context.is_current()

    def start(self):
        self.start_calls += 1
        assert self.start_calls == 1
        if self.fail_start:
            raise RuntimeError("SYNTHETIC_PRIVATE_START_DIAGNOSTIC")
        self.task = asyncio.create_task(self._enroll())
        self.task.add_done_callback(lambda task: task.exception() if not task.cancelled() else None)

    async def _enroll(self):
        self.entered.set()
        await self.release.wait()
        if self.fail_ack:
            raise RuntimeError("SYNTHETIC_PRIVATE_ACK_DIAGNOSTIC")
        return self.original

    async def ready(self):
        self.ready_calls += 1
        self.ready_entered.set()
        # The fake mimics retained work. It deliberately does not recheck
        # currentness here so the HTTP boundary must fence a late success.
        return await asyncio.shield(self.task)

    def revoke(self):
        self.revoked = True
        self.revoke_called.set()

    async def _retire(self):
        self.retire_entered.set()
        if self.task is not None:
            await asyncio.gather(self.task, return_exceptions=True)
        self.retire_finished.set()

    async def retire(self, reason):
        self.retire_reasons.append(reason)
        if self.retire_task is None:
            self.retire_task = asyncio.create_task(self._retire())
        await asyncio.shield(self.retire_task)

    async def close(self):
        self.close_calls += 1
        self.revoke()
        await self.retire("owner_closing")


async def entered(event):
    await asyncio.wait_for(event.wait(), 30)


@asynccontextmanager
async def boundary(tmp_path, monkeypatch, configure=None, factory_failure=False,
                   review_factory=None, upstream=None):
    owners, contexts, forwarded, minted, tasks = [], [], [], [], []
    original_token = console_auth._token

    def token(*args, **kwargs):
        minted.append((args, kwargs))
        return original_token(*args, **kwargs)

    def factory(context):
        contexts.append(context)
        if factory_failure:
            raise RuntimeError("SYNTHETIC_PRIVATE_FACTORY_DIAGNOSTIC")
        owner = EnrollmentOwner(context)
        owners.append(owner)
        if configure:
            configure(owner)
        return owner

    monkeypatch.setattr(console_auth, "_token", token)
    def native(request):
        forwarded.append(request)
        return upstream(request) if upstream else httpx.Response(200, content=b"synthetic native response")
    front = ConsoleFront(tmp_path, 8855, "local-user", "local-password-123",
        console_login_factory=factory, controller_review_factory=review_factory,
        transport=httpx.MockTransport(native))
    app = front.application("http://127.0.0.1:61347")
    try:
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=app),
                                     base_url=front.origin) as client:
            yield client, front, owners, contexts, forwarded, minted, tasks
    finally:
        for owner in owners:
            owner.release.set()
        for task in tasks:
            if not task.done():
                task.cancel()
        await asyncio.gather(*tasks, return_exceptions=True)
        await front.close()


async def test_password_login_eagerly_enrolls_once_and_gates_both_native_surfaces(tmp_path, monkeypatch):
    async with boundary(tmp_path, monkeypatch) as (client, front, owners, contexts, forwarded, minted, tasks):
        await login(client, front)
        actual = front._logins[client.cookies[front.cookie]]
        assert len(owners) == len(contexts) == 1
        owner, context = owners[0], contexts[0]
        assert context.principal == front.principal == "local-user"
        assert context.login_scope_id == actual.login_scope_id
        assert context.expires_at_ms == int(actual.expires * 1000)
        assert context.origin == front.origin and context.is_current() is True
        await entered(owner.entered)
        assert owner.start_calls == 1 and forwarded == minted == []
        abandoned = asyncio.create_task(client.get("/console"))
        tasks.append(abandoned)
        await entered(owner.ready_entered)
        surviving = asyncio.create_task(client.get("/host/toolkit/setup/data",
            headers={"Authorization": "Bearer browser-bearer-canary"}))
        tasks.append(surviving)
        await asyncio.sleep(0)
        assert not abandoned.done() and not surviving.done()
        assert forwarded == minted == []
        abandoned.cancel()
        with pytest.raises(asyncio.CancelledError):
            await abandoned
        assert not owner.task.done() and not owner.task.cancelled()
        assert context.is_current() is True and len(owners) == 1
        owner.release.set()
        assert (await surviving).status_code == 200
        assert (await client.get("/console")).status_code == 200
        assert owner.task.result() is owner.original and owner.start_calls == 1
        assert len(forwarded) == len(minted) == 2
        assert len(owners) == len(contexts) == 1
        assert len(front.sessions) == len(front._logins) == 1
        for request in forwarded:
            token = request.headers["authorization"].removeprefix("Bearer ")
            assert token != "browser-bearer-canary"
            claims = jwt.decode(token, front.key, algorithms=["HS256"], issuer=ISSUER, audience=AUDIENCE)
            assert claims["sub"] == context.principal and claims["sid"] == actual.login_scope_id
            assert request.headers.get("cookie") is None


async def test_same_principal_second_password_login_keeps_distinct_actual_enrollment(tmp_path, monkeypatch):
    async with boundary(tmp_path, monkeypatch) as (client, front, owners, contexts, forwarded, minted, tasks):
        await login(client, front)
        first = front._logins[client.cookies[front.cookie]]
        async with httpx.AsyncClient(transport=client._transport,
                                     base_url=front.origin) as second:
            await login(second, front)
            current = front._logins[second.cookies[front.cookie]]
            assert current is not first and current.login_scope_id != first.login_scope_id
            assert len(contexts) == len(owners) == len(front.sessions) == 2
            assert {context.login_scope_id for context in contexts} == {first.login_scope_id, current.login_scope_id}
            assert all(context.principal == "local-user" for context in contexts)
            assert all(owner.start_calls == 1 for owner in owners)
            owners[1].release.set()
            assert (await second.get("/console")).status_code == 200
            assert not owners[0].task.done() and owners[0].context.is_current() is True
            assert len(forwarded) == len(minted) == 1
            claims = jwt.decode(forwarded[0].headers["authorization"].removeprefix("Bearer "),
                front.key, algorithms=["HS256"], issuer=ISSUER, audience=AUDIENCE)
            assert claims["sid"] == current.login_scope_id


@pytest.mark.parametrize("retirement", ["logout", "expiry"])
async def test_pending_enrollment_retirement_fences_and_joins_original_owner(tmp_path, monkeypatch, retirement):
    clock = {"now": console_auth.time.time()}
    monkeypatch.setattr(console_auth.time, "time", lambda: clock["now"])
    async with boundary(tmp_path, monkeypatch) as (client, front, owners, contexts, forwarded, minted, tasks):
        await login(client, front)
        session = client.cookies[front.cookie]
        actual, owner = front._logins[session], owners[0]
        request = asyncio.create_task(client.get("/console"))
        tasks.append(request)
        await entered(owner.ready_entered)
        if retirement == "logout":
            retiring = asyncio.create_task(client.post("/logout", headers={"Origin": front.origin}))
            tasks.append(retiring)
        else:
            clock["now"] = actual.expires + 1
            assert (await client.get("/console/experience")).status_code == 401
            retiring = actual.retirement_task
        await entered(owner.revoke_called)
        await entered(owner.retire_entered)
        await asyncio.sleep(0)
        assert session not in front.sessions and actual.retired and not contexts[0].is_current()
        assert not owner.task.done() and not owner.retire_finished.is_set()
        assert not retiring.done() and forwarded == minted == []
        owner.release.set()
        assert (await request).status_code == 401
        if retirement == "logout":
            assert (await retiring).status_code == 303
        else:
            await retiring
        assert owner.task.result() is owner.original and owner.retire_finished.is_set()
        assert owner.retire_reasons[0] == ("logout" if retirement == "logout" else "login_expired")
        assert len(owners) == 1 and forwarded == minted == []


async def test_close_joins_pending_original_enrollment_even_when_close_caller_is_cancelled(tmp_path, monkeypatch):
    async with boundary(tmp_path, monkeypatch) as (client, front, owners, contexts, forwarded, minted, tasks):
        await login(client, front)
        owner = owners[0]
        await entered(owner.entered)
        closing = asyncio.create_task(front.close())
        tasks.append(closing)
        await entered(owner.revoke_called)
        await entered(owner.retire_entered)
        closing.cancel()
        await asyncio.sleep(0)
        assert not closing.done() and not owner.task.done() and not owner.retire_finished.is_set()
        assert not contexts[0].is_current() and forwarded == minted == []
        owner.release.set()
        with pytest.raises(asyncio.CancelledError):
            await closing
        assert owner.task.result() is owner.original and owner.retire_finished.is_set()
        assert owner.close_calls == 1 and front.client.is_closed


async def test_enrollment_rechecks_application_currentness_after_original_ack(tmp_path, monkeypatch):
    async with boundary(tmp_path, monkeypatch) as (client, front, owners, contexts, forwarded, minted, tasks):
        await login(client, front)
        owner = owners[0]
        request = asyncio.create_task(client.get("/host/toolkit/setup/data"))
        tasks.append(request)
        await entered(owner.ready_entered)
        owner.source_current = False
        assert contexts[0].is_current() is True and owner.is_current() is False
        owner.release.set()
        response = await request
        assert response.status_code == 503 and forwarded == minted == []
        assert (await client.get("/console")).status_code == 503
        assert len(owners) == 1 and owner.start_calls == 1


@pytest.mark.parametrize("field", ["issuer", "subject", "login_scope_id", "expires_at_ms"])
async def test_enrollment_rejects_mismatched_owner_binding_without_reenrollment(tmp_path, monkeypatch, field):
    def mismatch(owner):
        setattr(owner, field, owner.expires_at_ms + 1 if field == "expires_at_ms" else "wrong-binding")
    async with boundary(tmp_path, monkeypatch, configure=mismatch) as (client, front, owners, contexts, forwarded, minted, tasks):
        await login(client, front)
        for path in ("/console", "/host/toolkit/setup/data"):
            assert (await client.get(path)).status_code == 503
        assert len(owners) == len(contexts) == 1 and owners[0].start_calls == 0
        assert forwarded == minted == []
    assert owners[0].revoked and owners[0].retire_finished.is_set()
    assert owners[0].close_calls == 1


@pytest.mark.parametrize("failure", ["factory", "start", "ack"])
async def test_failed_enrollment_never_mints_forwards_or_retries_the_factory(tmp_path, monkeypatch, failure):
    def configure(owner):
        owner.fail_start = failure == "start"
        owner.fail_ack = failure == "ack"
        owner.release.set()
    async with boundary(tmp_path, monkeypatch, configure=configure,
                        factory_failure=failure == "factory") as (client, front, owners, contexts, forwarded, minted, tasks):
        await login(client, front)
        for path in ("/console", "/host/toolkit/setup/data"):
            response = await client.get(path)
            assert response.status_code == 503
            assert "SYNTHETIC_PRIVATE" not in response.text
        assert len(contexts) == 1 and len(owners) == (0 if failure == "factory" else 1)
        assert forwarded == minted == []
        if owners:
            assert owners[0].start_calls == 1
    if owners:
        assert owners[0].revoked and owners[0].retire_finished.is_set()
        assert owners[0].close_calls == 1


async def test_host_health_stays_outside_login_enrollment_and_has_no_sid(tmp_path, monkeypatch):
    async with boundary(tmp_path, monkeypatch) as (client, front, owners, contexts, forwarded, minted, tasks):
        credential = console_auth._token(front.key, front.principal, scope="host-health")
        minted.clear()
        incoming = jwt.decode(credential, front.key, algorithms=["HS256"], issuer=ISSUER,
                              audience=console_auth.HEALTH_AUDIENCE)
        assert "sid" not in incoming
        response = await client.get("/console/experience", headers={"Authorization": "Bearer " + credential})
        assert response.status_code == 200
        assert owners == contexts == [] and front.sessions == front._logins == {}
        assert len(forwarded) == len(minted) == 1
        upstream = jwt.decode(forwarded[0].headers["authorization"].removeprefix("Bearer "), front.key,
            algorithms=["HS256"], issuer=ISSUER, audience=AUDIENCE)
        assert upstream["sub"] == front.principal and "sid" not in upstream


@pytest.mark.parametrize("retirement", ["logout", "replacement"])
async def test_frontend_read_never_discloses_awaited_result_to_retired_or_replaced_login(tmp_path, monkeypatch, retirement):
    async with boundary(tmp_path, monkeypatch) as (client, front, owners, contexts, forwarded, minted, tasks):
        front.frontend_host = lambda: {"application_id": "synthetic-app", "source_digest": "a" * 64}
        read_entered, read_release = asyncio.Event(), asyncio.Event()
        commands = []
        async def read(command):
            commands.append(command)
            read_entered.set()
            await read_release.wait()
            return {"status": "inactive", "private_owner_observation": "SYNTHETIC_PRIVATE_LATE_READ"}
        front.frontend_management = read
        await login(client, front)
        owner, session = owners[0], client.cookies[front.cookie]
        actual = front._logins[session]
        owner.release.set()
        request = asyncio.create_task(client.get("/console/forge/frontends"))
        tasks.append(request)
        try:
            await entered(read_entered)
            if retirement == "logout":
                assert (await client.post("/logout", headers={"Origin": front.origin})).status_code == 303
                assert actual.retired
            else:
                front._logins[session] = console_auth._ConsoleLogin(session, actual.expires)
                assert front.sessions[session] == actual.expires
            read_release.set()
            response = await request
            assert response.status_code == 401 and "SYNTHETIC_PRIVATE_LATE_READ" not in response.text
            assert len(commands) == 1 and forwarded == minted == []
            assert commands[0].principal == front.principal
        finally:
            read_release.set()
            # A replacement is a custody fault schedule, not a second login.
            # Restore the original owner so the fixture joins its real cleanup.
            if retirement == "replacement":
                front._logins[session] = actual


@pytest.mark.parametrize("retire_before_ack", [False, True], ids=["current", "retired"])
async def test_original_review_selection_waits_for_same_login_ack_and_never_enters_after_retirement(tmp_path, monkeypatch, retire_before_ack):
    selected, reviews = [], []
    class Review:
        def __init__(self, context):
            self.login_scope_id, self.expires_at_ms = context.login_scope_id, context.expires_at_ms
            self.origin, self.is_current = context.origin, context.is_current
            self.started = 0
        def start(self):
            self.started += 1
        def revoke(self):
            pass
        async def retire(self, reason):
            pass
        async def close(self):
            pass
    monkeypatch.setitem(sys.modules, console_auth.__package__ + ".native_controller_review",
                        SimpleNamespace(NativeControllerReview=Review))
    def review_factory(context):
        selected.append(context)
        review = Review(context)
        reviews.append(review)
        return review
    async with boundary(tmp_path, monkeypatch, review_factory=review_factory) as (client, front, owners, contexts, forwarded, minted, tasks):
        await login(client, front)
        actual, owner = front._logins[client.cookies[front.cookie]], owners[0]
        await entered(owner.ready_entered)
        assert selected == reviews == [] and not actual.controller_review_start.done()
        assert forwarded == minted == []
        if retire_before_ack:
            retiring = asyncio.create_task(client.post("/logout", headers={"Origin": front.origin}))
            tasks.append(retiring)
            await entered(owner.revoke_called)
            assert not retiring.done() and not owner.task.done()
        owner.release.set()
        await actual.controller_review_start
        if retire_before_ack:
            assert (await retiring).status_code == 303
            assert selected == reviews == []
        else:
            assert len(selected) == len(reviews) == reviews[0].started == 1
            assert selected[0].login_scope_id == actual.login_scope_id
            assert selected[0].expires_at_ms == int(actual.expires * 1000)
            assert selected[0].is_current() is True
        assert owner.task.result() is owner.original and owner.start_calls == 1
        assert forwarded == minted == []


async def test_forwarded_finite_response_hides_private_body_and_etag_after_source_currentness_changes(tmp_path, monkeypatch):
    body_entered, body_release = asyncio.Event(), asyncio.Event()
    class HeldBody(httpx.AsyncByteStream):
        async def __aiter__(self):
            body_entered.set()
            await body_release.wait()
            yield b"SYNTHETIC_PRIVATE_NATIVE_BODY"
    def upstream(request):
        return httpx.Response(200, stream=HeldBody(), headers={"ETag": "private-native-etag-canary"})
    async with boundary(tmp_path, monkeypatch, upstream=upstream) as (client, front, owners, contexts, forwarded, minted, tasks):
        await login(client, front)
        owner = owners[0]
        owner.release.set()
        request = asyncio.create_task(client.get("/console/identities"))
        tasks.append(request)
        try:
            await entered(body_entered)
            assert len(forwarded) == len(minted) == 1 and not request.done()
            owner.source_current = False
            assert contexts[0].is_current() is True and owner.is_current() is False
            body_release.set()
            response = await request
            assert response.status_code == 401
            assert "SYNTHETIC_PRIVATE_NATIVE_BODY" not in response.text and "etag" not in response.headers
            assert len(owners) == 1 and owner.start_calls == 1
        finally:
            body_release.set()
