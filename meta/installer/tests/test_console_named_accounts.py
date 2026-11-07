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
import base64
from contextlib import asynccontextmanager
import hashlib
import json
import os
import socket
import stat
import sys
import threading
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
        # A replacement answer for `meta/console/whoami`, given the request id.
        self.whoami_answer = None

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
                if self.whoami_answer is not None:
                    return self.whoami_answer(identity)
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


def test_a_later_save_removes_the_temporary_file_a_killed_setup_left(tmp_path):
    """Setup saves the named accounts through a private temporary file and a
    rename. Killed before the rename, it leaves that file, with the accounts'
    hashes, in .mobkit: a later save leaves a young one to the setup that may
    still be writing it and removes an old one.
    """
    import signal
    import subprocess
    from pathlib import Path
    root = tmp_path / "application"
    root.mkdir()
    script = tmp_path / "killed_save.py"
    script.write_text(
        "import os, signal, sys\n"
        f"sys.path.insert(0, {str(Path(__file__).parents[1])!r})\n"
        "from launchkit.templates_v2 import console_auth\n"
        "def killed(*args, **kwargs):\n"
        "    os.kill(os.getpid(), signal.SIGKILL)\n"
        "os.replace = killed\n"
        f"console_auth.save_console_accounts({str(root)!r}, {named_accounts(ALICE)!r}, None, {OPERATOR!r})\n")
    result = subprocess.run([sys.executable, str(script)], capture_output=True, timeout=60)
    assert result.returncode == -signal.SIGKILL, result.stderr
    state = root / ".mobkit"
    left, = state.glob(".console-accounts.*.pending")
    assert b"scrypt:" in left.read_bytes() and not (state / console_auth.ACCOUNTS_FILE).exists()
    console_auth.save_console_accounts(root, named_accounts(BOB), None, OPERATOR)
    assert left.exists()
    hour_ago = time.time() - 3600
    os.utime(left, (hour_ago, hour_ago))
    accounts, original = console_auth.console_accounts_for_change(root, OPERATOR)
    console_auth.save_console_accounts(root, accounts, original, OPERATOR)
    assert sorted(path.name for path in state.iterdir()) == [console_auth.ACCOUNTS_FILE]
    assert set(console_auth.load_console_accounts(root, OPERATOR)) == {BOB}


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
            oldest = next(iter(console.login_tokens))
            # A full address refuses no form: its oldest form goes instead, so
            # one caller cannot hold off everyone who shares its address.
            assert (await busy.get("/login")).status_code == 200
            assert oldest not in console.login_tokens and len(console.login_tokens) == 100
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


async def test_shared_account_cannot_answer_setup_bridge_prompts(tmp_path):
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


# Review fix round. P1-1: the console's live access file is private state.
SECOND_ADMIN = ("# Meta access control. Managed by the console Access panel;\n"
                "# hand edits are preserved until the next console save.\n\n"
                'enabled = true\nadmins = ["' + OPERATOR + '", "' + ALICE + '"]\nrules = []\n\n[groups]\n').encode()


async def test_a_console_access_edit_keeps_forge_managing_the_installation(tmp_path):
    from launchkit import deployment_updates as updates
    from launchkit.deployment import update_receipt
    service, directory, destination, previous = await console_installation(tmp_path)
    gid, did = service.guide.identity, previous["deployment_id"]
    stopped = {"status": "stopped", "interactive_lifecycle_id": None, "interactive_cleanup": None,
               "process_live": False, "runtime_pid": None}
    update_receipt(directory, **stopped)
    # The console's Access panel adds a second administrator to its live copy.
    (destination / ".mobkit").mkdir(mode=0o700, exist_ok=True)
    live = destination / ".mobkit/console-access.toml"
    live.write_bytes(SECOND_ADMIN)
    live.chmod(0o600)
    installed = {"version": service.guide.state["version"], "deployment_id": did,
                 "build_id": previous["build_id"], "source_digest": "c" * 64,
                 "package_sha256": previous["package_sha256"]}
    # Change private settings and the installed package download.
    await service.configure(gid, **installed, settings={"OPENAI_API_KEY": "replacement-private-value"})
    await service.installed_package(gid, did, previous["build_id"], "c" * 64, previous["package_sha256"])
    # Restart, status and Stop; the inert worker confirms no native cleanup.
    restarted = await service.action(gid, did, "restart")
    assert restarted["status"] not in ("failed", "blocked"), restarted
    await service.status(gid)
    await service.action(gid, did, "stop")
    await service.shutdown()
    update_receipt(directory, **stopped)
    # The next update stages, applies and leaves the live copy as the console wrote it.
    receipt = await service.stage_update(gid, did, service.guide.state["version"], service.implementation.identity)
    assert receipt["update"]["status"] == "staged", receipt["update"]["blockers"]
    updates.activate(directory, receipt["update"]["update_id"])
    assert live.read_bytes() == SECOND_ADMIN
    assert (destination / "config/access.toml").read_bytes() == UPDATE_ACCESS


