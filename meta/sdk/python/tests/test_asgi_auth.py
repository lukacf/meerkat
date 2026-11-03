"""Tests for the ASGI bearer token validator and auth config handling."""
import base64
import hashlib
import hmac
import json
import time

import pytest

from meerkat_meta.config import auth
from meerkat_meta.runtime import (
    AsgiApp,
    MetaRuntime,
    _auth_config_to_dict,
    _serialize_config,
    _validate_bearer_token,
)


def _make_hs256_token(
    secret: str,
    claims: dict | None = None,
    header: dict | None = None,
) -> str:
    """Build a valid HS256 JWT for testing."""
    hdr = header or {"alg": "HS256", "typ": "JWT"}
    payload = claims or {}
    hdr_b64 = base64.urlsafe_b64encode(json.dumps(hdr).encode()).rstrip(b"=").decode()
    payload_b64 = base64.urlsafe_b64encode(json.dumps(payload).encode()).rstrip(b"=").decode()
    signing_input = f"{hdr_b64}.{payload_b64}".encode()
    sig = base64.urlsafe_b64encode(
        hmac.new(secret.encode(), signing_input, hashlib.sha256).digest()
    ).rstrip(b"=").decode()
    return f"{hdr_b64}.{payload_b64}.{sig}"


# ---------------------------------------------------------------------------
# _auth_config_to_dict
# ---------------------------------------------------------------------------


class TestAuthConfigToDict:
    def test_jwt_config_object(self):
        cfg = auth.jwt("my-secret", issuer="test-iss")
        d = _auth_config_to_dict(cfg)
        assert d["provider"] == "jwt"
        assert d["shared_secret"] == "my-secret"

    def test_google_config_object(self):
        cfg = auth.google("my-client-id")
        d = _auth_config_to_dict(cfg)
        assert d["provider"] == "google"

    def test_plain_dict(self):
        d = _auth_config_to_dict({"provider": "jwt", "shared_secret": "s"})
        assert d["provider"] == "jwt"

    def test_unknown_type_returns_empty(self):
        assert _auth_config_to_dict(42) == {}
        assert _auth_config_to_dict("string") == {}


# ---------------------------------------------------------------------------
# _validate_bearer_token — JWT provider
# ---------------------------------------------------------------------------


class TestValidateBearerTokenJwt:
    SECRET = "test-secret-key"

    def _config(self, **overrides):
        base = {"provider": "jwt", "shared_secret": self.SECRET}
        base.update(overrides)
        return base

    def test_valid_token(self):
        token = _make_hs256_token(self.SECRET, {"sub": "user"})
        assert _validate_bearer_token(token, self._config()) is True

    def test_valid_token_with_typed_config(self):
        token = _make_hs256_token(self.SECRET, {"sub": "user"})
        cfg = auth.jwt(self.SECRET)
        assert _validate_bearer_token(token, cfg) is True

    def test_wrong_secret_rejected(self):
        token = _make_hs256_token("wrong-secret", {"sub": "user"})
        assert _validate_bearer_token(token, self._config()) is False

    def test_issuer_mismatch_rejected(self):
        token = _make_hs256_token(self.SECRET, {"iss": "bad-issuer"})
        assert _validate_bearer_token(token, self._config(issuer="expected")) is False

    def test_audience_mismatch_rejected(self):
        token = _make_hs256_token(self.SECRET, {"aud": "wrong-aud"})
        assert _validate_bearer_token(token, self._config(audience="expected")) is False

    def test_expired_token_rejected(self):
        token = _make_hs256_token(self.SECRET, {"exp": time.time() - 120})
        assert _validate_bearer_token(token, self._config()) is False

    def test_expired_within_leeway_accepted(self):
        token = _make_hs256_token(self.SECRET, {"exp": time.time() - 30})
        assert _validate_bearer_token(token, self._config(leeway_seconds=60)) is True

    def test_not_before_future_rejected(self):
        token = _make_hs256_token(self.SECRET, {"nbf": time.time() + 120})
        assert _validate_bearer_token(token, self._config()) is False

    def test_not_before_within_leeway_accepted(self):
        token = _make_hs256_token(self.SECRET, {"nbf": time.time() + 30})
        assert _validate_bearer_token(token, self._config(leeway_seconds=60)) is True

    def test_garbage_token_rejected(self):
        assert _validate_bearer_token("not.a.jwt", self._config()) is False

    def test_two_part_token_rejected(self):
        assert _validate_bearer_token("only.two", self._config()) is False

    def test_non_hs256_alg_rejected(self):
        token = _make_hs256_token(self.SECRET, header={"alg": "RS256", "typ": "JWT"})
        assert _validate_bearer_token(token, self._config()) is False

    def test_array_audience_accepted(self):
        """JWT aud may be an array of strings (RFC 7519 §4.1.3)."""
        token = _make_hs256_token(self.SECRET, {"aud": ["my-app", "other-app"]})
        assert _validate_bearer_token(token, self._config(audience="my-app")) is True

    def test_array_audience_mismatch_rejected(self):
        token = _make_hs256_token(self.SECRET, {"aud": ["other-app"]})
        assert _validate_bearer_token(token, self._config(audience="my-app")) is False

    def test_string_exp_rejected(self):
        """exp must be numeric, not a string."""
        token = _make_hs256_token(self.SECRET, {"exp": "0"})
        assert _validate_bearer_token(token, self._config()) is False

    def test_array_payload_rejected(self):
        """JWT payload must be a JSON object, not an array."""
        token = _make_hs256_token(self.SECRET, claims=None)
        # Manually forge a token with array payload
        hdr_b64 = base64.urlsafe_b64encode(json.dumps({"alg": "HS256"}).encode()).rstrip(b"=").decode()
        payload_b64 = base64.urlsafe_b64encode(json.dumps([]).encode()).rstrip(b"=").decode()
        signing_input = f"{hdr_b64}.{payload_b64}".encode()
        sig = base64.urlsafe_b64encode(
            hmac.new(self.SECRET.encode(), signing_input, hashlib.sha256).digest()
        ).rstrip(b"=").decode()
        array_token = f"{hdr_b64}.{payload_b64}.{sig}"
        assert _validate_bearer_token(array_token, self._config()) is False

    def test_non_dict_header_rejected(self):
        """JWT header must be a JSON object."""
        # Forge a token with array header
        hdr_b64 = base64.urlsafe_b64encode(json.dumps([1, 2]).encode()).rstrip(b"=").decode()
        payload_b64 = base64.urlsafe_b64encode(json.dumps({}).encode()).rstrip(b"=").decode()
        signing_input = f"{hdr_b64}.{payload_b64}".encode()
        sig = base64.urlsafe_b64encode(
            hmac.new(self.SECRET.encode(), signing_input, hashlib.sha256).digest()
        ).rstrip(b"=").decode()
        bad_token = f"{hdr_b64}.{payload_b64}.{sig}"
        assert _validate_bearer_token(bad_token, self._config()) is False


