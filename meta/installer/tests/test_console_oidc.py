"""OIDC sign-in on the Forge front, against a local mock issuer.

The mock issuer serves discovery, a rotatable JWKS and a token endpoint that
checks the client's credentials and the PKCE verifier, and signs RS256 ID
tokens. The browser leg is simulated: the test follows the front's
authorization redirect, has the issuer sign a person in and follows the
callback. The front then mints its own ES256 tokens, under its own issuer,
for the console. The console-shaped native verifies them with exactly the
`auth_config` the front declares, under the console's rules for provider
"oidc": the key the token's kid names in the declared JWKS, the declared
issuer and audience, and a verified email on the allowlist. Meta's own
verifier checks the same configuration shape in
meta/crates/meerkat-meta/tests/jwt_rsa.rs. No live provider, network or
native process is used.
"""
import asyncio
import base64
from contextlib import AsyncExitStack
import hashlib
import json
import math
import os
from pathlib import Path
import re
import secrets
import stat
import sys
import time
from types import SimpleNamespace
from urllib.parse import parse_qsl, urlencode, urlsplit

import httpx
import jwt
import pytest
from cryptography.hazmat.primitives import serialization
from cryptography.hazmat.primitives.asymmetric import rsa

from launchkit.templates_v2 import console_auth
from launchkit.templates_v2.console_auth import AUDIENCE, ConsoleFront
from test_console_named_accounts import (ALICE, BOB, NATIVE, ConsoleNative, browser, clock,  # noqa: F401
    generated_setup, login_of, whoami)

ISSUER_URL = "https://login.example.test"
CLIENT_ID = "forge-console"
CLIENT_SECRET = "client-secret-canary-value-0123456789"
OPERATOR_EMAIL = "operator@example.test"
SUBJECT = "provider-subject-0001"
CANARIES = ("provider-access-canary", "provider-refresh-canary")
FRONT_ISSUER = re.compile(r"urn:forge-console:[a-f0-9]{32}")


def base64url(data):
    return base64.urlsafe_b64encode(data).rstrip(b"=").decode()


class MockIssuer:
    """Discovery, a rotatable JWKS and a code exchange with PKCE."""

    def __init__(self, *, auth_methods=("client_secret_basic", "client_secret_post"), discovery=None):
        self.keys = {"key-a": rsa.generate_private_key(public_exponent=65537, key_size=2048)}
        self.published, self.signing = ["key-a"], "key-a"
        self.auth_methods = auth_methods
        self.discovery = discovery or {}
        self.codes, self.token_requests, self.issued = {}, [], []
        self.jwks_fetches = 0
        self.tamper = False
        # Replaces the signature of every later ID token, given its claims.
        self.sign = None

    def rotate(self, kid, *, publish=True):
        self.keys[kid] = rsa.generate_private_key(public_exponent=65537, key_size=2048)
        if publish:
            self.published.append(kid)
        self.signing = kid

    def transport(self):
        return httpx.MockTransport(self.handle)

    def handle(self, request):
        path = request.url.path
        if path == "/.well-known/openid-configuration":
            return httpx.Response(200, json={"issuer": ISSUER_URL,
                "authorization_endpoint": ISSUER_URL + "/authorize", "token_endpoint": ISSUER_URL + "/token",
                "jwks_uri": ISSUER_URL + "/jwks", "response_types_supported": ["code"],
                "code_challenge_methods_supported": ["S256"],
                "token_endpoint_auth_methods_supported": list(self.auth_methods), **self.discovery})
        if path == "/jwks":
            self.jwks_fetches += 1
            keys = []
            for kid in self.published:
                key = jwt.algorithms.RSAAlgorithm.to_jwk(self.keys[kid].public_key(), as_dict=True)
                keys.append({**key, "kid": kid, "alg": "RS256", "use": "sig"})
            return httpx.Response(200, json={"keys": keys})
        if path == "/token" and request.method == "POST":
            self.token_requests.append(request)
            form = dict(parse_qsl(request.content.decode()))
            grant = self.codes.pop(form.get("code"), None)
            if "client_secret_basic" in self.auth_methods:
                basic = "Basic " + base64.b64encode(f"{CLIENT_ID}:{CLIENT_SECRET}".encode()).decode()
                client = request.headers.get("authorization") == basic and "client_secret" not in form
            else:
                client = (form.get("client_id") == CLIENT_ID and form.get("client_secret") == CLIENT_SECRET
                          and "authorization" not in request.headers)
            if (grant is None or not client or form.get("grant_type") != "authorization_code"
                    or form.get("redirect_uri") != grant["redirect_uri"]
                    or base64url(hashlib.sha256(form.get("code_verifier", "").encode()).digest()) != grant["challenge"]):
                return httpx.Response(400, json={"error": "invalid_grant"})
            token = self.id_token(grant)
            self.issued.append(token)
            return httpx.Response(200, json={"id_token": token, "access_token": CANARIES[0],
                "refresh_token": CANARIES[1], "token_type": "Bearer", "expires_in": 3600})
        return httpx.Response(404)

    def authorize(self, location, *, sub=SUBJECT, email=ALICE, verified=True, **claims):
        """The provider signs a person in and sends the browser back."""
        parsed = urlsplit(location)
        assert f"{parsed.scheme}://{parsed.netloc}{parsed.path}" == ISSUER_URL + "/authorize"
        query = dict(parse_qsl(parsed.query))
        code = secrets.token_urlsafe(16)
        self.codes[code] = {"redirect_uri": query["redirect_uri"], "challenge": query["code_challenge"],
            "nonce": query["nonce"], "sub": sub, "email": email, "verified": verified, "claims": claims}
        return query, query["redirect_uri"] + "?" + urlencode({"code": code, "state": query["state"]})

    def refuse(self, location, error="access_denied"):
        """The provider does not sign the person in."""
        query = dict(parse_qsl(urlsplit(location).query))
        return query["redirect_uri"] + "?" + urlencode({"error": error, "state": query["state"]})

    def id_token(self, grant):
        now = int(time.time())
        claims = {"iss": ISSUER_URL, "aud": CLIENT_ID, "sub": grant["sub"], "iat": now, "exp": now + 300,
                  "nonce": grant["nonce"]}
        if grant["email"] is not None:
            claims.update(email=grant["email"], email_verified=grant["verified"])
        claims.update(grant["claims"])
        if self.sign is not None:
            return self.sign(claims)
        token = jwt.encode(claims, self.keys[self.signing], algorithm="RS256", headers={"kid": self.signing})
        if self.tamper:
            head, body, signature = token.split(".")
            signature = ("A" if signature[0] != "A" else "B") + signature[1:]
            token = ".".join((head, body, signature))
        return token


class OidcNative(ConsoleNative):
    """The console's rules for provider "oidc", with the auth_config the front declares."""

    def claims(self, request):
        if self.auth.get("provider") != "oidc":
            return super().claims(request)
        header = request.headers.get("authorization", "")
        return self.verify(header[7:]) if header.startswith("Bearer ") else None

    def verify(self, token):
        try:
            head = jwt.get_unverified_header(token)
            alg, kid = head["alg"], head.get("kid")
            # The key the token's kid names, when its algorithm matches.
            keys = [key for key in self.auth["jwks_json"]["keys"]
                    if (kid is None or key.get("kid") == kid) and key.get("alg", alg) == alg]
            if alg not in ("RS256", "ES256") or not keys:
                return None
            claims = jwt.decode(token, jwt.PyJWK(keys[0], algorithm=alg).key, algorithms=[alg],
                issuer=self.auth["issuer"], audience=self.auth["audience"], leeway=30)
        except (jwt.PyJWTError, KeyError, TypeError, ValueError):
            return None
        # Only a verified email on the allowlist names the caller; `sub` never stands in for it.
        if claims.get("email_verified") is not True or claims.get("email") not in self.auth["email_allowlist"]:
            return None
        fence = self.fences.get((claims.get("sub"), claims.get("sid")))
        if fence is not None and claims.get("iat", 0) < fence + 30:
            return None
        return claims


