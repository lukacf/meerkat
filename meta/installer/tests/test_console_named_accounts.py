"""Named Console accounts on the Forge front, against a console-shaped native.

The native fixture verifies each bearer the front mints with the front's own
`auth_config`, as the console verifies a shared-secret issuer: signature,
issuer, audience, expiry and the email allowlist. A configuration that
declares `accounts: "personal"` names a person per token; one that does not
names a shared account. It answers `meta/console/whoami` and
`meta/console/sign_out` with the console's documented shapes and keeps the
sign-out fence of a login (`sid`) as the console does. It is an inert wire
peer: no native process, session or provider starts.
"""
import asyncio
from contextlib import asynccontextmanager
import hashlib
import json
import os
import stat
import sys
import time
from types import SimpleNamespace
import uuid

import httpx
import jwt
import pytest

from launchkit.templates_v2 import console_auth
from launchkit.templates_v2.console_auth import ConsoleFront
from test_console_auth import (connect_ready, connect_view, finish_stream_tasks,
    managed_connect_factory)

OPERATOR, OPERATOR_PASSWORD = "operator", "operator-password-1"
ALICE, ALICE_PASSWORD = "alice@example.test", "alice-password-123"
BOB, BOB_PASSWORD = "bob@example.test", "bob-password-1234"
CAROL, CAROL_PASSWORD = "carol@example.test", "carol-password-12"
NATIVE = "http://127.0.0.1:61347"


class ConsoleNative:
    """Console routes the front reaches, with the console's token rules."""

    def __init__(self, *, admins=(), sign_out=None):
        self.auth = None
        self.admins = set(admins)
        self.requests, self.sign_outs = [], []
        self.fences = {}
        self.sign_out = sign_out

    def transport(self):
        return httpx.MockTransport(self.handle)

    def claims(self, request):
        header = request.headers.get("authorization", "")
        if not header.startswith("Bearer "):
            return None
        try:
            claims = jwt.decode(header[7:], self.auth["shared_secret"], algorithms=["HS256"],
                issuer=self.auth["issuer"], audience=self.auth["audience"],
                options={"require": ["exp", "iat", "sub"]})
        except jwt.PyJWTError:
            return None
        if claims.get("email") not in self.auth["email_allowlist"]:
            return None
        fence = self.fences.get((claims["sub"], claims.get("sid")))
        if fence is not None and claims["iat"] < fence + 30:
            return None
        return claims

    def handle(self, request):
        self.requests.append(request)
        claims = self.claims(request)
        if claims is None:
            return httpx.Response(401, json={"error": {"code": -32600, "data": {"kind": "unauthenticated"}}})
        if request.url.path == "/console/rpc":
            body = json.loads(request.content)
            method, identity = body.get("method"), body.get("id")
            if method == "meta/console/whoami":
                return httpx.Response(200, json={"jsonrpc": "2.0", "id": identity, "result": self.whoami(claims)})
            if method == "meta/console/sign_out":
                self.sign_outs.append(claims)
                if self.sign_out is not None:
                    return self.sign_out(identity)
                not_before = max(int(time.time()), claims["iat"])
                session = claims.get("sid") is not None
                self.fences[(claims["sub"], claims.get("sid"))] = not_before
                return httpx.Response(200, json={"jsonrpc": "2.0", "id": identity, "result": {
                    "login": "session" if session else "token",
                    "accepts_tokens_issued_from": not_before + 30 if session else None}})
        return httpx.Response(200, content=b"native-console")

    def whoami(self, claims):
        person = self.auth.get("accounts") == "personal"
        caller = (["human", claims["iss"], claims["sub"]] if person
                  else ["principal", claims["email"], "jwt", claims["iss"]])
        return {"actor": {"kind": "person" if person else "token_principal",
                          "principal": claims["email"],
                          "assurance": "person_authenticated" if person else None},
                "posture": "enforced", "login_expires_at_ms": (claims["exp"] + 30) * 1000,
                "can_administer": claims["email"] in self.admins, "approver_ids": [],
                "caller_scope": "console-caller:v1:" + hashlib.sha256(json.dumps(caller).encode()).hexdigest()}


def named_accounts(*names):
    passwords = {ALICE: ALICE_PASSWORD, BOB: BOB_PASSWORD, CAROL: CAROL_PASSWORD}
    return {name: console_auth.hash_console_password(passwords[name]) for name in names}


def front(tmp_path, native, *, accounts=(ALICE, BOB, CAROL), shared=False, **options):
    selected = None if shared else named_accounts(*accounts)
    console = ConsoleFront(tmp_path, 8855, OPERATOR, OPERATOR_PASSWORD, accounts=selected,
        runtime_namespace="meta", transport=native.transport(), **options)
    native.auth = console.native_auth
    return console


def shared_front(tmp_path, native, **options):
    """The single shared account, constructed as every earlier front was."""
    console = ConsoleFront(tmp_path, 8855, OPERATOR, OPERATOR_PASSWORD, runtime_namespace="meta",
        transport=native.transport(), **options)
    native.auth = console.native_auth
    return console


@asynccontextmanager
async def browser(app, console, address="127.0.0.1"):
    transport = httpx.ASGITransport(app=app, client=(address, 41000))
    async with httpx.AsyncClient(transport=transport, base_url=console.origin) as client:
        yield client