def test_supervise_ignores_the_consoles_live_access_file(tmp_path):
    import hashlib as digests
    import types
    from launchkit.blueprint import starter_blueprint
    from launchkit.compiler import generate_files
    content = generate_files(starter_blueprint())["remote_operations.py"]
    operations = types.ModuleType("generated_remote_operations")
    operations.__file__ = str(tmp_path / "remote_operations.py")
    exec(compile(content, operations.__file__, "exec"), operations.__dict__)
    root = tmp_path / "installed"
    files = {"deployment.json": b"{}\n", "app.py": b"# app\n", "remote_operations.py": b"# operations\n",
             "config/access.toml": UPDATE_ACCESS}
    for name, data in files.items():
        path = root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)
        path.chmod(0o644)
    (root / "SHA256SUMS").write_text("".join(f"{digests.sha256(data).hexdigest()}  {name}\n"
                                             for name, data in sorted(files.items())))
    before = operations.source_digest(root)
    (root / ".mobkit").mkdir(mode=0o700)
    (root / ".mobkit/console-access.toml").write_bytes(SECOND_ADMIN)
    assert operations.source_digest(root) == before
    # The package's own file still has to be the accepted bytes.
    (root / "config/access.toml").write_bytes(SECOND_ADMIN)
    with pytest.raises(operations.DestinationError):
        operations.source_digest(root)


# P2-1: an attempt counts before the password work.
def concurrent_clients(app, console, count, address="192.0.2.50"):
    return [httpx.AsyncClient(transport=httpx.ASGITransport(app=app, client=(address, 41000)),
                              base_url=console.origin) for _ in range(count)]


async def concurrent_attempts(clients, console, accounts, password="wrong-password-1"):
    nonces = []
    for client in clients:
        assert (await client.get("/login")).status_code == 200
        nonces.append(client.cookies[console.cookie + "_login"])
    return [asyncio.create_task(client.post("/login", data={"nonce": nonce, "account": account, "password": password},
                                            headers={"Origin": console.origin}))
            for client, nonce, account in zip(clients, nonces, accounts)]


async def test_concurrent_wrong_passwords_for_one_account_stay_within_its_limit(tmp_path, monkeypatch):
    monkeypatch.setattr(console_auth, "PASSWORD_CHECKS", 1000, raising=False)
    checks, original = [], console_auth.verify_console_password

    def counted(stored, password):
        checks.append(password)
        time.sleep(0.02)
        return original(stored, password)
    monkeypatch.setattr(console_auth, "verify_console_password", counted)
    native = ConsoleNative()
    console = front(tmp_path, native)
    app = console.application(NATIVE)
    clients = concurrent_clients(app, console, 40)
    try:
        answers = await asyncio.gather(*await concurrent_attempts(clients, console, [ALICE] * 40))
        codes = [answer.status_code for answer in answers]
        assert len(checks) <= console_auth.FAILURES_PER_ACCOUNT
        assert codes.count(401) == len(checks) and codes.count(429) == 40 - len(checks)
    finally:
        for client in clients:
            await client.aclose()
        await console.close()


# P2-2: the address limit keys on the socket peer, and password checks are capped.
def free_port():
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return probe.getsockname()[1]


@asynccontextmanager
async def listening(tmp_path, native, **options):
    port = free_port()
    console = ConsoleFront(tmp_path, port, OPERATOR, OPERATOR_PASSWORD, accounts=named_accounts(ALICE, BOB, CAROL),
                           runtime_namespace="meta", transport=native.transport(), **options)
    native.auth = console.native_auth
    await console.start(NATIVE)
    try:
        yield console, f"http://127.0.0.1:{port}"
    finally:
        await console.close()