def oidc_settings(issuer, *, allowed=(ALICE,)):
    return console_auth.ConsoleOidc(issuer=ISSUER_URL, client_id=CLIENT_ID, client_secret=CLIENT_SECRET,
        allowed_emails=tuple(allowed), transport=issuer.transport())


def oidc_front(root, native, issuer, *, allowed=(ALICE,), principal=OPERATOR_EMAIL, port=8855, **options):
    console = ConsoleFront(root, port, principal, None, runtime_namespace="meta", transport=native.transport(),
        oidc=oidc_settings(issuer, allowed=allowed), **options)
    native.auth = console.native_auth
    return console


async def start_sign_in(client, console, *, status=303):
    form = await client.get("/login")
    assert form.status_code == 200, form.text
    nonce = client.cookies[console.cookie + "_login"]
    started = await client.post("/login", data={"nonce": nonce}, headers={"Origin": console.origin})
    assert started.status_code == status, started.text
    return started.headers.get("location")


async def oidc_sign_in(client, console, issuer, *, status=200, **person):
    query, callback = issuer.authorize(await start_sign_in(client, console), **person)
    response = await client.get(callback)
    assert response.status_code == status, response.text
    return query, callback, response


def front_claims(request, console):
    """The console token the front minted for this request, verified with its declared key."""
    token = request.headers["authorization"].removeprefix("Bearer ")
    key, = console.native_auth["jwks_json"]["keys"]
    assert jwt.get_unverified_header(token) == {"alg": "ES256", "kid": key["kid"], "typ": "JWT"}
    return jwt.decode(token, jwt.PyJWK(key).key, algorithms=["ES256"], issuer=console.native_auth["issuer"],
                      audience=AUDIENCE)


def front_key(root):
    return serialization.load_pem_private_key((root / ".mobkit" / console_auth.OIDC_KEY_FILE).read_bytes(), None)


# Row 1: a person signs in at the provider with the code flow (PKCE, state,
# nonce) and reaches the console as the provider's person, through the
# front's own token.
async def test_oidc_sign_in_reaches_the_console_as_the_providers_person(tmp_path):
    native, issuer = OidcNative(), MockIssuer()
    console = oidc_front(tmp_path, native, issuer)
    app = console.application(NATIVE)
    try:
        async with browser(app, console) as alice:
            page = await alice.get("/login")
            assert "identity provider" in page.text and 'type="password"' not in page.text
            query, _, landing = await oidc_sign_in(alice, console, issuer)
            assert query["response_type"] == "code" and query["client_id"] == CLIENT_ID
            assert query["redirect_uri"] == console.origin + console_auth.OIDC_CALLBACK_PATH
            assert {"openid", "email"} <= set(query["scope"].split()) and query["code_challenge_method"] == "S256"
            assert len(query["state"]) >= 32 and len(query["nonce"]) >= 32 and query["state"] != query["nonce"]
            assert len(query["code_challenge"]) == 43
            # One exchange, with the client's credentials and the PKCE verifier: the issuer checks both.
            assert len(issuer.token_requests) == 1
            # A same-site landing page carries the person on to the console.
            assert landing.headers["cache-control"] == "no-store"
            assert landing.headers["referrer-policy"] == "no-referrer"
            assert "url=/console" in landing.text
            # No provider token and no client secret reach the browser: only the front's own session.
            exposed = " ".join([landing.text, *alice.cookies.values(), *landing.headers.values()])
            assert all(value not in exposed for value in (*CANARIES, *issuer.issued, CLIENT_SECRET))
            actor = (await whoami(alice, console))["actor"]
            assert actor == {"kind": "person", "principal": ALICE, "assurance": "person_authenticated"}
            login = login_of(alice, console)
            assert login.person and login.principal == ALICE
            # The front signs as itself and names the provider's person as the subject.
            claims = front_claims(native.requests[-1], console)
            assert FRONT_ISSUER.fullmatch(claims["iss"]) and claims["iss"] != ISSUER_URL
            assert json.loads(claims["sub"]) == [ISSUER_URL, SUBJECT]
            assert claims["sub"] == console_auth.oidc_subject(ISSUER_URL, SUBJECT)
            assert claims["email"] == ALICE and claims["email_verified"] is True and claims["aud"] == AUDIENCE
            assert claims["sid"] == login.login_scope_id and 0 < claims["exp"] - claims["iat"] <= 120
            assert (await alice.get("/console")).status_code == 200
    finally:
        await console.close()


# Row 2: the console trusts the front's own issuer and one public key, both
# recorded once per installation in private state, never derived from an
# address, and never the provider's.
def test_oidc_front_declares_its_own_issuer_and_key_to_the_console(tmp_path):
    issuer = MockIssuer()
    console = oidc_front(tmp_path, OidcNative(), issuer, allowed=(ALICE, BOB, OPERATOR_EMAIL.upper()))
    auth = console.native_auth
    key, = auth["jwks_json"]["keys"]
    assert auth == {"provider": "oidc", "issuer": auth["issuer"], "audience": AUDIENCE,
        "jwks_json": {"keys": [key]}, "email_allowlist": [OPERATOR_EMAIL, ALICE, BOB], "accounts": "personal"}
    assert FRONT_ISSUER.fullmatch(auth["issuer"])
    assert set(key) == {"kty", "crv", "x", "y", "kid", "alg", "use"}
    assert (key["kty"], key["crv"], key["alg"], key["use"]) == ("EC", "P-256", "ES256", "sig")
    # The kid is the key's RFC 7638 thumbprint.
    members = json.dumps({"crv": key["crv"], "kty": key["kty"], "x": key["x"], "y": key["y"]},
                         separators=(",", ":"), sort_keys=True)
    assert key["kid"] == base64url(hashlib.sha256(members.encode()).digest())
    records = [tmp_path / ".mobkit" / name for name in (console_auth.OIDC_ISSUER_FILE, console_auth.OIDC_KEY_FILE)]
    for path in records:
        info = path.lstat()
        assert stat.S_ISREG(info.st_mode) and stat.S_IMODE(info.st_mode) == 0o600 and info.st_uid == os.getuid()
    assert records[0].read_text() == auth["issuer"] + "\n"
    # Another port and a public origin keep the same issuer and key.
    moved = oidc_front(tmp_path, OidcNative(), issuer, port=8866, public_origin="https://console.example.test")
    assert moved.native_auth["issuer"] == auth["issuer"] and moved.native_auth["jwks_json"] == auth["jwks_json"]
    # Another installation has its own.
    other_root = tmp_path / "other"
    other_root.mkdir()
    other = oidc_front(other_root, OidcNative(), issuer)
    assert other.native_auth["issuer"] != auth["issuer"]
    assert other.native_auth["jwks_json"] != auth["jwks_json"]
    # The signing key stays in private state: the console gets only the public key.
    assert "d" not in key and front_key(tmp_path).private_numbers().private_value
    # A malformed issuer record is refused, never replaced.
    records[0].write_text("https://console.example.test\n")
    with pytest.raises(ValueError, match="issuer"):
        oidc_front(tmp_path, OidcNative(), issuer)