async def sign_in(client, console, account, password, *, status=303):
    form = await client.get("/login")
    assert form.status_code == 200, form.text
    nonce = client.cookies[console.cookie + "_login"]
    data = {"nonce": nonce, "password": password}
    if account is not None:
        data["account"] = account
    response = await client.post("/login", data=data, headers={"Origin": console.origin})
    assert response.status_code == status, response.text
    return response


async def whoami(client, console):
    response = await client.post("/console/rpc", json={"jsonrpc": "2.0", "id": "who",
        "method": "meta/console/whoami", "params": {}}, headers={"Origin": console.origin})
    assert response.status_code == 200, response.text
    return response.json()["result"]


def login_of(client, console):
    return console._logins[client.cookies[console.cookie]]


def bearer_claims(request, console):
    return jwt.decode(request.headers["authorization"].removeprefix("Bearer "), console.key,
        algorithms=["HS256"], issuer=console_auth.ISSUER, audience=console_auth.AUDIENCE)


# Row 1: two named accounts reach the console as two people.
async def test_two_named_accounts_reach_the_console_as_two_people(tmp_path):
    native = ConsoleNative()
    console = front(tmp_path, native)
    app = console.application(NATIVE)
    try:
        async with browser(app, console) as alice, browser(app, console) as bob, \
                browser(app, console) as operator, browser(app, console) as stranger:
            await sign_in(alice, console, ALICE, ALICE_PASSWORD)
            await sign_in(bob, console, BOB, BOB_PASSWORD)
            # An empty account is the installation's own account.
            await sign_in(operator, console, "", OPERATOR_PASSWORD)
            seen = [await whoami(client, console) for client in (alice, bob, operator)]
            assert [row["actor"]["principal"] for row in seen] == [ALICE, BOB, OPERATOR]
            assert all(row["actor"]["kind"] == "person" for row in seen)
            assert len({row["caller_scope"] for row in seen}) == 3
            # Each login's tokens carry its own account and its own login.
            for client, account in ((alice, ALICE), (bob, BOB), (operator, OPERATOR)):
                assert login_of(client, console).principal == account
            minted = {(claims["sub"], claims["sid"]) for claims in
                      (bearer_claims(request, console) for request in native.requests)}
            assert {sub for sub, _ in minted} == {ALICE, BOB, OPERATOR} and len(minted) == 3
            # A password opens only its own account; an unknown account opens nothing.
            await sign_in(stranger, console, ALICE, BOB_PASSWORD, status=401)
            await sign_in(stranger, console, "mallory@example.test", ALICE_PASSWORD, status=401)
            await sign_in(stranger, console, BOB, OPERATOR_PASSWORD, status=401)
            assert console.cookie not in stranger.cookies
    finally:
        await console.close()


def test_named_accounts_declare_personal_tokens_and_list_every_account(tmp_path):
    named = ConsoleFront(tmp_path, 8855, OPERATOR, OPERATOR_PASSWORD,
        accounts=named_accounts(ALICE, BOB), runtime_namespace="meta")
    assert named.native_auth == {"provider": "jwt", "shared_secret": named.key,
        "issuer": console_auth.ISSUER, "audience": console_auth.AUDIENCE,
        "email_allowlist": [OPERATOR, ALICE, BOB], "accounts": "personal"}
    # One shared account keeps the shared declaration (none) it always had.
    shared = ConsoleFront(tmp_path, 8856, OPERATOR, OPERATOR_PASSWORD, runtime_namespace="meta")
    assert shared.native_auth == {"provider": "jwt", "shared_secret": shared.key,
        "issuer": console_auth.ISSUER, "audience": console_auth.AUDIENCE,
        "email_allowlist": [OPERATOR]}
    # The installation's own account is not also a named account.
    with pytest.raises(ValueError, match="installation's own account"):
        ConsoleFront(tmp_path, 8857, OPERATOR, OPERATOR_PASSWORD,
            accounts={OPERATOR: console_auth.hash_console_password(OPERATOR_PASSWORD)},
            runtime_namespace="meta")
    # Named accounts need the Meta console, whose tokens can name a person.
    with pytest.raises(ValueError, match="Meta"):
        ConsoleFront(tmp_path, 8858, OPERATOR, OPERATOR_PASSWORD, accounts={})
    for invalid in ({"not an account": named_accounts(ALICE)[ALICE]}, {ALICE: "plaintext-password"}):
        with pytest.raises(ValueError):
            ConsoleFront(tmp_path, 8859, OPERATOR, OPERATOR_PASSWORD, accounts=invalid,
                runtime_namespace="meta")


def test_named_account_source_needs_the_installation_account_fields(tmp_path):
    from launchkit.templates_v2.console_configuration import (CONSOLE_FIELDS, console_authentication,
        console_configuration)
    fields = [dict(field) for field in CONSOLE_FIELDS]
    assert console_authentication({"authentication": "local-accounts"}, fields) == "local-accounts"
    assert console_authentication({"authentication": "local-password"}, fields) == "local-password"
    with pytest.raises(ValueError, match="CONSOLE_LOGIN_PASSWORD"):
        console_authentication({"authentication": "local-accounts"}, fields[:1])
    (tmp_path / "application").mkdir()
    (tmp_path / "application/console.json").write_text('{"authentication":"local-accounts"}')
    assert console_configuration(tmp_path, fields) is True