async def listener_sign_in(base, console, account, password, headers=None):
    headers = headers or {}
    async with httpx.AsyncClient(base_url=base) as client:
        assert (await client.get("/login", headers=headers)).status_code == 200
        nonce = client.cookies[console.cookie + "_login"]
        response = await client.post("/login", data={"nonce": nonce, "account": account, "password": password},
                                     headers={"Origin": console.origin, **headers})
        return response.status_code, client.cookies.get(console.cookie)


async def test_a_client_set_forwarded_for_does_not_change_the_address_limit(tmp_path):
    native = ConsoleNative()
    async with listening(tmp_path, native) as (console, base):
        for index in range(console_auth.FAILURES_PER_CLIENT):
            status, _ = await listener_sign_in(base, console, f"guess-{index % 5}@example.test", "wrong-password-1",
                                               {"X-Forwarded-For": "198.51.100.1"})
            assert status == 401
        # Another forwarded address from the same peer is the same client.
        status, _ = await listener_sign_in(base, console, ALICE, ALICE_PASSWORD, {"X-Forwarded-For": "198.51.100.2"})
        assert status == 429
        assert {key[1] for key in console.failures if key[0] == "client"} == {"127.0.0.1"}


async def test_password_checks_are_capped_across_accounts(tmp_path, monkeypatch):
    monkeypatch.setattr(console_auth, "PASSWORD_CHECKS", 3, raising=False)
    release, entered = threading.Event(), []

    def held(stored, password):
        entered.append(password)
        release.wait(10)
        return False
    monkeypatch.setattr(console_auth, "verify_console_password", held)
    native = ConsoleNative()
    console = front(tmp_path, native)
    app = console.application(NATIVE)
    clients, tasks = concurrent_clients(app, console, 8), []
    try:
        tasks = await concurrent_attempts(clients, console, [f"person-{index}@example.test" for index in range(8)])
        for _ in range(500):
            if len(entered) >= 3 and sum(task.done() for task in tasks) >= 5:
                break
            await asyncio.sleep(0.01)
        refused = [task for task in tasks if task.done()]
        assert len(entered) == 3 and len(refused) == 5
        assert all(task.result().status_code == 429 and "Try again shortly" in task.result().text for task in refused)
    finally:
        release.set()
        await asyncio.gather(*tasks, return_exceptions=True)
        for client in clients:
            await client.aclose()
        await console.close()


# P3-3 and P3-4: one password check per account, invalid names hashed never.
async def test_every_account_costs_one_password_check(tmp_path, monkeypatch):
    checks, original = [], console_auth.verify_console_password
    monkeypatch.setattr(console_auth, "verify_console_password",
                        lambda stored, password: checks.append(1) or original(stored, password))
    native = ConsoleNative()
    console = front(tmp_path, native)
    app = console.application(NATIVE)
    try:
        for account, password, status in ((None, OPERATOR_PASSWORD, 303), ("", "wrong-password-1", 401),
                                          (ALICE, ALICE_PASSWORD, 303), ("nobody@example.test", "wrong-password-1", 401)):
            async with browser(app, console) as client:
                before = len(checks)
                await sign_in(client, console, account, password, status=status)
                assert len(checks) == before + 1, account
    finally:
        await console.close()


async def test_names_that_cannot_be_accounts_share_one_limit_without_hashing(tmp_path, monkeypatch):
    checks, original = [], console_auth.verify_console_password
    monkeypatch.setattr(console_auth, "verify_console_password",
                        lambda stored, password: checks.append(1) or original(stored, password))
    native = ConsoleNative()
    console = front(tmp_path, native)
    app = console.application(NATIVE)
    try:
        async with browser(app, console, "192.0.2.60") as guessing:
            for index in range(console_auth.FAILURES_PER_ACCOUNT):
                await sign_in(guessing, console, f"not an account {index} " + "x" * 300, "wrong-password-1", status=401)
            assert checks == []
            await sign_in(guessing, console, "yet another invalid name!", "wrong-password-1", status=429)
            # Real accounts are not held by those attempts.
            await sign_in(guessing, console, ALICE, ALICE_PASSWORD)
        assert len([key for key in console.failures if key[0] == "account"]) <= 2
    finally:
        await console.close()