@pytest.mark.parametrize("killed_in", ["key", "removal"])
def test_oidc_front_starts_after_a_start_killed_while_recording_its_identity(tmp_path, killed_in):
    """The front records its issuer and key through private temporary files.

    Killed while writing the key, a start leaves an unpublished temporary
    key: a later start leaves a young one to the start that may still be
    writing it and removes an old one. Killed after publishing the issuer,
    the temporary file is a second link to the record; the next start
    removes it, so the record is again the one private file it must be.
    """
    import signal
    import subprocess
    root = tmp_path / "application"
    root.mkdir()
    stopped = "console_auth._new_signing_key" if killed_in == "key" else "pathlib.Path.unlink"
    script = tmp_path / "killed_record.py"
    script.write_text(
        "import os, pathlib, signal, sys\n"
        f"sys.path.insert(0, {str(Path(__file__).parents[1])!r})\n"
        "from launchkit.templates_v2 import console_auth\n"
        "def killed(*args, **kwargs):\n"
        "    os.kill(os.getpid(), signal.SIGKILL)\n"
        f"{stopped} = killed\n"
        f"console_auth._front_identity({str(root)!r})\n")
    result = subprocess.run([sys.executable, str(script)], capture_output=True, timeout=60)
    assert result.returncode == -signal.SIGKILL, result.stderr
    state = root / ".mobkit"
    recorded = (state / console_auth.OIDC_ISSUER_FILE).read_text()
    left, = state.glob(".console-oidc-*.pending")
    if killed_in == "key":
        assert not (state / console_auth.OIDC_KEY_FILE).exists()
        assert console_auth._front_identity(root)[0] + "\n" == recorded
        assert left.exists()
        hour_ago = time.time() - 3600
        os.utime(left, (hour_ago, hour_ago))
    else:
        assert left.stat().st_nlink == 2
    assert console_auth._front_identity(root)[0] + "\n" == recorded
    assert sorted(path.name for path in state.iterdir()) == sorted(
        [console_auth.OIDC_ISSUER_FILE, console_auth.OIDC_KEY_FILE])
    assert (state / console_auth.OIDC_ISSUER_FILE).stat().st_nlink == 1


# Row 3: the console accepts the front's own tokens and nothing else: not
# another key, not another issuer (the provider's included), not the
# provider's own ID token, not an unverified or missing email, and only
# the people on the list.
async def test_oidc_console_accepts_only_tokens_the_front_signed(tmp_path):
    native, issuer = OidcNative(), MockIssuer()
    console = oidc_front(tmp_path, native, issuer)
    other_root = tmp_path / "other"
    other_root.mkdir()
    other = oidc_front(other_root, OidcNative(), MockIssuer())
    app = console.application(NATIVE)
    try:
        async with browser(app, console) as alice:
            await oidc_sign_in(alice, console, issuer)
            assert (await alice.get("/console/experience")).status_code == 200
            minted = native.requests[-1].headers["authorization"].removeprefix("Bearer ")
        claims = native.verify(minted)
        assert claims is not None and claims["iss"] == console.native_auth["issuer"]
        kid = console.native_auth["jwks_json"]["keys"][0]["kid"]
        own, foreign = front_key(tmp_path), front_key(other_root)

        def signed(key, changes=(), *, key_id=kid, drop=()):
            body = {name: value for name, value in {**claims, **dict(changes)}.items() if name not in drop}
            return jwt.encode(body, key, algorithm="ES256", headers={"kid": key_id})
        assert native.verify(signed(own)) is not None
        assert native.verify(signed(foreign)) is None
        assert native.verify(signed(foreign, key_id=other.native_auth["jwks_json"]["keys"][0]["kid"])) is None
        assert native.verify(signed(own, {"iss": ISSUER_URL})) is None
        assert native.verify(signed(own, {"iss": other.native_auth["issuer"]})) is None
        provider_token = issuer.issued[-1]
        assert native.verify(provider_token) is None
        readdressed = jwt.encode({**jwt.decode(provider_token, options={"verify_signature": False}),
            "aud": AUDIENCE, "iss": console.native_auth["issuer"]}, issuer.keys["key-a"], algorithm="RS256",
            headers={"kid": kid})
        assert native.verify(readdressed) is None
        assert native.verify(signed(own, {"email_verified": False})) is None
        assert native.verify(signed(own, drop=("email", "email_verified"))) is None
        assert native.verify(signed(own, {"email": BOB})) is None
    finally:
        await console.close()
        await other.close()


@pytest.mark.parametrize("defect", ["signature", "audience", "expired", "issued-later", "issuer", "algorithm",
                                    "unsigned", "nonce", "party", "subject", "audiences", "not-yet-valid",
                                    "expiry-infinite", "expiry-nan", "issued-nan", "issued-minus-infinite"])
async def test_oidc_refuses_an_id_token_that_does_not_verify(tmp_path, defect):
    native, issuer = OidcNative(), MockIssuer()
    console = oidc_front(tmp_path, native, issuer)
    app = console.application(NATIVE)
    now = int(time.time())
    # Several audiences need an authorized party that names this client. A
    # time is a finite number: Python's json reads Infinity and NaN as well.
    claims = {"audience": {"aud": "another-client"}, "expired": {"exp": now - 600},
              "issued-later": {"iat": now + 600}, "issuer": {"iss": "https://other.example.test"},
              "nonce": {"nonce": "another-sign-in-nonce"},
              "party": {"aud": [CLIENT_ID, "another-client"], "azp": "another-client"},
              "subject": {"sub": "s" * 256}, "audiences": {"aud": [CLIENT_ID, "another-client"]},
              "not-yet-valid": {"nbf": now + 600}, "expiry-infinite": {"exp": math.inf},
              "expiry-nan": {"exp": math.nan}, "issued-nan": {"iat": math.nan},
              "issued-minus-infinite": {"iat": -math.inf}}.get(defect, {})
    if defect == "signature":
        issuer.tamper = True
    elif defect == "algorithm":
        # A token "signed" with the client secret as an HMAC key.
        issuer.sign = lambda body: jwt.encode(body, CLIENT_SECRET, algorithm="HS256", headers={"kid": "key-a"})
    elif defect == "unsigned":
        issuer.sign = lambda body: jwt.encode(body, None, algorithm="none", headers={"kid": "key-a"})
    try:
        async with browser(app, console) as alice:
            _, _, refused = await oidc_sign_in(alice, console, issuer, status=401, **claims)
            assert "Sign in again" in refused.text
            assert console.cookie not in alice.cookies and not console.sessions
            assert not native.requests
    finally:
        await console.close()


async def test_oidc_refuses_a_replayed_state_or_nonce(tmp_path):
    native, issuer = OidcNative(), MockIssuer()
    console = oidc_front(tmp_path, native, issuer, allowed=(ALICE, BOB))
    app = console.application(NATIVE)
    try:
        async with browser(app, console) as alice:
            query, callback, _ = await oidc_sign_in(alice, console, issuer)
            # The same callback again: its state was used once and is gone.
            replay = await alice.get(callback)
            assert replay.status_code == 400 and "Sign in again" in replay.text
            assert len(issuer.token_requests) == 1
        async with browser(app, console) as bob:
            # An ID token carrying the earlier, finished sign-in's nonce.
            await oidc_sign_in(bob, console, issuer, status=401, sub="provider-subject-0002", email=BOB,
                               nonce=query["nonce"])
            assert console.cookie not in bob.cookies
    finally:
        await console.close()


async def test_oidc_sign_in_is_bound_to_the_browser_that_started_it(tmp_path):
    native, issuer = OidcNative(), MockIssuer()
    console = oidc_front(tmp_path, native, issuer)
    app = console.application(NATIVE)
    try:
        async with browser(app, console) as alice, browser(app, console) as other:
            _, callback = issuer.authorize(await start_sign_in(alice, console))
            # Another browser that holds the link but not the binding cookie.
            refused = await other.get(callback)
            assert refused.status_code == 400 and console.cookie not in other.cookies
            # That attempt spent the state, and the code never reached the provider.
            assert (await alice.get(callback)).status_code == 400 and not issuer.token_requests
    finally:
        await console.close()