# Row 1c: the deployment computer's setup keeps named accounts as hashes only.
def named_console_source(root):
    from launchkit.templates_v2.console_configuration import CONSOLE_FIELDS
    application = root / "application"
    application.mkdir(exist_ok=True)
    (application / "__init__.py").write_text("def build(context):\n    return object()\n")
    (application / "console.json").write_text('{"authentication":"local-accounts"}')
    (application / "configuration.json").write_text(json.dumps({"fields": list(CONSOLE_FIELDS)}))


def test_setup_keeps_named_accounts_as_hashes_only(generated_setup, monkeypatch, capsys):
    module, root = generated_setup
    named_console_source(root)
    env = root / ".env"
    env.write_text(f'CONSOLE_USER_ID="{OPERATOR}"\nCONSOLE_LOGIN_PASSWORD="{OPERATOR_PASSWORD}"\n')
    env.chmod(0o600)
    names = iter([ALICE, "not an account!", OPERATOR, BOB, ""])
    passwords = iter([ALICE_PASSWORD, ALICE_PASSWORD, "short", "short", BOB_PASSWORD, "different-password-9",
                      BOB_PASSWORD, BOB_PASSWORD])

    def answer(prompt):
        if "account" in prompt.lower():
            return next(names)
        return ""

    def secret(prompt):
        if "Console password" in prompt:
            return ""
        return next(passwords)
    monkeypatch.setattr("builtins.input", answer)
    monkeypatch.setattr(module.getpass, "getpass", secret)
    assert module.main() == 0
    path = root / ".mobkit/console-accounts.json"
    info = path.lstat()
    assert stat.S_ISREG(info.st_mode) and stat.S_IMODE(info.st_mode) == 0o600
    content = path.read_bytes()
    for password in (ALICE_PASSWORD, BOB_PASSWORD, OPERATOR_PASSWORD):
        assert password.encode() not in content
    accounts = console_auth.load_console_accounts(root, OPERATOR)
    assert set(accounts) == {ALICE, BOB}
    assert console_auth.verify_console_password(accounts[ALICE], ALICE_PASSWORD)
    assert not console_auth.verify_console_password(accounts[ALICE], BOB_PASSWORD)
    output = capsys.readouterr().out
    assert ALICE_PASSWORD not in output and BOB_PASSWORD not in output
    # A second run removes one account and keeps the other's hash unchanged.
    names = iter([":remove " + ALICE, ""])
    monkeypatch.setattr("builtins.input", answer)
    assert module.main() == 0
    remaining = console_auth.load_console_accounts(root, OPERATOR)
    assert remaining == {BOB: accounts[BOB]}


@pytest.fixture
def generated_setup(tmp_path, monkeypatch):
    import importlib.util
    from launchkit.blueprint import starter_blueprint
    from launchkit.compiler import generate_files
    for name, content in generate_files(starter_blueprint()).items():
        target = tmp_path / name
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(content)
    for name in ("mcp_connections", "application_configuration", "console_configuration", "connectors",
                 "console_auth", "native_definition_continuity", "setup"):
        spec = importlib.util.spec_from_file_location(name, tmp_path / (name + ".py"))
        module = importlib.util.module_from_spec(spec)
        monkeypatch.setitem(sys.modules, name, module)
        spec.loader.exec_module(module)
    module = sys.modules["setup"]
    deployment = json.loads((tmp_path / "deployment.json").read_text())
    deployment["environment"] = []
    deployment["knowledge"]["mode"] = "none"
    (tmp_path / "deployment.json").write_text(json.dumps(deployment))
    monkeypatch.setattr(module.sys, "stdin", SimpleNamespace(isatty=lambda: True))
    return module, tmp_path


# Row 3: logout ends the login in the console, not only in the front.
async def test_logout_signs_the_login_out_of_the_console(tmp_path):
    native = ConsoleNative()
    console = shared_front(tmp_path, native)
    app = console.application(NATIVE)
    try:
        async with browser(app, console) as leaving, browser(app, console) as staying:
            await sign_in(leaving, console, None, OPERATOR_PASSWORD)
            await sign_in(staying, console, None, OPERATOR_PASSWORD)
            login = login_of(leaving, console)
            assert (await leaving.get("/console")).status_code == 200
            held = native.requests[-1].headers["authorization"]
            assert bearer_claims(native.requests[-1], console)["sid"] == login.login_scope_id
            response = await leaving.post("/logout", headers={"Origin": console.origin})
            assert response.status_code == 303 and response.headers["location"] == "/login"
            assert len(native.sign_outs) == 1
            assert native.sign_outs[0]["sid"] == login.login_scope_id
            assert native.sign_outs[0]["sub"] == OPERATOR
            # The token the console already held for that login is refused now,
            # and the front no longer mints for it.
            replay = httpx.Request("GET", NATIVE + "/console/experience", headers={"Authorization": held})
            assert native.handle(replay).status_code == 401
            before = len(native.requests)
            assert (await leaving.get("/console/experience")).status_code == 401
            assert len(native.requests) == before
            # The same person's other login keeps working.
            assert (await staying.get("/console/experience")).status_code == 200
            assert bearer_claims(native.requests[-1], console)["sid"] != login.login_scope_id
    finally:
        await console.close()