async def test_client_address_keys_are_bounded(tmp_path, monkeypatch):
    monkeypatch.setattr(console_auth, "FAILURE_CLIENTS", 5, raising=False)
    native = ConsoleNative()
    console = front(tmp_path, native)
    app = console.application(NATIVE)
    try:
        for index in range(12):
            async with browser(app, console, f"192.0.2.{100 + index}") as client:
                await sign_in(client, console, f"guess-{index}@example.test", "wrong-password-1", status=401)
        assert len([key for key in console.failures if key[0] == "client"]) <= 5
    finally:
        await console.close()


def test_new_password_hashes_meet_the_scrypt_floor_and_name_their_parameters():
    stored = console_auth.hash_console_password(ALICE_PASSWORD)
    algorithm, n, r, p, salt, digest = stored.split(":")
    assert algorithm == "scrypt" and (int(n), int(r), int(p)) in {(2**14, 8, 5), (2**15, 8, 3), (2**16, 8, 2), (2**17, 8, 1)}
    assert console_auth.verify_console_password(stored, ALICE_PASSWORD)
    assert not console_auth.verify_console_password(stored, BOB_PASSWORD)

    def stored_with(n, r, p):
        salt = os.urandom(16)
        derived = hashlib.scrypt(ALICE_PASSWORD.encode(), salt=salt, n=n, r=r, p=p, dklen=32,
                                 maxmem=256 * 1024 * 1024)
        encode = lambda data: base64.urlsafe_b64encode(data).rstrip(b"=").decode()
        return f"scrypt:{n}:{r}:{p}:{encode(salt)}:{encode(derived)}"
    # A stored hash names its parameters; another allowed choice verifies.
    assert console_auth.verify_console_password(stored_with(2**17, 8, 1), ALICE_PASSWORD)
    # Parameters below the floor are refused.
    for weak in ((2**14, 8, 1), (2**10, 8, 1)):
        with pytest.raises(ValueError):
            console_auth.verify_console_password(stored_with(*weak), ALICE_PASSWORD)


# P3-1: the review is an administrator's, from its start.
async def started_reviews(console):
    for login in list(console._logins.values()):
        if login.controller_review_start is not None:
            await asyncio.gather(login.controller_review_start, return_exceptions=True)


async def test_the_review_starts_at_an_administrators_first_login(tmp_path, reviews):
    native = ConsoleNative(admins=(BOB,))
    console = front(tmp_path, native, controller_review_factory=Review)
    app = console.application(NATIVE)
    try:
        async with browser(app, console) as alice, browser(app, console) as bob:
            await sign_in(alice, console, ALICE, ALICE_PASSWORD)
            await started_reviews(console)
            assert reviews == []
            await sign_in(bob, console, BOB, BOB_PASSWORD)
            assert (await review_state(bob)).json()["status"] == "prompt"
            assert len(reviews) == 1 and reviews[0].login_scope_id == login_of(bob, console).login_scope_id
    finally:
        await console.close()


# P3-A: an administrator who opens the review starts it, when the start at
# sign-in could not.
async def test_the_review_starts_when_an_administrator_opens_it_after_a_failed_start(tmp_path, reviews):
    native = ConsoleNative(admins=(ALICE,))

    def unavailable(identity):
        raise httpx.ConnectError("the console is still starting")
    native.whoami_answer = unavailable
    console = front(tmp_path, native, controller_review_factory=Review)
    app = console.application(NATIVE)
    try:
        async with browser(app, console) as alice:
            await sign_in(alice, console, ALICE, ALICE_PASSWORD)
            await started_reviews(console)
            assert reviews == []
            native.whoami_answer = None
            state = await alice.get("/console/forge/controller-review/state")
            assert state.status_code == 200 and state.json()["status"] == "prompt"
            assert len(reviews) == 1 and reviews[0].started == 1
    finally:
        await console.close()