async def test_oidc_fetches_a_rotated_key_once_for_an_unknown_kid(tmp_path, clock):
    native, issuer = OidcNative(), MockIssuer()
    console = oidc_front(tmp_path, native, issuer, allowed=(ALICE, BOB))
    app = console.application(NATIVE)
    try:
        async with browser(app, console) as alice:
            await oidc_sign_in(alice, console, issuer)
        assert issuer.jwks_fetches == 1
        clock["now"] += console_auth.OIDC_JWKS_REFRESH_SECONDS + 1
        issuer.rotate("key-b")
        async with browser(app, console) as bob:
            await oidc_sign_in(bob, console, issuer, sub="provider-subject-0002", email=BOB)
        assert issuer.jwks_fetches == 2
        # A kid the provider never published: one bounded refetch, then none
        # within the window, and the sign-in is refused.
        clock["now"] += console_auth.OIDC_JWKS_REFRESH_SECONDS + 1
        issuer.rotate("key-unpublished", publish=False)
        async with browser(app, console) as alice:
            await oidc_sign_in(alice, console, issuer, status=401)
        assert issuer.jwks_fetches == 3
        async with browser(app, console) as alice:
            await oidc_sign_in(alice, console, issuer, status=401)
        assert issuer.jwks_fetches == 3
    finally:
        await console.close()


async def test_oidc_admits_only_listed_people_with_a_verified_email(tmp_path):
    native, issuer = OidcNative(), MockIssuer()
    console = oidc_front(tmp_path, native, issuer)
    app = console.application(NATIVE)
    try:
        async with browser(app, console) as person:
            _, _, refused = await oidc_sign_in(person, console, issuer, status=403, verified=False)
            assert "verified email" in refused.text and ALICE in refused.text
        async with browser(app, console) as person:
            _, _, refused = await oidc_sign_in(person, console, issuer, status=403, verified="true")
            assert "verified email" in refused.text
        async with browser(app, console) as person:
            _, _, refused = await oidc_sign_in(person, console, issuer, status=403, email=None)
            # Without an email, the front's own page names the person by the provider's subject.
            assert "verified email" in refused.text and SUBJECT in refused.text
        async with browser(app, console) as person:
            _, _, refused = await oidc_sign_in(person, console, issuer, status=403, email=BOB)
            assert BOB in refused.text and console.cookie not in person.cookies
        # Nobody refused reached the console.
        assert not native.requests and not console.sessions
        async with browser(app, console) as operator:
            # The operator is always admitted; a differently cased address is the same person.
            await oidc_sign_in(operator, console, issuer, sub="provider-subject-0003", email=OPERATOR_EMAIL.upper())
            assert login_of(operator, console).principal == OPERATOR_EMAIL
            assert (await whoami(operator, console))["actor"]["principal"] == OPERATOR_EMAIL
    finally:
        await console.close()


async def test_oidc_logout_signs_the_login_out_of_the_console(tmp_path):
    native, issuer = OidcNative(), MockIssuer()
    console = oidc_front(tmp_path, native, issuer)
    app = console.application(NATIVE)
    try:
        async with browser(app, console) as alice:
            await oidc_sign_in(alice, console, issuer)
            login = login_of(alice, console)
            assert (await alice.get("/console/experience")).status_code == 200
            held = native.requests[-1].headers["authorization"]
            response = await alice.post("/logout", headers={"Origin": console.origin})
            assert response.status_code == 303 and response.headers["location"] == "/login"
            signed_out, = native.sign_outs
            assert signed_out["iss"] == console.native_auth["issuer"]
            assert signed_out["sub"] == console_auth.oidc_subject(ISSUER_URL, SUBJECT)
            assert signed_out["sid"] == login.login_scope_id and login.retired and not console.sessions
            # The token the console already held for that login is refused now.
            replay = httpx.Request("GET", NATIVE + "/console/experience", headers={"Authorization": held})
            assert native.handle(replay).status_code == 401
            assert (await alice.get("/console/experience")).status_code == 401
    finally:
        await console.close()


async def test_oidc_host_health_probe_names_the_operator_and_no_person(tmp_path):
    native, issuer = OidcNative(), MockIssuer()
    console = oidc_front(tmp_path, native, issuer)
    app = console.application(NATIVE)
    try:
        async with browser(app, console) as probe:
            bearer = console_auth._token(console.key, OPERATOR_EMAIL, scope="host-health")
            response = await probe.get("/console/experience", headers={"Authorization": "Bearer " + bearer})
            assert response.status_code == 200
            claims = front_claims(native.requests[-1], console)
            assert claims["email"] == OPERATOR_EMAIL and "sub" not in claims and "sid" not in claims
    finally:
        await console.close()


async def test_oidc_limits_pending_sign_ins_and_failures_per_address(tmp_path, clock, monkeypatch):
    monkeypatch.setattr(console_auth, "LOGIN_FORMS_PER_CLIENT", 3)
    monkeypatch.setattr(console_auth, "LOGIN_FORMS", 5)
    native, issuer = OidcNative(), MockIssuer()
    console = oidc_front(tmp_path, native, issuer)
    app = console.application(NATIVE)

    def pending(address):
        return [state for state, entry in console._oidc_pending.items() if entry.client == address]
    try:
        async with AsyncExitStack() as stack:
            # Four people behind one relay share its address. A full address
            # refuses no new sign-in: its oldest pending sign-in goes, so one
            # caller there cannot hold everyone else's off.
            relay = [await stack.enter_async_context(browser(app, console, "192.0.2.10")) for _ in range(4)]
            started = [await start_sign_in(person, console) for person in relay]
            assert len(pending("192.0.2.10")) == 3
            evicted = await relay[0].get(issuer.authorize(started[0])[1])
            assert evicted.status_code == 400 and "expired or was already used" in evicted.text
            assert (await relay[3].get(issuer.authorize(started[3])[1])).status_code == 200
            # A full table drops its oldest pending sign-in, whoever started it.
            oldest = pending("192.0.2.10")[0]
            others = [await stack.enter_async_context(browser(app, console, f"192.0.2.{20 + index}"))
                      for index in range(4)]
            for person in others:
                await start_sign_in(person, console)
            assert len(console._oidc_pending) == 5 and oldest not in console._oidc_pending
            # Sign-in forms behave alike: the address's oldest form expires.
            forms = [await stack.enter_async_context(browser(app, console, "192.0.2.30")) for _ in range(4)]
            for person in forms:
                assert (await person.get("/login")).status_code == 200
            stale = await forms[0].post("/login", data={"nonce": forms[0].cookies[console.cookie + "_login"]},
                                        headers={"Origin": console.origin})
            assert stale.status_code == 403 and "expired" in stale.text
            fresh = await forms[3].post("/login", data={"nonce": forms[3].cookies[console.cookie + "_login"]},
                                        headers={"Origin": console.origin})
            assert fresh.status_code == 303
        async with browser(app, console, "192.0.2.40") as failing, browser(app, console, "192.0.2.50") as fine:
            issuer.tamper = True
            for _ in range(console_auth.FAILURES_PER_CLIENT):
                await oidc_sign_in(failing, console, issuer, status=401)
            issuer.tamper = False
            # The address is held before it reaches the provider again.
            exchanges = len(issuer.token_requests)
            await start_sign_in(failing, console, status=429)
            assert len(issuer.token_requests) == exchanges
            await oidc_sign_in(fine, console, issuer)
            clock["now"] += console_auth.FAILURE_WINDOW_SECONDS + 1
            await oidc_sign_in(failing, console, issuer)
    finally:
        await console.close()