@pytest.mark.parametrize("answer,status", [("refused", 503), ("unavailable", 503), ("unauthenticated", 503),
                                           ("not-served", 303)])
async def test_logout_reports_an_unconfirmed_console_sign_out(tmp_path, answer, status):
    def sign_out(identity):
        if answer == "unavailable":
            raise httpx.ConnectError("the console is unavailable")
        if answer == "unauthenticated":
            # A console that did not verify the token fenced nothing.
            return httpx.Response(401, json={"jsonrpc": "2.0", "id": identity, "error": {
                "code": -32600, "message": "unauthorized", "data": {"kind": "unauthenticated"}}})
        code = -32603 if answer == "refused" else -32601
        return httpx.Response(200, json={"jsonrpc": "2.0", "id": identity,
            "error": {"code": code, "message": "synthetic console answer"}})
    native = ConsoleNative(sign_out=sign_out)
    console = shared_front(tmp_path, native)
    app = console.application(NATIVE)
    try:
        async with browser(app, console) as client:
            await sign_in(client, console, None, OPERATOR_PASSWORD)
            assert console.cookie in client.cookies
            response = await client.post("/logout", headers={"Origin": console.origin})
            assert response.status_code == status, response.text
            assert len(native.sign_outs) == 1
            if status == 503:
                assert "did not confirm" in response.text and 'href="/login"' in response.text
            # The front's own login has ended either way, and its cookie is gone.
            assert console.sessions == {} and console.cookie not in client.cookies
            assert (await client.get("/console/experience")).status_code == 401
            # The person can sign in again at once.
            await sign_in(client, console, None, OPERATOR_PASSWORD)
    finally:
        await console.close()


async def test_logout_on_the_predecessor_runtime_stays_front_only(tmp_path):
    native = ConsoleNative()
    console = ConsoleFront(tmp_path, 8855, OPERATOR, OPERATOR_PASSWORD, transport=native.transport())
    native.auth = console.native_auth
    app = console.application(NATIVE)
    try:
        async with browser(app, console) as client:
            await sign_in(client, console, None, OPERATOR_PASSWORD)
            assert (await client.post("/logout", headers={"Origin": console.origin})).status_code == 303
            assert native.sign_outs == []
    finally:
        await console.close()


# Row 4: one account's failed sign-ins do not lock out another.
@pytest.fixture
def clock(monkeypatch):
    now = {"now": float(int(time.time()))}
    monkeypatch.setattr(console_auth, "time", SimpleNamespace(time=lambda: now["now"]))
    return now


async def test_ten_failures_for_one_account_do_not_lock_out_another(tmp_path, clock):
    native = ConsoleNative()
    console = front(tmp_path, native)
    app = console.application(NATIVE)
    try:
        async with browser(app, console) as guessing, browser(app, console) as bob, \
                browser(app, console) as operator, browser(app, console) as alice:
            for _ in range(10):
                await sign_in(guessing, console, ALICE, "wrong-password-1", status=401)
            # Alice's account is held for the window, even with her password.
            await sign_in(alice, console, ALICE, ALICE_PASSWORD, status=429)
            # Others sign in from the same address meanwhile.
            await sign_in(bob, console, BOB, BOB_PASSWORD)
            await sign_in(operator, console, "", OPERATOR_PASSWORD)
            clock["now"] += 61
            await sign_in(alice, console, ALICE, ALICE_PASSWORD)
    finally:
        await console.close()


async def test_failures_from_one_client_address_do_not_lock_out_another_address(tmp_path, clock):
    native = ConsoleNative()
    console = front(tmp_path, native)
    app = console.application(NATIVE)
    try:
        async with browser(app, console, "192.0.2.10") as spraying, \
                browser(app, console, "192.0.2.20") as elsewhere:
            # Thirty failures from one address, never ten for one name.
            for index in range(30):
                await sign_in(spraying, console, f"guess-{index % 5}@example.test", "wrong-password-1", status=401)
            await sign_in(spraying, console, ALICE, ALICE_PASSWORD, status=429)
            await sign_in(elsewhere, console, ALICE, ALICE_PASSWORD)
            clock["now"] += 61
            await sign_in(spraying, console, BOB, BOB_PASSWORD)
    finally:
        await console.close()


async def test_login_forms_and_sessions_are_bounded_per_address_and_per_account(tmp_path, clock, monkeypatch):
    monkeypatch.setattr(console_auth, "SESSIONS_PER_ACCOUNT", 3)
    native = ConsoleNative()
    console = front(tmp_path, native)
    app = console.application(NATIVE)
    try:
        async with browser(app, console, "192.0.2.10") as busy, browser(app, console, "192.0.2.20") as other:
            for _ in range(100):
                assert (await busy.get("/login")).status_code == 200
            assert (await busy.get("/login")).status_code == 429
            # Another address still gets a sign-in form.
            assert (await other.get("/login")).status_code == 200
        for _ in range(3):
            async with browser(app, console) as alice:
                await sign_in(alice, console, ALICE, ALICE_PASSWORD)
        async with browser(app, console) as alice, browser(app, console) as bob:
            await sign_in(alice, console, ALICE, ALICE_PASSWORD, status=429)
            await sign_in(bob, console, BOB, BOB_PASSWORD)
    finally:
        await console.close()