async def test_the_review_starts_for_an_administrator_who_signed_in_during_another_check(
        tmp_path, reviews, monkeypatch):
    native = ConsoleNative(admins=(BOB,))
    console = front(tmp_path, native, controller_review_factory=Review)
    checking, release = asyncio.Event(), asyncio.Event()
    original = console._console_administrator

    async def slow_for_carol(login):
        if login.principal == CAROL:
            checking.set()
            await release.wait()
        return await original(login)
    monkeypatch.setattr(console, "_console_administrator", slow_for_carol)
    app = console.application(NATIVE)
    try:
        async with browser(app, console) as carol, browser(app, console) as bob:
            await sign_in(carol, console, CAROL, CAROL_PASSWORD)
            await checking.wait()
            # Bob signs in while Carol's check holds the start.
            await sign_in(bob, console, BOB, BOB_PASSWORD)
            release.set()
            await started_reviews(console)
            assert reviews == []
            state = await bob.get("/console/forge/controller-review/state")
            assert state.status_code == 200 and state.json()["status"] == "prompt"
            assert len(reviews) == 1 and reviews[0].login_scope_id == login_of(bob, console).login_scope_id
    finally:
        await console.close()


# P3-C: a native callback is never mounted on a route the console owns, the
# consent routes included. Plain stand-ins: no toolkit is needed.
@pytest.mark.parametrize("path,mounted", [
    ("/host/toolkit/consent/decide", False), ("/host/toolkit/consent/read", False),
    ("/host/toolkit/consent", False), ("/host/toolkit/consent/", False),
    ("/host/toolkit/setup/callback", False), ("/login", False), ("/login/callback", False),
    ("/mcp/callback", True)])
def test_native_callbacks_are_never_mounted_on_the_consoles_own_routes(tmp_path, path, mounted):
    console = ConsoleFront(tmp_path, 8855, OPERATOR, OPERATOR_PASSWORD, runtime_namespace="meta",
                           public_origin="https://console.example.test")
    login = SimpleNamespace(mcp_attempts={})
    attempt = SimpleNamespace(client=None, binding=object())
    if mounted:
        console._mount_mcp(login, attempt, console.origin + path)
        assert console._mcp_routes[path] == (console.origin + path, attempt.binding)
    else:
        with pytest.raises(ValueError, match="cannot be mounted"):
            console._mount_mcp(login, attempt, console.origin + path)
        assert path not in console._mcp_routes


@pytest.mark.parametrize("answer", ["transport", "not-served", "malformed"])
async def test_the_review_is_unavailable_when_the_console_cannot_tell(tmp_path, reviews, answer):
    native = ConsoleNative(admins=(ALICE,))
    console = front(tmp_path, native, controller_review_factory=Review)
    app = console.application(NATIVE)
    try:
        async with browser(app, console) as alice:
            await sign_in(alice, console, ALICE, ALICE_PASSWORD)
            assert (await review_state(alice)).json()["status"] == "prompt"

            def unknown(identity):
                if answer == "transport":
                    raise httpx.ConnectError("the console is unavailable")
                if answer == "not-served":
                    return httpx.Response(200, json={"jsonrpc": "2.0", "id": identity,
                        "error": {"code": -32601, "message": "Method not found"}})
                return httpx.Response(200, json={"jsonrpc": "2.0", "id": identity, "result": {"actor": "person"}})
            native.whoami_answer = unknown
            state = await alice.get("/console/forge/controller-review/state")
            assert state.status_code == 503 and state.json() == {"error": {"code": "unavailable"}}
            answered = await answer_review(alice, console)
            assert answered.status_code == 503 and answered.json() == {"error": {"code": "unavailable"}}
            assert reviews[0].answers == []
    finally:
        await console.close()


async def test_an_administrator_demoted_between_state_and_answer_is_refused(tmp_path, reviews):
    native = ConsoleNative(admins=(ALICE,))
    console = front(tmp_path, native, controller_review_factory=Review)
    app = console.application(NATIVE)
    try:
        async with browser(app, console) as alice:
            await sign_in(alice, console, ALICE, ALICE_PASSWORD)
            assert (await review_state(alice)).json()["status"] == "prompt"
            native.admins.discard(ALICE)
            refused = await answer_review(alice, console)
            assert refused.status_code == 403 and refused.json() == {"error": {"code": "not_administrator"}}
            assert reviews[0].answers == []
    finally:
        await console.close()


async def test_an_administrator_demoted_while_the_review_is_read_sees_nothing(tmp_path, reviews, monkeypatch):
    native = ConsoleNative(admins=(ALICE,))
    console = front(tmp_path, native, controller_review_factory=Review)
    app = console.application(NATIVE)
    try:
        async with browser(app, console) as alice:
            await sign_in(alice, console, ALICE, ALICE_PASSWORD)
            assert (await review_state(alice)).json()["status"] == "prompt"
            observe = Review.observe

            async def demoted(self):
                native.admins.discard(ALICE)
                return await observe(self)
            monkeypatch.setattr(Review, "observe", demoted)
            refused = await alice.get("/console/forge/controller-review/state")
            assert refused.status_code == 403 and refused.json() == {"error": {"code": "not_administrator"}}
            assert "review-approval" not in refused.text
    finally:
        await console.close()