async def test_oidc_signing_in_again_ends_the_browsers_earlier_login(tmp_path):
    native, issuer = OidcNative(), MockIssuer()
    console = oidc_front(tmp_path, native, issuer, allowed=(ALICE, BOB))
    app = console.application(NATIVE)
    try:
        async with browser(app, console) as shared_browser:
            await oidc_sign_in(shared_browser, console, issuer)
            earlier = login_of(shared_browser, console)
            await oidc_sign_in(shared_browser, console, issuer, sub="provider-subject-0002", email=BOB)
            later = login_of(shared_browser, console)
            assert later.principal == BOB and earlier.retired and earlier.session not in console.sessions
            for _ in range(200):
                if native.sign_outs:
                    break
                await asyncio.sleep(0.01)
            assert [claims["sid"] for claims in native.sign_outs] == [earlier.login_scope_id]
    finally:
        await console.close()


async def test_oidc_reports_a_provider_refusal_without_an_exchange(tmp_path):
    native, issuer = OidcNative(), MockIssuer()
    console = oidc_front(tmp_path, native, issuer)
    app = console.application(NATIVE)
    try:
        async with browser(app, console) as alice:
            refused = await alice.get(issuer.refuse(await start_sign_in(alice, console)))
            assert refused.status_code == 401 and "access_denied" in refused.text and "Sign in again" in refused.text
            assert not issuer.token_requests and console.cookie not in alice.cookies
            # A provider error text that is not a plain code is not repeated.
            denied = await alice.get(issuer.refuse(await start_sign_in(alice, console), "<script>x</script>"))
            assert denied.status_code == 401 and "<script>" not in denied.text
    finally:
        await console.close()


async def test_oidc_uses_the_client_authentication_the_provider_supports(tmp_path):
    native, issuer = OidcNative(), MockIssuer(auth_methods=("client_secret_post",))
    console = oidc_front(tmp_path, native, issuer)
    app = console.application(NATIVE)
    try:
        async with browser(app, console) as alice:
            await oidc_sign_in(alice, console, issuer)
            request, = issuer.token_requests
            assert "authorization" not in request.headers
    finally:
        await console.close()


@pytest.mark.parametrize("discovery", [{"issuer": "https://other.example.test"},
                                       {"token_endpoint": "http://login.example.test/token"},
                                       {"code_challenge_methods_supported": ["plain"]},
                                       {"token_endpoint_auth_methods_supported": ["private_key_jwt"]}])
async def test_oidc_refuses_a_provider_it_cannot_use_safely(tmp_path, discovery):
    native, issuer = OidcNative(), MockIssuer(discovery=discovery)
    console = oidc_front(tmp_path, native, issuer)
    app = console.application(NATIVE)
    try:
        async with browser(app, console) as alice:
            # The sign-in page names the provider's authorization endpoint in
            # its policy, so it is offered only for a usable provider.
            page = await alice.get("/login")
            assert page.status_code == 503 and "identity provider" in page.text and "<form" not in page.text
            assert console.cookie + "_login" not in alice.cookies
            # A provider that changes after the page was served is refused at the start.
            issuer.discovery = {}
            form = await alice.get("/login")
            issuer.discovery = discovery
            console._oidc_discovery = None
            nonce = alice.cookies[console.cookie + "_login"]
            started = await alice.post("/login", data={"nonce": nonce}, headers={"Origin": console.origin})
            assert started.status_code == 503 and "identity provider" in started.text
            assert "location" not in started.headers and form.status_code == 200
    finally:
        await console.close()


# The sign-in page's form posts to the front, which answers with a redirect to
# the identity provider. Chromium applies form-action to the redirects of a
# form submission too, so the page admits the front and exactly the origin of
# the provider's authorization endpoint, nothing wider.
@pytest.mark.parametrize("endpoint, origin", [
    (ISSUER_URL + "/authorize", "https://login.example.test"),
    ("https://auth.example.test:8443/oauth2/v2/authorize?tenant=a", "https://auth.example.test:8443")])
async def test_oidc_sign_in_page_lets_its_form_go_on_to_the_provider_only(tmp_path, endpoint, origin):
    native, issuer = OidcNative(), MockIssuer(discovery={"authorization_endpoint": endpoint})
    console = oidc_front(tmp_path, native, issuer)
    app = console.application(NATIVE)
    try:
        async with browser(app, console) as alice:
            page = await alice.get("/login")
            again = await alice.post("/login", data={"nonce": "expired-form"}, headers={"Origin": console.origin})
            assert (page.status_code, again.status_code) == (200, 403)
            for response in (page, again):
                directives = [part.strip() for part in response.headers["content-security-policy"].split(";")]
                assert [part for part in directives if part.startswith("form-action")] == [
                    f"form-action 'self' {origin}"]
                assert directives == ["default-src 'none'", "style-src 'unsafe-inline'",
                                      f"form-action 'self' {origin}", "frame-ancestors 'none'"]
            started = await start_sign_in(alice, console)
            assert started.startswith(endpoint.split("?")[0] + "?")
    finally:
        await console.close()


# A provider-signed ID token the front cannot read as one is a bad answer
# from the provider: a typed 502, never a 500.
@pytest.mark.parametrize("content", ["nonce", "algorithm"])
async def test_oidc_answers_malformed_provider_content_as_a_bad_gateway(tmp_path, content):
    native, issuer = OidcNative(), MockIssuer()
    claims = {}
    if content == "nonce":
        # A lone surrogate: valid JSON, not text.
        claims = {"nonce": "\ud800"}
    else:
        def sign(body):
            _, payload, signature = jwt.encode(body, issuer.keys["key-a"], algorithm="RS256",
                                               headers={"kid": "key-a"}).split(".")
            header = base64url(json.dumps({"alg": ["RS256"], "kid": "key-a", "typ": "JWT"}).encode())
            return ".".join((header, payload, signature))
        issuer.sign = sign
    console = oidc_front(tmp_path, native, issuer)
    app = console.application(NATIVE)
    try:
        async with browser(app, console) as alice:
            _, _, answer = await oidc_sign_in(alice, console, issuer, status=502, **claims)
            assert "could not read" in answer.text and "Sign in again" in answer.text
            assert console._oidc_exchanges == 0 and not console.sessions and not native.requests
    finally:
        await console.close()


# A verified address that only lowercases to a listed one is another person:
# the front folds the case of ASCII addresses only.
async def test_oidc_refuses_an_email_that_only_case_folds_to_a_listed_one(tmp_path):
    kate = "kate@example.test"
    # U+212A KELVIN SIGN first: str.lower() turns it into an ASCII "k".
    kelvin = "\u212aate@example.test"
    assert kelvin.lower() == kate and not kelvin.isascii()
    native, issuer = OidcNative(), MockIssuer()
    console = oidc_front(tmp_path, native, issuer, allowed=(kate,))
    app = console.application(NATIVE)
    try:
        async with browser(app, console) as person:
            await oidc_sign_in(person, console, issuer, status=403, email=kelvin)
            assert console.cookie not in person.cookies and not console.sessions
        async with browser(app, console) as listed:
            await oidc_sign_in(listed, console, issuer, sub="kate-subject", email="Kate@example.test")
            assert login_of(listed, console).principal == kate
    finally:
        await console.close()