# ---------------------------------------------------------------------------
# _validate_bearer_token — non-JWT providers
# ---------------------------------------------------------------------------


class TestValidateBearerTokenNonJwt:
    def test_google_provider_rejected(self):
        token = _make_hs256_token("x", {"aud": "client-id"})
        assert _validate_bearer_token(token, {"provider": "google"}) is False

    def test_unknown_provider_rejected(self):
        token = _make_hs256_token("x")
        assert _validate_bearer_token(token, {"provider": "custom"}) is False

    def test_empty_config_rejected(self):
        token = _make_hs256_token("x")
        assert _validate_bearer_token(token, {}) is False


# ---------------------------------------------------------------------------
# AsgiApp — fail-fast on Google auth
# ---------------------------------------------------------------------------


class TestAsgiAppGoogleAuthRejection:
    def test_google_config_raises_at_construction(self):
        rt = MetaRuntime.__new__(MetaRuntime)
        with pytest.raises(ValueError, match="GoogleAuthConfig"):
            AsgiApp(runtime=rt, auth_config=auth.google("client-id"))

    def test_google_dict_config_raises_at_construction(self):
        rt = MetaRuntime.__new__(MetaRuntime)
        with pytest.raises(ValueError, match="GoogleAuthConfig"):
            AsgiApp(runtime=rt, auth_config={"provider": "google", "client_id": "x"})

    def test_jwt_config_accepted(self):
        rt = MetaRuntime.__new__(MetaRuntime)
        app = AsgiApp(runtime=rt, auth_config=auth.jwt("secret"))
        assert app._auth_config is not None

    def test_none_auth_accepted(self):
        rt = MetaRuntime.__new__(MetaRuntime)
        app = AsgiApp(runtime=rt, auth_config=None)
        assert app._auth_config is None


# ---------------------------------------------------------------------------
# _serialize_config — non-serializable objects
# ---------------------------------------------------------------------------


class TestSerializeConfig:
    def test_dict_with_to_dict_object(self):
        cfg = auth.jwt("secret", issuer="iss")
        result = _serialize_config(cfg)
        assert result["provider"] == "jwt"
        assert result["shared_secret"] == "secret"

    def test_nested_non_serializable_becomes_class_name(self):
        """event_log(storage=DummyStore()) must not crash json.dumps."""
        class DummyStore:
            pass
        result = _serialize_config({"storage": DummyStore(), "batch_size": 10})
        assert result["batch_size"] == 10
        assert "DummyStore" in result["storage"]
        # Must be JSON-serializable now
        json.dumps(result)

    def test_primitives_pass_through(self):
        assert _serialize_config(None) is None
        assert _serialize_config(42) == 42
        assert _serialize_config("hello") == "hello"
        assert _serialize_config(True) is True