# P3-2: dot and empty path segments are refused before routing.
async def raw_request(port, request):
    reader, writer = await asyncio.open_connection("127.0.0.1", port)
    writer.write(request)
    await writer.drain()
    answer = await reader.read(65536)
    writer.close()
    return int(answer.split(b" ", 2)[1])


def raw_post(console, session, target, body, content_type=b"application/json"):
    return (b"POST " + target + b" HTTP/1.1\r\nHost: 127.0.0.1:" + str(console.port).encode()
            + b"\r\nOrigin: " + console.origin.encode() + b"\r\nContent-Type: " + content_type
            + b"\r\nCookie: " + console.cookie.encode() + b"=" + session.encode()
            + b"\r\nContent-Length: " + str(len(body)).encode() + b"\r\nConnection: close\r\n\r\n" + body)


async def test_dot_and_empty_path_segments_are_refused_before_routing(tmp_path):
    native = ConsoleNative()
    async with listening(tmp_path, native) as (console, base):
        status, session = await listener_sign_in(base, console, ALICE, ALICE_PASSWORD)
        assert status == 303
        before = len(native.requests)
        body = b'{"approval_id":"x","decision":"approve","unexpected":true}'
        for target in (b"/console/../host/toolkit/consent/decide?probe=1", b"/blobs/../healthz",
                       b"/console/./rpc", b"/console//rpc", b"/console/rpc/..", b"/host//toolkit/consent/decide",
                       b"/host/toolkit/./consent/decide"):
            assert await raw_request(console.port, raw_post(console, session, target, body, b"text/plain")) == 400
        assert len(native.requests) == before


async def test_consent_route_forms_on_the_listener(tmp_path):
    native = ConsoleNative()
    async with listening(tmp_path, native) as (console, base):
        status, session = await listener_sign_in(base, console, ALICE, ALICE_PASSWORD)
        assert status == 303
        body = b'{"approval_id":"x","decision":"deny"}'
        for target, forwarded in ((b"/host/toolkit/consent/decide", True), (b"/host/toolkit/consent%2Fdecide", True),
                                  (b"/host/toolkit/consent/decide/", False), (b"/HOST/toolkit/consent/decide", False)):
            before = len(native.requests)
            answered = await raw_request(console.port, raw_post(console, session, target, body))
            if forwarded:
                assert answered == 200 and len(native.requests) == before + 1
                request = native.requests[-1]
                assert request.url.raw_path == b"/host/toolkit/consent/decide"
                assert request.headers["origin"] == console.origin
            else:
                assert answered == 404 and len(native.requests) == before


# P3-5: the logout page says what is unconfirmed, all of it.
async def test_logout_reports_both_unconfirmed_parts(tmp_path, monkeypatch):
    def refused(identity):
        return httpx.Response(200, json={"jsonrpc": "2.0", "id": identity,
            "error": {"code": -32603, "message": "synthetic console answer"}})
    native = ConsoleNative(sign_out=refused)
    console = front(tmp_path, native)
    app = console.application(NATIVE)

    async def unconfirmed(login, reason):
        raise RuntimeError("Console subscription cleanup is unconfirmed.")
    monkeypatch.setattr(console, "_finish_login", unconfirmed)
    try:
        async with browser(app, console) as client:
            await sign_in(client, console, ALICE, ALICE_PASSWORD)
            response = await client.post("/logout", headers={"Origin": console.origin})
            assert response.status_code == 503
            assert "cleanup is unconfirmed" in response.text and "did not confirm" in response.text
            assert "within three minutes" in response.text
            assert console.sessions == {} and console.cookie not in client.cookies
    finally:
        # The unconfirmed cleanup stays reported at close, too.
        await asyncio.gather(console.close(), return_exceptions=True)