# Code exchanges at the provider are capped across every sign-in: one more
# answer is told to try again, and its state stays usable.
async def test_oidc_caps_code_exchanges_at_the_provider(tmp_path):
    native, issuer = OidcNative(), MockIssuer()
    release, entered = asyncio.Event(), []
    answer = issuer.handle

    async def handle(request):
        if request.url.path == "/token":
            entered.append(request)
            await release.wait()
        return answer(request)
    issuer.transport = lambda: httpx.MockTransport(handle)
    people = [f"person-{index}@example.test" for index in range(console_auth.OIDC_EXCHANGES + 1)]
    console = oidc_front(tmp_path, native, issuer, allowed=tuple(people))
    app = console.application(NATIVE)
    tasks = []
    try:
        async with AsyncExitStack() as stack:
            callbacks = []
            for index, email in enumerate(people):
                client = await stack.enter_async_context(browser(app, console, f"192.0.2.{index + 1}"))
                _, callback = issuer.authorize(await start_sign_in(client, console), sub=f"subject-{index}",
                                               email=email)
                callbacks.append((client, callback))
            tasks = [asyncio.create_task(client.get(callback)) for client, callback in callbacks[:-1]]
            for _ in range(500):
                if len(entered) == console_auth.OIDC_EXCHANGES:
                    break
                await asyncio.sleep(0.01)
            assert len(entered) == console_auth.OIDC_EXCHANGES
            last, last_callback = callbacks[-1]
            # Without the cap this exchange would wait at the provider too.
            refused = await asyncio.wait_for(last.get(last_callback), 10)
            assert refused.status_code == 429
            release.set()
            assert [response.status_code for response in await asyncio.gather(*tasks)] == [200] * len(tasks)
            assert (await last.get(last_callback)).status_code == 200
    finally:
        release.set()
        await asyncio.gather(*tasks, return_exceptions=True)
        await console.close()


class AlglessIssuer(MockIssuer):
    """A provider that publishes its keys without "alg", as some do."""

    def handle(self, request):
        response = super().handle(request)
        if request.url.path == "/jwks":
            keys = [{name: value for name, value in key.items() if name != "alg"} for key in response.json()["keys"]]
            return httpx.Response(200, json={"keys": keys})
        return response


# With keys that name no algorithm, the front's own algorithm list alone
# refuses an HMAC token keyed by the client secret and an unsigned one.
@pytest.mark.parametrize("algorithm, key", [("HS256", CLIENT_SECRET), ("none", None)])
async def test_oidc_refuses_hmac_and_unsigned_tokens_when_keys_name_no_algorithm(tmp_path, algorithm, key):
    native, issuer = OidcNative(), AlglessIssuer()
    issuer.sign = lambda body: jwt.encode(body, key, algorithm=algorithm, headers={"kid": "key-a"})
    console = oidc_front(tmp_path, native, issuer)
    app = console.application(NATIVE)
    try:
        async with browser(app, console) as alice:
            await oidc_sign_in(alice, console, issuer, status=401)
            assert not console.sessions and not native.requests
    finally:
        await console.close()


# The provider calls of one sign-in step (discovery, the code exchange and the
# keys) share one deadline: each call within any limit of its own, together
# past it, still ends the step with a typed timeout.
async def test_oidc_sign_in_meets_one_overall_deadline_at_the_provider(tmp_path, monkeypatch):
    monkeypatch.setattr(console_auth, "OIDC_SIGN_IN_SECONDS", 0.6, raising=False)
    native, issuer = OidcNative(), MockIssuer()
    delays = {}
    answer = issuer.handle

    async def handle(request):
        await asyncio.sleep(delays.get(request.url.path, 0))
        return answer(request)
    issuer.transport = lambda: httpx.MockTransport(handle)
    console = oidc_front(tmp_path, native, issuer)
    app = console.application(NATIVE)
    try:
        async with browser(app, console) as alice:
            started = await start_sign_in(alice, console)
            delays.update({"/token": 0.4, "/jwks": 0.4})
            late = await alice.get(issuer.authorize(started)[1])
            assert late.status_code == 504 and "did not answer in time" in late.text
            assert console._oidc_exchanges == 0 and not console.sessions and not native.requests
            # The page for a provider whose discovery does not answer in time is typed too.
            console._oidc_discovery = None
            delays.update({"/.well-known/openid-configuration": 1.0})
            page = await alice.get("/login")
            assert page.status_code == 504 and "did not answer in time" in page.text
    finally:
        await console.close()


# The issuer URL is checked whole where it is declared, so no sign-in fails
# on it later.
@pytest.mark.parametrize("issuer_url", ["https://login.example.test:abc", "https://login.example.test:99999",
                                        "https://login.example.test:0", "https://login.example.test:",
                                        "https://login;example.test", "https://login.example.test:0443"])
def test_oidc_issuer_url_is_checked_whole_where_it_is_declared(issuer_url):
    from launchkit.templates_v2.console_configuration import console_authentication
    with pytest.raises(ValueError, match="OIDC issuer"):
        console_authentication(oidc_source(issuer=issuer_url), oidc_fields())
    with pytest.raises(ValueError, match="OIDC issuer"):
        console_auth.ConsoleOidc(issuer=issuer_url, client_id=CLIENT_ID, client_secret=CLIENT_SECRET)
    assert console_authentication(oidc_source(issuer="https://login.example.test:8443/realms/forge"),
                                  oidc_fields()) == "oidc"


def test_oidc_subject_encodes_the_providers_person_injectively():
    pairs = [(ISSUER_URL, SUBJECT), ("https://other.example.test", SUBJECT), (ISSUER_URL, "provider-subject-0002"),
             ("https://a.example.test", "b|c"), ("https://a.example.test|b", "c"),
             ("https://a.example.test", 'b","c'), ('https://a.example.test","b', "c"),
             ("https://a.example.test/", "b"), ("https://a.example.test", "/b")]
    encoded = [console_auth.oidc_subject(iss, sub) for iss, sub in pairs]
    assert len(set(encoded)) == len(pairs)
    assert [tuple(json.loads(value)) for value in encoded] == pairs
    assert all(value.isascii() and value.isprintable() for value in encoded)


class Enrollment:
    """An inert native enrollment bound to the labels it is given."""

    def __init__(self, context, *, issuer=None, subject=None):
        self.context = context
        self.issuer = context.issuer if issuer is None else issuer
        self.subject = context.subject if subject is None else subject
        self.login_scope_id, self.expires_at_ms = context.login_scope_id, context.expires_at_ms

    def is_current(self):
        return self.context.is_current() is True

    def start(self):
        pass

    def revoke(self):
        pass

    async def ready(self):
        return self

    async def retire(self, reason):
        return None

    async def close(self):
        return None


@pytest.mark.parametrize("labels", ["login", "account"])
async def test_oidc_console_login_enrollment_names_the_fronts_issuer_and_person(tmp_path, labels):
    native, issuer = OidcNative(), MockIssuer()
    console = oidc_front(tmp_path, native, issuer)
    enrolled = []

    def factory(context):
        owner = (Enrollment(context) if labels == "login"
                 else Enrollment(context, issuer=console_auth.ISSUER, subject=context.principal))
        enrolled.append(owner)
        return owner
    console.console_login_factory = factory
    app = console.application(NATIVE)
    try:
        async with browser(app, console) as alice:
            await oidc_sign_in(alice, console, issuer)
            response = await alice.get("/console/experience")
            owner, = enrolled
            context = owner.context
            assert (context.principal, context.issuer, context.subject) == (
                ALICE, console.native_auth["issuer"], console_auth.oidc_subject(ISSUER_URL, SUBJECT))
            if labels == "login":
                assert response.status_code == 200
                claims = front_claims(native.requests[-1], console)
                assert (claims["iss"], claims["sub"]) == (owner.issuer, owner.subject)
            else:
                # Labels naming the account, not this login's token issuer and subject, are not this login's.
                assert response.status_code == 503 and not native.requests
    finally:
        await console.close()


def oidc_fields(secret="CONSOLE_OIDC_CLIENT_SECRET"):
    return [{"key": "CONSOLE_USER_ID", "label": "Console operator email", "secret": False, "required": True},
            {"key": secret, "label": "OIDC client secret", "secret": True, "required": True},
            {"key": "CONSOLE_ALLOWED_EMAILS", "label": "People who may sign in", "secret": False, "required": False}]