# Rows 5 and 8: the controller review.
class ReviewError(RuntimeError):
    def __init__(self, code):
        self.code = code
        super().__init__(code)


class Review:
    """Inert controller review owner: one prompt, one claimed answer."""
    instances = []

    def __init__(self, context):
        self.context = context
        self.login_scope_id, self.expires_at_ms = context.login_scope_id, context.expires_at_ms
        self.origin, self.is_current = context.origin, context.is_current
        self.started, self.answers, self.retired = 0, [], []
        Review.instances.append(self)

    def start(self):
        self.started += 1

    async def observe(self):
        if not self.is_current():
            raise ReviewError("invalid_binding")
        if self.answers:
            return {"status": "answer_submitted"}
        return {"status": "prompt", "prompt": {"kind": "approval", "approval_id": "review-approval",
                "summary": "Use the reviewed controller", "body": {}, "allowed_decisions": ["approve", "deny"],
                "expires_at": None}}

    def claim_answer(self, approval_id, decision):
        if not self.is_current():
            raise ReviewError("invalid_binding")
        if self.answers or approval_id != "review-approval":
            raise ReviewError("stale_prompt")
        self.answers.append((approval_id, decision))

    def revoke(self):
        self.retired.append("revoked")

    async def retire(self, reason):
        self.retired.append(reason)

    async def close(self):
        self.retired.append("closed")


@pytest.fixture
def reviews(monkeypatch):
    Review.instances = []
    monkeypatch.setitem(sys.modules, console_auth.__package__ + ".native_controller_review",
                        SimpleNamespace(NativeControllerReview=Review, ControllerReviewError=ReviewError))
    return Review.instances


async def review_state(client, *, status=200):
    for _ in range(400):
        response = await client.get("/console/forge/controller-review/state")
        if response.status_code == status:
            return response
        await asyncio.sleep(0.005)
    raise AssertionError(f"The review state answered {response.status_code}: {response.text}")


async def answer_review(client, console):
    return await client.post("/console/forge/controller-review/answer",
        json={"approval_id": "review-approval", "decision": "approve"}, headers={"Origin": console.origin})


async def test_a_second_administrator_sees_the_review_after_the_first_logs_out(tmp_path, reviews):
    native = ConsoleNative(admins=(OPERATOR, ALICE, BOB))
    console = front(tmp_path, native, controller_review_factory=Review)
    app = console.application(NATIVE)
    try:
        async with browser(app, console) as alice, browser(app, console) as bob, browser(app, console) as carol:
            await sign_in(alice, console, ALICE, ALICE_PASSWORD)
            assert (await review_state(alice)).json()["status"] == "prompt"
            await sign_in(bob, console, BOB, BOB_PASSWORD)
            assert (await review_state(bob)).json()["status"] == "prompt"
            assert len(reviews) == 1 and reviews[0].started == 1
            # The person who opened it leaves; the review stays for the others.
            assert (await alice.post("/logout", headers={"Origin": console.origin})).status_code == 303
            assert reviews[0].retired == []
            assert (await review_state(bob)).json()["status"] == "prompt"
            response = await answer_review(bob, console)
            assert response.status_code == 202 and reviews[0].answers == [("review-approval", "approve")]
            # A person who is not an administrator does not see it.
            await sign_in(carol, console, CAROL, CAROL_PASSWORD)
            refused = await review_state(carol, status=403)
            assert refused.json() == {"error": {"code": "not_administrator"}}
            assert len(reviews) == 1
    finally:
        await console.close()
    assert reviews[0].retired and reviews[0].retired[-1] in ("closed", "application_stopping")


async def test_shared_account_cannot_answer_the_controller_review(tmp_path, reviews):
    native = ConsoleNative(admins=(OPERATOR,))
    console = shared_front(tmp_path, native, controller_review_factory=Review)
    app = console.application(NATIVE)
    try:
        async with browser(app, console) as client:
            await sign_in(client, console, None, OPERATOR_PASSWORD)
            state = await client.get("/console/forge/controller-review/state")
            assert state.status_code == 403 and state.json() == {"error": {"code": "approver_unauthenticated"}}
            response = await answer_review(client, console)
            assert response.status_code == 403
            assert response.json() == {"error": {"code": "approver_unauthenticated"}}
            assert all(review.answers == [] for review in reviews)
    finally:
        await console.close()


