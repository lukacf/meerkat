"""Known-account Google profiles share one protocol, with explicit host choice."""

import asyncio
import base64
import hashlib
import json
import unittest
from urllib.parse import parse_qs, urlsplit

import test_google_oauth as fixture


oauth = fixture.oauth
PROFILES = {
    "gmail": ("https://www.googleapis.com/auth/gmail.send", "https://gmail.googleapis.com/"),
    "calendar": ("https://www.googleapis.com/auth/calendar.events.readonly",
                 "https://www.googleapis.com/calendar/v3/"),
    "drive": ("https://www.googleapis.com/auth/drive.readonly",
              "https://www.googleapis.com/drive/v3/"),
}


def reconnect(service, scopes=None):
    return fixture.spec(service=service, requested_scopes=scopes or frozenset({"openid", PROFILES[service][0]}))


def refresh(service, scopes=None):
    return fixture.refresh_spec(service=service, required_scopes=scopes or frozenset({"openid", PROFILES[service][0]}))


async def exchange(client, spec):
    return await oauth.exchange_and_observe(spec, code="fixture-code", code_verifier=fixture.VERIFIER,
        expected_nonce=fixture.NONCE, http_client=client)


class GoogleServiceProfileTests(unittest.IsolatedAsyncioTestCase):
    def test_host_explicit_service_and_scope_choice_no_inference(self):
        vocabularies = {"gmail": oauth.GMAIL_SCOPES, "calendar": oauth.CALENDAR_SCOPES, "drive": oauth.DRIVE_SCOPES}
        for service, vocabulary in vocabularies.items():
            for scope in vocabulary:
                selected = frozenset({"openid", scope})
                self.assertEqual(reconnect(service, selected).requested_scopes, selected)
                self.assertEqual(refresh(service, selected).required_scopes, selected)
            for other in set(PROFILES) - {service}:
                wrong = frozenset({"openid", PROFILES[other][0]})
                for factory in (reconnect, refresh):
                    with self.subTest(service=service, wrong=other, factory=factory.__name__), self.assertRaises(oauth.GoogleOAuthError) as caught:
                        factory(service, wrong)
                    self.assertEqual(caught.exception.exchange_state, oauth.ExchangeState.NOT_STARTED)
        for service in (None, True, [], "", "Google", "Calendar", "unknown"):
            with self.assertRaises(oauth.GoogleOAuthError):
                fixture.spec(service=service)
            with self.assertRaises(oauth.GoogleOAuthError):
                fixture.refresh_spec(service=service)
        # Existing omitted-service callers retain Gmail semantics and scope refusal.
        self.assertEqual(fixture.spec().service, "gmail")
        self.assertEqual(fixture.refresh_spec().service, "gmail")
        with self.assertRaises(oauth.GoogleOAuthError):
            fixture.spec(requested_scopes=frozenset({"openid", PROFILES["drive"][0]}))

    def test_authorization_url_retains_only_explicit_scopes_and_fixed_destination(self):
        challenge = base64.urlsafe_b64encode(hashlib.sha256(fixture.VERIFIER.encode()).digest()).decode().rstrip("=")
        for service in PROFILES:
            selected = reconnect(service)
            request = oauth.authorization_request(selected, state="owner-state", nonce=fixture.NONCE,
                                                   code_challenge=challenge)
            url = urlsplit(request.url)
            params = parse_qs(url.query)
            self.assertEqual(url.scheme + "://" + url.netloc + url.path, oauth.AUTHORIZATION_ENDPOINT)
            self.assertEqual(set(params["scope"][0].split()), selected.requested_scopes)
            self.assertEqual(params["login_hint"], [fixture.SUBJECT])
            self.assertNotIn("resource", params)
            self.assertNotIn("include_granted_scopes", params)
            self.assertNotIn("service", params)

    async def test_signed_exchange_preserves_service_resource_and_surplus_observations(self):
        for service, (_, resource) in PROFILES.items():
            selected = reconnect(service)
            peer = fixture.Fixture()
            scopes = selected.requested_scopes | {"https://www.googleapis.com/auth/unselected.scope"}
            peer.token["scope"] = " ".join(sorted(scopes))
            async with fixture.client_for(peer) as client:
                result = await exchange(client, selected)
            self.assertEqual(result.observation.intended_resource, resource)
            self.assertEqual(result.safe_view()["intended_resource"], resource)
            self.assertEqual(set(result.observation.reported_scopes), scopes)
            self.assertEqual([str(call.url) for call in peer.calls],
                [oauth.TOKEN_ENDPOINT, oauth.CERTIFICATES_ENDPOINT, oauth.USERINFO_ENDPOINT])
            self.assertEqual(peer.calls[-1].headers["authorization"], "Bearer " + result.access_token)
            self.assertNotIn(result.access_token, json.dumps(result.safe_view()))
            self.assertNotIn("service", parse_qs(peer.calls[0].content.decode()))

    async def test_resource_is_host_selection_not_surplus_scope_or_userinfo_hint(self):
        selected = reconnect("calendar")
        peer = fixture.Fixture()
        peer.token["scope"] = " ".join(sorted(selected.requested_scopes | {PROFILES["drive"][0]}))
        peer.user.update(service="drive", intended_resource=PROFILES["drive"][1])
        async with fixture.client_for(peer) as client:
            result = await exchange(client, selected)
        self.assertEqual(result.observation.intended_resource, PROFILES["calendar"][1])

    async def test_each_service_refuses_missing_scope_and_wrong_subject(self):
        for service in ("calendar", "drive"):
            for failure in ("scope", "subject"):
                selected = reconnect(service)
                peer = fixture.Fixture()
                peer.token["scope"] = " ".join(sorted(selected.requested_scopes))
                if failure == "scope":
                    peer.token["scope"] = "openid " + PROFILES["gmail"][0]
                else:
                    peer.user["sub"] = "different-subject"
                async with fixture.client_for(peer) as client:
                    with self.subTest(service=service, failure=failure), self.assertRaises(oauth.GoogleOAuthError) as caught:
                        await exchange(client, selected)
                self.assertEqual(caught.exception.reason, "missing_scope" if failure == "scope" else "identity_mismatch")
                self.assertEqual(len(peer.calls), 1 if failure == "scope" else 3)

    async def test_refresh_preserves_service_with_reported_or_unreported_scopes(self):
        for service, (_, resource) in PROFILES.items():
            selected = refresh(service)
            for reported in (False, True):
                peer = fixture.RefreshFixture()
                peer.token.pop("scope")
                if reported:
                    peer.token["scope"] = " ".join(sorted(selected.required_scopes))
                async with fixture.client_for(peer) as client:
                    result = await oauth.refresh_and_observe(selected, refresh_token="old-refresh", http_client=client)
                self.assertEqual(result.observation.intended_resource, resource)
                self.assertEqual(result.safe_view()["intended_resource"], resource)
                self.assertEqual(result.observation.reported_scopes,
                    tuple(sorted(selected.required_scopes)) if reported else None)
                self.assertEqual(result.observation.scope_evidence, "google_token_endpoint" if reported else None)
                self.assertEqual(len(peer.calls), 2)
                self.assertEqual(parse_qs(peer.calls[0].content.decode()),
                    {"client_id": [fixture.CLIENT_ID], "refresh_token": ["old-refresh"], "grant_type": ["refresh_token"]})
                self.assertIsNone(result.refresh_token)

    async def test_refresh_refuses_wrong_subject_and_lost_required_scope(self):
        for service in ("calendar", "drive"):
            for failure in ("scope", "subject"):
                peer = fixture.RefreshFixture()
                peer.token["scope"] = " ".join(sorted(refresh(service).required_scopes))
                if failure == "scope":
                    peer.token["scope"] = "openid " + PROFILES["gmail"][0]
                else:
                    peer.user["sub"] = "wrong-subject"
                async with fixture.client_for(peer) as client:
                    with self.subTest(service=service, failure=failure), self.assertRaises(oauth.GoogleOAuthError) as caught:
                        await oauth.refresh_and_observe(refresh(service), refresh_token="old-refresh", http_client=client)
                self.assertEqual(caught.exception.reason, "missing_scope" if failure == "scope" else "identity_mismatch")
                self.assertEqual(len(peer.calls), 1 if failure == "scope" else 2)

    async def test_mutated_service_is_revalidated_before_network(self):
        for mode in ("exchange", "refresh"):
            for bad in ("drive", "unknown", None):
                selected = reconnect("calendar") if mode == "exchange" else refresh("calendar")
                object.__setattr__(selected, "service", bad)
                peer = fixture.Fixture()
                async with fixture.client_for(peer) as client:
                    with self.assertRaises(oauth.GoogleOAuthError) as caught:
                        if mode == "exchange":
                            await exchange(client, selected)
                        else:
                            await oauth.refresh_and_observe(selected, refresh_token="old-refresh", http_client=client)
                self.assertEqual(caught.exception.reason, "invalid_inputs")
                self.assertEqual(peer.calls, [])

    async def test_new_service_cancellation_retains_joined_cleanup_and_outcome(self):
        for service in ("calendar", "drive"):
            peer = fixture.RefreshFixture()
            peer.token["scope"] = " ".join(sorted(refresh(service).required_scopes))
            peer.gate_url = oauth.USERINFO_ENDPOINT
            async with fixture.client_for(peer) as client:
                task = asyncio.create_task(oauth.refresh_and_observe(refresh(service), refresh_token="old-refresh", http_client=client))
                await asyncio.wait_for(peer.entered.wait(), 3)
                task.cancel()
                await asyncio.sleep(0)
                self.assertFalse(task.done())
                peer.release.set()
                with self.assertRaises(oauth.GoogleOAuthCancelled) as caught:
                    await task
            self.assertEqual(caught.exception.exchange_state, oauth.ExchangeState.RESPONSE_RECEIVED)
            self.assertEqual(len(peer.calls), 2)
            self.assertIn(oauth.USERINFO_ENDPOINT, peer.closed)


if __name__ == "__main__":
    unittest.main()