def oidc_source(**changes):
    value = {"authentication": "oidc", "issuer": ISSUER_URL, "client_id": CLIENT_ID,
             "client_secret": {"env": "CONSOLE_OIDC_CLIENT_SECRET"}}
    value.update(changes)
    return value


def test_oidc_source_references_its_client_secret_and_never_holds_it():
    from launchkit.templates_v2.console_configuration import console_authentication
    assert console_authentication(oidc_source(), oidc_fields()) == "oidc"
    assert console_authentication(oidc_source(client_secret={"file": "/run/secrets/oidc-client"}),
                                  oidc_fields()[:1]) == "oidc"
    # A loopback issuer may use plain HTTP, for a provider on this computer.
    assert console_authentication(oidc_source(issuer="http://127.0.0.1:8180/realms/forge"), oidc_fields()) == "oidc"
    fields = oidc_fields()
    for changed, declared in [
            (oidc_source(client_secret=CLIENT_SECRET), fields),
            (oidc_source(client_secret={"env": "UNDECLARED_SECRET"}), fields),
            (oidc_source(client_secret={"env": "CONSOLE_USER_ID"}), fields),
            (oidc_source(client_secret={"env": "CONSOLE_OIDC_CLIENT_SECRET"}),
             [fields[0], {**fields[1], "default": "source-default"}, fields[2]]),
            (oidc_source(client_secret={"file": "relative/secret"}), fields),
            (oidc_source(client_secret={"file": "/run/../secret"}), fields),
            (oidc_source(client_secret={"env": "CONSOLE_OIDC_CLIENT_SECRET", "file": "/run/x"}), fields),
            (oidc_source(issuer="http://login.example.test"), fields),
            (oidc_source(issuer=ISSUER_URL + "?tenant=x"), fields),
            (oidc_source(issuer=ISSUER_URL + "#x"), fields),
            (oidc_source(issuer="https://user:pass@login.example.test"), fields),
            (oidc_source(client_id=""), fields),
            (oidc_source(client_id="two words"), fields),
            ({**oidc_source(), "scope": "openid"}, fields),
            ({"authentication": "oidc"}, fields),
            (oidc_source(), fields[1:]),
            (oidc_source(), [{**fields[0], "default": OPERATOR_EMAIL}, *fields[1:]]),
            (oidc_source(), [*fields[:2], {**fields[2], "secret": True}])]:
        with pytest.raises(ValueError):
            console_authentication(changed, declared)


def write_oidc_source(root, source=None):
    (root / "application").mkdir(parents=True, exist_ok=True)
    (root / "application/console.json").write_text(json.dumps(source or oidc_source()))


def test_oidc_settings_read_the_secret_by_reference_and_the_people_to_admit(tmp_path):
    root = tmp_path / "application-root"
    write_oidc_source(root)
    environ = {"CONSOLE_USER_ID": OPERATOR_EMAIL, "CONSOLE_OIDC_CLIENT_SECRET": CLIENT_SECRET,
               "CONSOLE_ALLOWED_EMAILS": f"{ALICE}, {BOB}\n{ALICE}"}
    settings = console_auth.console_oidc_settings(root, oidc_fields(), environ)
    assert settings.issuer == ISSUER_URL and settings.client_id == CLIENT_ID
    assert settings.client_secret == CLIENT_SECRET and settings.allowed_emails == (ALICE, BOB)
    assert CLIENT_SECRET not in repr(settings)
    secret = tmp_path / "oidc-client-secret"
    secret.write_text(CLIENT_SECRET + "\n")
    secret.chmod(0o600)
    write_oidc_source(root, oidc_source(client_secret={"file": str(secret)}))
    only_operator = {"CONSOLE_USER_ID": OPERATOR_EMAIL}
    assert console_auth.console_oidc_settings(root, oidc_fields()[:1], only_operator).client_secret == CLIENT_SECRET
    secret.chmod(0o644)
    with pytest.raises(ValueError, match="secret file"):
        console_auth.console_oidc_settings(root, oidc_fields()[:1], only_operator)
    link = tmp_path / "linked-secret"
    link.symlink_to(secret)
    secret.chmod(0o600)
    write_oidc_source(root, oidc_source(client_secret={"file": str(link)}))
    with pytest.raises(ValueError, match="secret file"):
        console_auth.console_oidc_settings(root, oidc_fields()[:1], only_operator)
    write_oidc_source(root)
    with pytest.raises(ValueError, match="CONSOLE_OIDC_CLIENT_SECRET"):
        console_auth.console_oidc_settings(root, oidc_fields(), only_operator)
    with pytest.raises(ValueError, match="CONSOLE_ALLOWED_EMAILS"):
        console_auth.console_oidc_settings(root, oidc_fields(), {**environ, "CONSOLE_ALLOWED_EMAILS": "not-an-email"})
    many = " ".join(f"person{index}@example.test" for index in range(101))
    with pytest.raises(ValueError, match="CONSOLE_ALLOWED_EMAILS"):
        console_auth.console_oidc_settings(root, oidc_fields(), {**environ, "CONSOLE_ALLOWED_EMAILS": many})
    with pytest.raises(ValueError, match="CONSOLE_USER_ID"):
        console_auth.console_oidc_settings(root, oidc_fields(), {**environ, "CONSOLE_USER_ID": "operator"})


def test_oidc_front_needs_the_meta_runtime_and_the_operator_email(tmp_path):
    settings = oidc_settings(MockIssuer())
    with pytest.raises(ValueError, match="Meta"):
        ConsoleFront(tmp_path, 8855, OPERATOR_EMAIL, None, runtime_namespace="mobkit", oidc=settings)
    with pytest.raises(ValueError, match="email"):
        ConsoleFront(tmp_path, 8855, "operator", None, runtime_namespace="meta", oidc=settings)
    with pytest.raises(ValueError, match="password"):
        ConsoleFront(tmp_path, 8855, OPERATOR_EMAIL, "operator-password-1", runtime_namespace="meta", oidc=settings)
    with pytest.raises(ValueError, match="named accounts"):
        ConsoleFront(tmp_path, 8855, OPERATOR_EMAIL, None, runtime_namespace="meta", oidc=settings, accounts={})
    for invalid in ({"issuer": "http://login.example.test"}, {"client_id": ""}, {"client_secret": ""},
                    {"allowed_emails": ("not-an-email",)}):
        with pytest.raises(ValueError):
            console_auth.ConsoleOidc(**{"issuer": ISSUER_URL, "client_id": CLIENT_ID,
                                        "client_secret": CLIENT_SECRET, **invalid})


async def test_forge_deployment_treats_oidc_packages_as_authenticated(tmp_path):
    from launchkit import deployment
    from launchkit.remote_browser_access import browser_responds
    fields = oidc_fields()
    files = {"application/console.json": (json.dumps(oidc_source()).encode(), 0o644)}
    assert deployment.package_console_scope(files, fields, {})["authentication"] == "oidc"
    inline = {"application/console.json": (json.dumps(oidc_source(client_secret=CLIENT_SECRET)).encode(), 0o644)}
    with pytest.raises(ValueError, match="Console authentication"):
        deployment.package_console_scope(inline, fields, {})
    values = {"CONSOLE_USER_ID": OPERATOR_EMAIL, "CONSOLE_OIDC_CLIENT_SECRET": CLIENT_SECRET,
              "CONSOLE_ALLOWED_EMAILS": ALICE}
    settings = deployment.prepare_private_settings(tmp_path, fields, values, False, port=18088,
                                                   console_authentication="oidc")
    assert settings["CONSOLE_USER_ID"] == OPERATOR_EMAIL and not (tmp_path / ".env").exists()
    for changed in ({"CONSOLE_USER_ID": "operator"}, {"CONSOLE_ALLOWED_EMAILS": "not-an-email"}):
        with pytest.raises(ValueError):
            deployment.prepare_private_settings(tmp_path, fields, {**values, **changed}, False, port=18088,
                                                console_authentication="oidc")

    async def sign_in_redirect(reader, writer):
        await reader.readuntil(b"\r\n\r\n")
        writer.write(b"HTTP/1.1 303 See Other\r\nLocation: /login\r\n\r\n")
        await writer.drain()
        writer.close()
        await writer.wait_closed()
    server = await asyncio.start_server(sign_in_redirect, "127.0.0.1", 0)
    try:
        assert await browser_responds(server.sockets[0].getsockname()[1], "oidc") is True
    finally:
        server.close()
        await server.wait_closed()