async def test_shared_account_cannot_answer_setup_prompts(tmp_path):
    factory, children, _, traces = managed_connect_factory(tmp_path)
    native = ConsoleNative()
    console = shared_front(tmp_path, native, connect_factory=factory)
    app, tasks = console.application(NATIVE), []
    try:
        async with browser(app, console) as client:
            await sign_in(client, console, None, OPERATOR_PASSWORD)
            await connect_ready(client, console)
            task, frames, _ = await connect_view(app, console, client.cookies[console.cookie], private=True)
            tasks.append(task)
            await frames.get()
            sent = await client.post("/console/forge/connect/send", json={"text": "Set up the channel"},
                                     headers={"Origin": console.origin})
            assert sent.status_code == 202
            while (prompt := await asyncio.wait_for(frames.get(), 5))["type"] != "prompt":
                pass
            response = await client.post("/console/forge/connect/answer",
                json={"prompt_id": prompt["id"], "answer": {"kind": "approval", "decision": "approve"}},
                headers={"Origin": console.origin})
            assert response.status_code == 403
            assert response.json() == {"error": {"code": "approver_unauthenticated"}}
    finally:
        await console.close()
        await finish_stream_tasks(tasks)
    wire = [json.loads(row) for row in traces[0].read_text().splitlines()]
    assert not any(row.get("id") == prompt["id"] and "result" in row for row in wire)


# Row 7: the local frontend-management approval path needs a person.
async def test_shared_account_login_cannot_approve_frontend_management(tmp_path):
    from test_frontend_management import CONFIG, HOST, PATH, Owner
    owner = Owner()
    native = ConsoleNative()
    console = shared_front(tmp_path, native)
    console.frontend_management = owner
    console.frontend_host = lambda: dict(HOST)
    console.frontend_private_fields = ("CONSOLE_LOGIN_PASSWORD",)
    app = console.application(NATIVE)
    try:
        async with browser(app, console) as client:
            await sign_in(client, console, None, OPERATOR_PASSWORD)
            # Reads stay open to the shared account.
            assert (await client.get(PATH)).status_code == 200
            prepared = await client.post(PATH + "/prepare", json={"configuration": CONFIG,
                "expected_revision": {"component": 7}}, headers={"Origin": console.origin})
            assert prepared.status_code == 403, prepared.text
            # A proof this front would accept still approves nothing for it.
            proposal = {"change_id": "change:one", "configuration": CONFIG, "expected_revision": {"component": 7}}
            expires_at = int(console_auth.time.time()) + 120
            proof = console._frontend_proof(HOST, proposal, expires_at)
            submitted = await client.post(PATH + "/submit", json={"proposal": proposal,
                "approval": {"approved": True, "expires_at": expires_at, "proof": proof}},
                headers={"Origin": console.origin})
            assert submitted.status_code == 403, submitted.text
            assert [request.operation for request in owner.calls] == ["inventory"]
            assert not any(request.approved for request in owner.calls)
    finally:
        await console.close()


async def test_named_account_approves_frontend_management_as_itself(tmp_path):
    from test_frontend_management import CONFIG, HOST, PATH, Owner
    owner = Owner()
    native = ConsoleNative()
    console = front(tmp_path, native)
    console.frontend_management = owner
    console.frontend_host = lambda: dict(HOST)
    console.frontend_private_fields = ("CONSOLE_LOGIN_PASSWORD",)
    app = console.application(NATIVE)
    try:
        async with browser(app, console) as alice, browser(app, console) as bob:
            await sign_in(alice, console, ALICE, ALICE_PASSWORD)
            await sign_in(bob, console, BOB, BOB_PASSWORD)
            prepared = await alice.post(PATH + "/prepare", json={"configuration": CONFIG,
                "expected_revision": {"component": 7}}, headers={"Origin": console.origin})
            assert prepared.status_code == 200, prepared.text
            body = {"proposal": prepared.json()["proposal"],
                    "approval": {"approved": True, **prepared.json()["approval"]}}
            # Alice's proof is hers: Bob cannot submit it.
            assert (await bob.post(PATH + "/submit", json=body, headers={"Origin": console.origin})).status_code == 403
            accepted = await alice.post(PATH + "/submit", json=body, headers={"Origin": console.origin})
            assert accepted.status_code == 200, accepted.text
            approved = [request for request in owner.calls if request.approved]
            assert len(approved) == 1 and approved[0].principal == ALICE
            assert approved[0].payload["approval"]["principal"] == ALICE
    finally:
        await console.close()


# Row 6: further access admins after the installation's own account.
def test_console_operator_is_the_first_of_distinct_admins():
    from launchkit.templates_v2.console_configuration import console_operator
    assert console_operator({"enabled": True, "admins": [OPERATOR]}) == OPERATOR
    assert console_operator({"enabled": True, "admins": [OPERATOR, ALICE, BOB], "rules": []}) == OPERATOR
    for invalid in ({"enabled": False, "admins": [OPERATOR]}, {"enabled": True, "admins": []},
                    {"enabled": True, "admins": [OPERATOR, OPERATOR]}, {"enabled": True, "admins": [" " + OPERATOR]},
                    {"enabled": True, "admins": [OPERATOR, ""]}, {"enabled": True, "admins": OPERATOR},
                    {"enabled": True, "admins": [OPERATOR, 7]}, {"access": {"enabled": True, "admins": [OPERATOR]}}):
        with pytest.raises(ValueError):
            console_operator(invalid)