# P3-6: a frontend approval proof belongs to the login that prepared it.
async def test_a_frontend_approval_proof_belongs_to_its_login(tmp_path):
    from test_frontend_management import CONFIG, HOST, PATH, Owner
    owner = Owner()
    native = ConsoleNative()
    console = front(tmp_path, native)
    console.frontend_management = owner
    console.frontend_host = lambda: dict(HOST)
    console.frontend_private_fields = ("CONSOLE_LOGIN_PASSWORD",)
    app = console.application(NATIVE)
    try:
        async with browser(app, console) as first, browser(app, console) as second:
            await sign_in(first, console, ALICE, ALICE_PASSWORD)
            await sign_in(second, console, ALICE, ALICE_PASSWORD)
            prepared = await first.post(PATH + "/prepare", json={"configuration": CONFIG,
                "expected_revision": {"component": 7}}, headers={"Origin": console.origin})
            assert prepared.status_code == 200, prepared.text
            body = {"proposal": prepared.json()["proposal"],
                    "approval": {"approved": True, **prepared.json()["approval"]}}
            # The same account's other login cannot use this login's proof.
            assert (await second.post(PATH + "/submit", json=body, headers={"Origin": console.origin})).status_code == 403
            # After the login that prepared it ends, the proof approves nothing.
            assert (await first.post("/logout", headers={"Origin": console.origin})).status_code == 303
            assert (await second.post(PATH + "/submit", json=body, headers={"Origin": console.origin})).status_code == 403
            assert not any(request.approved for request in owner.calls)
    finally:
        await console.close()


# P3-7: signing in over a live session ends the earlier login.
async def test_signing_in_over_a_live_session_ends_the_earlier_login(tmp_path):
    native = ConsoleNative()
    console = front(tmp_path, native)
    app = console.application(NATIVE)
    try:
        async with browser(app, console) as shared_browser:
            await sign_in(shared_browser, console, ALICE, ALICE_PASSWORD)
            earlier = login_of(shared_browser, console)
            await sign_in(shared_browser, console, BOB, BOB_PASSWORD)
            later = login_of(shared_browser, console)
            assert later.principal == BOB and earlier.retired and earlier.session not in console.sessions
            for _ in range(200):
                if native.sign_outs:
                    break
                await asyncio.sleep(0.01)
            assert [claims["sid"] for claims in native.sign_outs] == [earlier.login_scope_id]
    finally:
        await console.close()


# P3-8: setup refuses an installation account that is a named account.
def test_setup_refuses_an_installation_account_that_is_a_named_account(generated_setup, monkeypatch, capsys):
    module, root = generated_setup
    named_console_source(root)
    env = root / ".env"
    env.write_text(f'CONSOLE_USER_ID="{OPERATOR}"\nCONSOLE_LOGIN_PASSWORD="{OPERATOR_PASSWORD}"\n')
    env.chmod(0o600)
    console_auth.save_console_accounts(root, named_accounts(ALICE), None, OPERATOR)
    users = iter([ALICE, "renamed-operator"])

    def answer(prompt):
        return next(users) if prompt.startswith("Console user") else ""
    monkeypatch.setattr("builtins.input", answer)
    monkeypatch.setattr(module.getpass, "getpass", lambda prompt: "")
    assert module.main() == 0
    assert "named account" in capsys.readouterr().out
    assert 'CONSOLE_USER_ID="renamed-operator"' in env.read_text()
    assert set(console_auth.load_console_accounts(root, "renamed-operator")) == {ALICE}


# Logout fences the consent route and the review too.
async def test_logout_fences_the_consent_and_review_routes(tmp_path, reviews):
    native = ConsoleNative(admins=(ALICE,))
    console = front(tmp_path, native, controller_review_factory=Review)
    app = console.application(NATIVE)
    try:
        async with browser(app, console) as alice:
            await sign_in(alice, console, ALICE, ALICE_PASSWORD)
            assert (await review_state(alice)).json()["status"] == "prompt"
            session = alice.cookies[console.cookie]
            assert (await alice.post("/logout", headers={"Origin": console.origin})).status_code == 303
            # The browser presents the ended login's cookie again.
            alice.cookies.set(console.cookie, session)
            before = len(native.requests)
            consent = await alice.post("/host/toolkit/consent/decide", json={"approval_id": "x", "decision": "deny"},
                                       headers={"Origin": console.origin})
            review = await answer_review(alice, console)
            assert consent.status_code == 401 and review.status_code == 401
            assert len(native.requests) == before and reviews[0].answers == []
    finally:
        await console.close()