def test_update_does_not_move_an_installation_to_oidc():
    # OIDC sign-in is a new installation's choice: no update moves to it.
    from test_console_named_accounts import SHARED, console_update
    for before in (SHARED, b'{"authentication":"local-accounts"}\n'):
        _, blockers = console_update(before, json.dumps(oidc_source()).encode() + b"\n")
        assert any("application/console.json" in blocker for blocker in blockers)


def test_update_does_not_move_an_installation_from_oidc_or_change_its_provider():
    from test_console_named_accounts import console_update
    before = json.dumps(oidc_source()).encode() + b"\n"
    for after in (b'{"authentication":"local-accounts"}\n', b'{"authentication":"local-password"}\n',
                  json.dumps(oidc_source(issuer="https://other.example.test")).encode() + b"\n",
                  json.dumps(oidc_source(client_secret={"file": "/run/secrets/oidc"})).encode() + b"\n"):
        _, blockers = console_update(before, after)
        assert any("application/console.json" in blocker for blocker in blockers), after


# The front signs with cryptography directly. OIDC runs on the product route
# only (it needs the Meta runtime and the application's own source), whose
# requirements are the release's dependency closure: it pins cryptography
# exactly, with its hash, like every other dependency there.
def test_oidc_route_requirements_pin_the_fronts_signing_library():
    closures = sorted((Path(__file__).parents[1] / "release/inputs/product").glob("*/dependency-requirements.txt"))
    assert closures
    for closure in closures:
        pins = [line for line in closure.read_text().splitlines() if line.lower().startswith("cryptography")]
        assert len(pins) == 1 and re.fullmatch(r"cryptography==[0-9][0-9.]* --hash=sha256:[0-9a-f]{64}", pins[0]), closure


def test_setup_checks_the_oidc_operator_email_and_the_people_to_admit(generated_setup, monkeypatch, capsys):
    module, root = generated_setup
    write_oidc_source(root)
    (root / "application/__init__.py").write_text("def build(context):\n    return object()\n")
    (root / "application/configuration.json").write_text(json.dumps({"fields": oidc_fields()}))
    answers = {"Console operator email": iter(["operator", OPERATOR_EMAIL]),
               "People who may sign in": iter(["not-an-email", f"{ALICE}, {BOB}"])}

    def answer(prompt):
        for label, values in answers.items():
            if prompt.startswith(label):
                return next(values)
        return ""
    monkeypatch.setattr("builtins.input", answer)
    monkeypatch.setattr(module.getpass, "getpass",
                        lambda prompt: CLIENT_SECRET if prompt.startswith("OIDC client secret") else "")
    assert module.main() == 0
    output = capsys.readouterr().out
    assert "email address" in output and "/login/callback" in output and CLIENT_SECRET not in output
    saved = (root / ".env").read_text()
    assert f'CONSOLE_USER_ID="{OPERATOR_EMAIL}"' in saved and f'"{ALICE}, {BOB}"' in saved
    # An OIDC Console keeps no named accounts.
    assert not (root / ".mobkit" / console_auth.ACCOUNTS_FILE).exists()


async def test_generated_host_builds_the_oidc_front_from_its_source_and_private_settings(tmp_path, monkeypatch):
    """The generated Meta host reads the source and private settings once, at start."""
    import importlib.util
    from launchkit.blueprint import starter_blueprint
    from launchkit.compiler import generate_files

    files = generate_files(starter_blueprint())
    for name, content in files.items():
        destination = tmp_path / name
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_text(content)
    monkeypatch.syspath_prepend(str(tmp_path))
    definition = importlib.util.spec_from_file_location("oidc_generated_app", tmp_path / "app.py")
    host = importlib.util.module_from_spec(definition)
    definition.loader.exec_module(host)
    spec = json.loads(files["deployment.json"])
    (tmp_path / "application").mkdir(exist_ok=True)
    (tmp_path / "application/__init__.py").write_text(
        "class Integration: pass\ndef build(context): return Integration()\n")
    (tmp_path / "application/tool-policy.json").write_text(json.dumps({
        "provider_id": "oidc-host-specimen", "policy_id": "explicit-empty-intent", "revision": 1,
        "members": [{"member_identity": row["identity"], "grants": []} for row in spec["agents"]]}))
    write_oidc_source(tmp_path)
    (tmp_path / "application/configuration.json").write_text(json.dumps({"fields": oidc_fields()}))
    monkeypatch.setattr(host, "validate_configuration_values", lambda *a: None)
    monkeypatch.setattr(host, "local_tools", lambda *a: [])
    monkeypatch.setenv("CONSOLE_USER_ID", OPERATOR_EMAIL)
    monkeypatch.setenv("CONSOLE_OIDC_CLIENT_SECRET", CLIENT_SECRET)
    monkeypatch.setenv("CONSOLE_ALLOWED_EMAILS", ALICE)
    monkeypatch.delenv("CONSOLE_LOGIN_PASSWORD", raising=False)
    fronts, configs = [], []
    original = host.ConsoleFront

    def front(*args, **options):
        console = original(*args, **options)
        fronts.append((args, options, console))
        return console
    monkeypatch.setattr(host, "ConsoleFront", front)

    class Pool:
        def __init__(self, spec):
            self.connections = {}

        async def start(self):
            pass

        async def close(self):
            pass

        def for_class(self, item):
            return []
    monkeypatch.setattr(host, "MCPPool", Pool)

    class Runtime:
        async def identity_bootstrap_status(self):
            return SimpleNamespace(complete=True, ready=True, counts=SimpleNamespace(broken=0))

        async def shutdown(self):
            pass
    runtime = Runtime()

    class Startup:
        console_public_origin = None
        hosted_mcp_targets = ()
        gateway = SimpleNamespace(path=tmp_path / "app.py")

        async def prepare(self, application):
            pass

        def builder_config(self, **options):
            configs.append(options)
            return SimpleNamespace(**options)

        async def connect(self, config, application):
            application.context.stop_event.set()
            return runtime

        async def shutdown(self, selected):
            await selected.shutdown()
    await host.run(spec, SimpleNamespace(activate_revision=None, connections_check=False),
                   application_startup=Startup())
    (args_used, options, console), = fronts
    assert args_used[2:4] == (OPERATOR_EMAIL, None) and options["runtime_namespace"] == "meta"
    settings = options["oidc"]
    assert (settings.issuer, settings.client_id, settings.client_secret, settings.allowed_emails) == (
        ISSUER_URL, CLIENT_ID, CLIENT_SECRET, (ALICE,))
    assert configs[-1]["auth_config"] == console.native_auth and console.native_auth["provider"] == "oidc"
    assert FRONT_ISSUER.fullmatch(console.native_auth["issuer"])
    assert CLIENT_SECRET not in json.dumps(configs[-1]["auth_config"])