async def test_forge_deployment_treats_named_account_packages_as_authenticated(tmp_path):
    from launchkit import deployment
    from launchkit.remote_browser_access import browser_responds
    from launchkit.templates_v2.console_configuration import CONSOLE_FIELDS
    fields = [dict(field) for field in CONSOLE_FIELDS]
    files = {"application/console.json": (b'{"authentication":"local-accounts"}', 0o644)}
    assert deployment.package_console_scope(files, fields, {})["authentication"] == "local-accounts"
    with pytest.raises(ValueError, match="CONSOLE_LOGIN_PASSWORD"):
        deployment.prepare_private_settings(tmp_path, fields, {"CONSOLE_USER_ID": OPERATOR,
            "CONSOLE_LOGIN_PASSWORD": "short"}, False, port=18088, console_authentication="local-accounts")
    values = deployment.prepare_private_settings(tmp_path, fields, {"CONSOLE_USER_ID": OPERATOR,
        "CONSOLE_LOGIN_PASSWORD": OPERATOR_PASSWORD}, False, port=18088, console_authentication="local-accounts")
    assert values["CONSOLE_USER_ID"] == OPERATOR and not (tmp_path / ".env").exists()

    async def sign_in_redirect(reader, writer):
        await reader.readuntil(b"\r\n\r\n")
        writer.write(b"HTTP/1.1 303 See Other\r\nLocation: /login\r\n\r\n")
        await writer.drain()
        writer.close()
        await writer.wait_closed()
    server = await asyncio.start_server(sign_in_redirect, "127.0.0.1", 0)
    try:
        assert await browser_responds(server.sockets[0].getsockname()[1], "local-accounts") is True
    finally:
        server.close()
        await server.wait_closed()


# Row 2: native consent reads and decisions reach the console as the
# signed-in person, under the setup routes' rules: closed JSON, a bounded
# body, JSON only, no query, the front's own Origin and the login's token.
@pytest.mark.parametrize("route,payload,limit", [
    ("read", {"approval_id": "Å" * 256}, 4 * 1024),
    ("decide", {"approval_id": "opaque-approval", "decision": "deny", "reason": "Å" * 2048}, 32 * 1024),
])
async def test_consent_read_and_decide_are_forwarded(tmp_path, route, payload, limit):
    native = ConsoleNative()
    console = front(tmp_path, native)
    app = console.application(NATIVE)
    path = "/host/toolkit/consent/" + route
    raw = json.dumps(payload, ensure_ascii=False).encode("utf-8")
    raw += b" " * (limit - len(raw))
    headers = {"Origin": console.origin, "Content-Type": "application/json"}
    try:
        async with browser(app, console) as alice:
            await sign_in(alice, console, ALICE, ALICE_PASSWORD)
            response = await alice.post(path, content=raw,
                headers={**headers, "Authorization": "Bearer browser-canary"})
            assert response.status_code == 200, response.text
            request = native.requests[-1]
            assert request.method == "POST" and request.url.raw_path == path.encode()
            assert request.content == raw and request.headers["origin"] == console.origin
            claims = bearer_claims(request, console)
            assert claims["sub"] == ALICE and claims["sid"] == login_of(alice, console).login_scope_id
            assert response.headers["cache-control"] == "no-store"
            before = len(native.requests)
            refused = [b"[]", b"null", json.dumps({**payload, "principal": OPERATOR}).encode(),
                       json.dumps({**payload, "approval_id": "Å" * 257}).encode(),
                       json.dumps({**payload, "approval_id": 7}).encode(), b" " * (limit + 1)]
            if route == "decide":
                refused += [json.dumps({**payload, "decision": "browser-approved"}).encode(),
                            json.dumps({**payload, "reason": "x" * 4097}).encode(),
                            json.dumps({**payload, "reason": ["opaque"]}).encode(),
                            json.dumps({"approval_id": "opaque-approval"}).encode()]
            for body in refused:
                assert (await alice.post(path, content=body, headers=headers)).status_code == 400
            assert (await alice.post(path + "?approval_id=other", content=raw, headers=headers)).status_code == 400
            assert (await alice.post(path, content=raw, headers={**headers, "Content-Type": "text/plain"})).status_code == 400
            assert (await alice.post(path, content=raw, headers={**headers, "Origin": "https://foreign.example"})).status_code == 403
            assert (await alice.get(path)).status_code == 405
            assert len(native.requests) == before
        async with browser(app, console) as stranger:
            assert (await stranger.post(path, content=raw, headers=headers)).status_code == 401
            assert len(native.requests) == before
    finally:
        await console.close()


# Forge's Build default: named accounts for new builds, and the one console
# update transition that moves an existing installation on its next update.
SHARED = b'{"authentication":"local-password"}\n'
NAMED = b'{"authentication":"local-accounts"}\n'
UPDATE_FIELDS = (json.dumps({"fields": [
    {"key": "CONSOLE_USER_ID", "label": "Console user", "secret": False, "required": True},
    {"key": "CONSOLE_LOGIN_PASSWORD", "label": "Console password", "secret": True, "required": True,
     "min_length": 12, "max_length": 4096}]}, separators=(",", ":")) + "\n").encode()
UPDATE_ACCESS = ('enabled = true\nadmins = ["' + OPERATOR + '"]\nrules = []\n\n[groups]\n').encode()


