# Observe a known Google account after code exchange or refresh

`meerkat_toolkit.auth.google_oauth` is a host-only protocol helper for an already
known Google subject and an explicitly admitted Desktop OAuth registration. It
builds a protected authorization URL, exchanges one authorization code, verifies
Google's signed ID token, and observes the same subject through UserInfo using
the exact returned access token. A separate explicit access-token refresh
operation observes the new token's UserInfo subject without consuming ID tokens
or recreating the original sign-in ceremony.

This is not a browser flow, credential store, refresh scheduler, account picker,
native completion handle or tool grant. The production downstream credential
binding remains with the native owner under #1631. A local signed fixture test
does not establish a real Google login or protected browser integration.
The [native owner's accepted contract](../../docs/coordination/20261004T003359.800515-claude-gcp-lead-45cc8b.txt)
puts nonce custody in the flow authority. It generates and retains the nonce
at start, returns it for the authorization URL, then supplies it from the
admitted attempt at completion. Toolkit and application hosts never generate,
persist or derive it. The nonce expires or is consumed with that attempt; a
reopened flow gets a fresh one. This contract is scheduled under #1631 after
0.8.51 and is not yet a consumable SDK API.

## Host composition

For code exchange, install the optional `google-oauth` extra. It uses `google-auth==2.59.1` for
cryptographic verification and the existing HTTPX/AnyIO transport dependencies.
It does not use ADC, `InstalledAppFlow`, `AuthorizedSession` or Google credential
refresh APIs. The separate refresh operation uses HTTPX/AnyIO without importing
the optional cryptographic verifier.

```python
from meerkat_toolkit.auth.google_oauth import (
    GoogleReconnectSpec,
    authorization_request,
    exchange_and_observe,
)
from meerkat_toolkit.httpx_client import prepare_httpx_client

spec = GoogleReconnectSpec(
    client_id=registered_desktop_client_id,
    redirect_uri=exact_host_loopback_redirect,
    expected_subject=known_google_subject,
    requested_scopes=frozenset({
        "openid",
        "https://www.googleapis.com/auth/gmail.send",
    }),
)
# Values below come from the native flow owner; these names are local variables,
# not claims about an already released start-result API.
request = authorization_request(
    spec, state=owner_state, nonce=owner_nonce,
    code_challenge=owner_s256_challenge, offline=True,
)
# Give request.url only to the host's protected presenter.
# The native owner retains the ceremony and validates callback state, exact
# redirect, expiry, cancellation and one-time admission before exchange.
prepare_httpx_client(host_http_client)  # Before sharing or starting any calls.
result = await exchange_and_observe(
    spec, code=admitted_code, code_verifier=owner_verifier,
    expected_nonce=admitted_owner_nonce, client_secret=desktop_secret_or_none,
    http_client=host_http_client, timeout_seconds=20,
)
# Keep result host-private. The native owner still decides persistence and use.
```

The host explicitly selects `service="gmail"` (the backward-compatible default),
`service="calendar"` or `service="drive"` on both spec types. Every selection
requires `openid` plus at least one scope from that service's supported set.
The helper never infers the service from user text, scopes or provider fields,
and never adds `email`, `profile` or an API scope implicitly.

| Service | Supported API scope suffixes under `https://www.googleapis.com/auth/` |
| --- | --- |
| `gmail` | `gmail.send`, `gmail.readonly`, `gmail.compose`, `gmail.modify`, `gmail.metadata`; also `https://mail.google.com/` |
| `calendar` | `calendar`, `calendar.readonly`, `calendar.events`, `calendar.events.readonly`, `calendar.events.owned`, `calendar.events.owned.readonly`, `calendar.app.created`, `calendar.calendarlist.readonly`, `calendar.calendars.readonly` |
| `drive` | `drive`, `drive.readonly`, `drive.file`, `drive.metadata`, `drive.metadata.readonly` |

For example, a host reconnecting a known Calendar account supplies
`service="calendar"` with the explicit set `{"openid",
"https://www.googleapis.com/auth/calendar.events.readonly"}`. A metadata-only
Drive task can select `drive.metadata.readonly`; content reads need an
applicable content scope such as `drive.readonly` or access to the chosen file
under `drive.file`. The latter is not an account-wide read grant. The broader
scope spellings above are supported for explicit host uses, not requested by
default. Connector operation selection still applies independently.
[Calendar scope reference](https://developers.google.com/workspace/calendar/api/auth)
and [Drive scope reference](https://developers.google.com/workspace/drive/api/guides/api-specific-auth)
were checked on 2026-10-04.

Cross-service requested sets are refused. A host uses a separate explicit
selection for each service. The returned `intended_resource` records the host's
selected API (`https://gmail.googleapis.com/`,
`https://www.googleapis.com/calendar/v3/` or
`https://www.googleapis.com/drive/v3/`). It is not a signed token audience or a
claim that the opaque Google token cannot access another service. Surplus
reported scopes cannot change this host selection. All requested scopes must
appear in the token endpoint's explicit response. Surplus reported scopes are
preserved for host policy; they do not expose additional tools or prove resource
access. The local evidence label is `google_token_endpoint`. At native
composition it maps to the owner's typed `ScopeEvidence::TokenEndpointResponse`;
it is not itself a native authority field. The owner requires requested scopes
to be a subset of granted scopes, stores the exact granted set, and persists
the evidence kind with the credential for status readback. Surplus-scope policy
belongs to that owner.

The known subject is an exact, nonempty, control-free ASCII identifier of at
most 255 characters. The helper does not infer it from an email or display name.
It accepts only HTTP loopback redirects with an explicit port, a path and the
literal host `127.0.0.1` or `[::1]`; credentials, query and fragment are refused.
The host must establish that the registration and redirect are appropriate.
`offline` and `prompt_consent` are explicit host choices, both false by default.
There is no incremental-authorization parameter or automatic consent retry.

## One explicit access-token refresh

For an already selected known account, the host can request one refresh:

```python
from meerkat_toolkit.auth.google_oauth import GoogleRefreshSpec, refresh_and_observe

refresh_spec = GoogleRefreshSpec(
    client_id=registered_desktop_client_id,
    expected_subject=known_google_subject,
    required_scopes=frozenset({
        "openid",
        "https://www.googleapis.com/auth/gmail.send",
    }),
)
# The host admits and serializes this attempt for its selected credential.
refreshed = await refresh_and_observe(
    refresh_spec, refresh_token=owner_refresh_token,
    client_secret=desktop_secret_or_none,
    http_client=host_http_client, timeout_seconds=20,
)
# The native owner reconciles reported or retained scopes and current policy,
# then atomically commits the credential update. A result is not that commit.
```

This profile selects **unbound bearer Desktop credentials**. The host must know
that issuance provenance; neither opaque token text nor `token_type=Bearer`
proves the absence of DPoP binding. Google's current Desktop documentation also
uses `Bearer` for DPoP responses. DPoP keys, proofs and nonce retries are outside
this operation. The refresh POST sends only `client_id`, `refresh_token`,
`grant_type=refresh_token` and an explicitly supplied optional Desktop
`client_secret`. It sends no scope, redirect, authorization code, PKCE verifier
or ceremony nonce. There is no alternate credential dialect or automatic retry.
[Google Desktop refresh contract](https://developers.google.com/identity/protocols/oauth2/native-app#offline)

`GoogleRefreshSpec` uses the same explicit service selection and scope vocabulary
as the code-exchange profile. Its `required_scopes` is a conditional validation
requirement, not a statement that those grants were observed. When Google
reports `scope`, it must parse strictly and include the required set; the
complete set, including surplus scopes, is preserved with local evidence
`google_token_endpoint`. A present malformed or insufficient scope report is
refused. An absent field instead produces `reported_scopes=None` and
`scope_evidence=None`. The helper does not substitute requested scopes, UserInfo
fields, a previous response or a historical grant. No tokeninfo fallback runs.

The [accepted native refresh disposition](https://github.com/lukacf/meerkat/issues/1631)
keeps ownership of omitted-scope handling in the native credential owner. Under
[RFC 6749 sections 5.1 and 6](https://www.rfc-editor.org/rfc/rfc6749.html#section-6),
it will retain the original bound grant's exact scope with distinct typed
`RetainedOnRefresh` evidence referencing the original evidence. Present scopes
are authoritative for the new token; losing a required scope refuses the
update. This is an accepted native contract, not a released Toolkit commit
implementation. The provider result intentionally contains no retained-scope
value and no ready, authorized or usable flag.

Every successful return requires an authenticated UserInfo GET with the
**exact new access token** and an exact matching `sub`. Email and other profile
fields do not establish identity and are discarded. The profile requests
`openid` plus the explicitly selected service scopes and adds no API permissions; successful use
requires UserInfo to return the selected `sub`. This has fixture, not
live-provider, verification. It does not call Gmail's mailbox-profile API.
A returned `GoogleRefreshObservation` identifies
`google_userinfo` as its identity evidence. `configured_client_id` records the
client selected for the successful refresh grant; it is not a freshly verified
signed audience. `provider` and `intended_resource` name this protocol profile,
not signed issuer claims or proof that any selected service operation is permitted.
[Google UserInfo contract](https://developers.google.com/identity/openid-connect/reference#userinfo-endpoint)

Google can omit `id_token` during refresh. If present, it is discarded without
decoding, verifying, exporting or using it. This access-token-only operation
does not authenticate a new user session, assert a fresh OIDC identity proof,
or reuse the code exchange's nonce or `at_hash` checks. A component that consumes
a refreshed ID token must separately implement the
[OIDC refresh validation rules](https://openid.net/specs/openid-connect-core-1_0.html#RefreshTokenResponse),
including their relationship to the original authentication. The existing
code-exchange verifier is unchanged.

`GoogleRefreshResult` is a separate private type with access token, optional
replacement refresh token, timing and observation fields; it has no ID-token
field. Missing `refresh_token` remains `None`. The native owner retains its
previous refresh token in that case, or atomically replaces it when Google
returns one. The helper never reads or changes a credential store.

Access lifetime is mandatory and checked before and after UserInfo using wall
and monotonic time. Optional `refresh_token_expires_in` is validated as a
positive integer and preserved alongside a conservative
`refresh_token_expires_at`. Both absolute expiry values start at token POST
entry. Missing refresh lifetime stays `None` and does not extend the original
owner-held expiry. A reported refresh lifetime can elapse during UserInfo even
while the access token remains valid; the owner must inspect the returned
refresh expiry, not assume that a successful return guarantees a still-valid
refresh token. `safe_view()` exposes only observations and timing.

## Network and verification contract

Code exchange uses only these fixed provider destinations:

- Authorization: `https://accounts.google.com/o/oauth2/v2/auth`
- One token POST: `https://oauth2.googleapis.com/token`
- One certificate GET: `https://www.googleapis.com/oauth2/v1/certs`
- One UserInfo GET: `https://openidconnect.googleapis.com/v1/userinfo`

Refresh uses only one token POST and one UserInfo GET from this list. It does
not retrieve certificates or invoke the optional cryptographic verifier.

The URL constants were checked against Google's
[discovery document](https://accounts.google.com/.well-known/openid-configuration)
and the maintained verifier's documented
[X.509 certificate endpoint](https://google-auth.readthedocs.io/en/latest/reference/google.oauth2.id_token.html).
No issuer, key URL, redirect or returned profile link chooses another network
destination. There is no tokeninfo, Gmail-write or profile-image fallback.

The caller supplies a prepared, borrowed HTTPX client. The helper constructs
explicit absolute requests and supplies an empty HTTPX authentication flow;
client default headers, auth, parameters and cookies are not transmitted. The
client can still retain response cookies internally. Redirect following is
disabled for each request. The host retains client lifetime, TLS verification,
proxy configuration, egress policy and response-hook order. Request/response
hooks and transports are trusted code: they must not inject credentials,
retarget or replay requests, bypass response bounds, or detach cleanup. Client
flags are not a security boundary against those callbacks.

Token, certificate and UserInfo bodies are limited to 64 KiB, 256 KiB and 32 KiB
respectively, with bounded JSON depth and node count. Duplicate fields,
non-finite or fractional JSON numbers, invalid UTF-8, unsupported encodings and
invalid response lengths are refused. For code exchange, ID-token header and claims JSON are
checked for duplicates before verification. Only RS256, a bounded `kid`, and
optional `typ=JWT` are supported; other JOSE fields, including `crit`, `jku` and
embedded keys, are refused. The certificate response must be an X.509 map with
at most 32 entries. It is passed to Google's synchronous verifier through an
in-memory adapter that accepts only the exact certificate GET, so verification
cannot initiate additional requests.

Code exchange requires the exact scalar audience, known subject, nonce, matching
`azp` when present, strict integer time claims and the access token's RS256
`at_hash`. Google's documented `accounts.google.com` issuer spelling maps to
`https://accounts.google.com` only after verification. UserInfo must return the
same subject. Optional profile fields are discarded. These checks follow
[Google's ID-token contract](https://developers.google.com/identity/openid-connect/openid-connect)
and [OIDC UserInfo validation](https://openid.net/specs/openid-connect-core-1_0.html#UserInfoResponse).
Requiring `at_hash` is an explicit restriction of this profile; its absence is
a refusal, never a decoded-claims fallback.

## Results, failure and cancellation

For code exchange, `GoogleProtocolResult` contains host-private access, optional refresh and ID
tokens. Missing refresh tokens remain `None`; the helper neither recovers an old
one nor decides retention. Its representation hides all fields. `safe_view()`
contains the immutable observation's identity, reported scopes and provenance,
verification time, token-response receipt time, the original reported
`expires_in` lifetime and a
conservative absolute `expires_at`. It excludes tokens and raw ID-token claims.
Python object privacy and redacted representations are conventions, not an
isolation boundary; never serialize the complete result into an agent tool.

For code exchange, the absolute expiry starts conservatively at token POST entry rather than at
handoff. The helper also uses elapsed monotonic time while checking validity.
Both access-token lifetime and ID-token expiry are rechecked after UserInfo;
neither can be prolonged by time spent retrieving keys or observing the account.
Consumers must use `expires_at`, not restart the raw `expires_in` lifetime.

`GoogleOAuthError` reports fixed `reason`, `phase` and `exchange_state` values.
Its optional `provider_code` is `invalid_grant`, `invalid_client` or `None`.
Only a complete, bounded JSON refusal from the token endpoint with HTTP 400 or
401 can supply a code. The refusal must contain an exact recognized `error`
and no fields other than optional string `error_description` and `error_uri`.
Descriptions and URIs are discarded, never followed or exposed. Unknown,
mixed, malformed, encoded, oversized or interrupted refusal bodies keep
`provider_code=None` and the existing `provider_rejected` reason. Reading this
optional diagnostic uses the same response limits, deadline and joined cleanup
as other token responses; cancellation or deadline expiry retains its own
outcome. The exception string and existing failure fields are unchanged.
[Google token errors](https://developers.google.com/identity/protocols/oauth2/web-server#errors)
and [Desktop token exchange](https://developers.google.com/identity/protocols/oauth2/native-app#exchange-authorization-code)
describe the provider codes. A code is diagnostic evidence, not an automatic
reconnect decision or proof that a credential was not issued or rotated.

`GoogleOAuthCancelled` preserves cancellation while exposing the phase and state.
Neither carries raw provider bodies, headers, token claims or original exception
objects. The state names have deliberately limited meanings:

- `not_started`: the token POST has not entered the helper's send path.
- `unconfirmed`: token send has entered; no response headers were observed. The
  code or refresh credential may already have caused a provider effect.
- `response_received`: token response headers were observed. This says nothing
  about successful authentication, valid credentials or persistence.

No failure after send entry recommends replaying the code or refresh. Key retrieval,
signature validation, expiry, missing scope or UserInfo failure can follow a
consumed code. A failed refresh observation can follow issuance of a new access
token or rotation of a refresh token. The helper makes no recovery exchange and
returns no credential result after a failed required check. An intentionally
unreported refresh scope remains a missing observation, not a failed check or
an effective grant.

The deadline bounds protocol work, including later requests. Cancellation and
deadline expiry still join admitted response cleanup, which can finish after
the deadline. Repeated direct task cancellation and outer AnyIO cancellation
do not detach cleanup. The host must supply a transport whose cleanup eventually
settles, and must close its client only after this operation has joined.

The native-application registration, PKCE and loopback requirements are described
in [Google's Desktop flow](https://developers.google.com/identity/protocols/oauth2/native-app).
The native flow authority owns state/nonce/verifier generation and storage,
callback admission and later credential custody. The application supplies the
protected browser presentation. The future registered Google code-exchange strategy must
declare `requires_nonce`; the protocol helper does not register that strategy
or implement an alternate owner.