def console_update(before, after, *, fields=UPDATE_FIELDS, access=UPDATE_ACCESS, mode=0o644):
    from launchkit import deployment_updates as updates
    previous = {"application/configuration.json": (UPDATE_FIELDS, 0o644), "config/access.toml": (UPDATE_ACCESS, 0o644)}
    candidate = {"application/configuration.json": (fields, 0o644), "config/access.toml": (access, 0o644)}
    if before is not None:
        previous["application/console.json"] = (before, 0o644)
    if after is not None:
        candidate["application/console.json"] = (after, mode)
    return updates.compatibility(previous, candidate, {})


def test_update_moves_one_shared_console_account_to_named_accounts():
    changes, blockers = console_update(SHARED, NAMED)
    assert changes == [{"path": "application/console.json", "change": "changed"}]
    assert blockers == []
    # The same two documents written with other whitespace are the same choice.
    assert console_update(b'{\n  "authentication": "local-password"\n}\n', b'{"authentication": "local-accounts"}')[1] == []


@pytest.mark.parametrize("variant", ["fields", "operator", "back", "other", "added", "removed", "mode",
                                     "extra", "duplicate", "oversized"])
def test_update_refuses_every_other_console_authentication_change(variant):
    before, after, options = SHARED, NAMED, {}
    if variant == "fields":
        options["fields"] = UPDATE_FIELDS.replace(b'"Console user"', b'"Console account"')
    elif variant == "operator":
        options["access"] = UPDATE_ACCESS.replace(OPERATOR.encode(), b"another-operator")
    elif variant == "back":
        before, after = NAMED, SHARED
    elif variant == "other":
        after = b'{"authentication":"oidc"}\n'
    elif variant == "added":
        before = None
    elif variant == "removed":
        after = None
    elif variant == "mode":
        options["mode"] = 0o755
    elif variant == "extra":
        after = b'{"authentication":"local-accounts","accounts":["' + ALICE.encode() + b'"]}\n'
    elif variant == "duplicate":
        after = b'{"authentication":"local-password","authentication":"local-accounts"}\n'
    else:
        after = NAMED + b" " * 4096
    _, blockers = console_update(before, after, **options)
    assert any("application/console.json" in blocker for blocker in blockers)


async def console_installation(tmp_path):
    """One installed package with the single shared account, then its next build."""
    from launchkit.deployment import materialize, read_receipt, update_receipt
    from test_deployment import Guide, Implementation, Service, submit
    guide = Guide()
    implementation = Implementation(guide)
    implementation.digest = "c" * 64
    implementation.application_configuration = json.loads(UPDATE_FIELDS)
    implementation.extra_files = {"application/console.json": SHARED.decode(),
                                  "config/access.toml": UPDATE_ACCESS.decode()}
    service = Service(tmp_path / "private-server", guide, implementation)
    destination = tmp_path / "application"
    await submit(service, destination, settings={"OPENAI_API_KEY": "private-api-key-value",
        "CONSOLE_USER_ID": OPERATOR, "CONSOLE_LOGIN_PASSWORD": OPERATOR_PASSWORD})
    directory = service._directory(guide.identity)
    materialize(directory)
    await service.shutdown()
    update_receipt(directory, status="stopped")
    previous = read_receipt(directory)
    implementation.identity = str(uuid.uuid4())
    implementation.digest = "d" * 64
    implementation.extra_files = {"application/console.json": NAMED.decode(),
                                  "config/access.toml": UPDATE_ACCESS.decode()}
    return service, directory, destination, previous


async def test_an_installed_shared_account_moves_to_named_accounts_on_its_next_update(tmp_path):
    from launchkit import deployment_updates as updates
    service, directory, destination, previous = await console_installation(tmp_path)
    private = (destination / ".env").read_bytes()
    receipt = await service.stage_update(service.guide.identity, previous["deployment_id"],
        service.guide.state["version"], service.implementation.identity)
    update = receipt["update"]
    assert update["status"] == "staged" and update["blockers"] == [], update["blockers"]
    assert {"path": "application/console.json", "change": "changed"} in update["changes"]
    updates.activate(directory, update["update_id"])
    assert (destination / "application/console.json").read_bytes() == NAMED
    assert (destination / "config/access.toml").read_bytes() == UPDATE_ACCESS
    assert (destination / ".env").read_bytes() == private


async def test_an_update_that_also_changes_the_operator_stays_blocked(tmp_path):
    service, directory, destination, previous = await console_installation(tmp_path)
    service.implementation.extra_files["config/access.toml"] = UPDATE_ACCESS.decode().replace(OPERATOR, "another-operator")
    receipt = await service.stage_update(service.guide.identity, previous["deployment_id"],
        service.guide.state["version"], service.implementation.identity)
    update = receipt["update"]
    assert update["status"] == "blocked"
    assert any("application/console.json" in blocker for blocker in update["blockers"])
    assert (destination / "application/console.json").read_bytes() == SHARED


def test_legacy_ordinary_source_keeps_one_shared_account():
    # Its continuation re-derives these bytes and refuses different ones, so
    # the legacy ordinary route keeps the single shared account.
    from launchkit.ordinary_application_source import _console_source
    assert json.loads(_console_source("guide")["application/console.json"]) == {"authentication": "local-password"}
