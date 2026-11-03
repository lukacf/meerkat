# Slack known-account OAuth protocol helper

`meerkat_toolkit.auth.slack_oauth` exchanges an admitted authorization code or
refreshes a host-selected Slack user token, then compares provider observations
with an exact known user/workspace selection. It uses the existing `slack` extra's
HTTPX and AnyIO dependencies. It has no browser, callback listener, flow store,
refresh scheduler or credential persistence.

This is a host-only component, not an agent tool or released native OAuth
integration. The generic downstream connector identity/commit contract remains
in native issue #1631. Toolkit's provider strategy owns Slack identity semantics;
the native owner controls admitted attempts, credentials and permissions.
Returning a matching observation neither commits a credential nor authorizes a
Slack read or an email send.

## Host composition

```python
from meerkat_toolkit.auth.slack_oauth import (
    SlackReconnectSpec, authorization_request, exchange_and_observe,
)
from meerkat_toolkit.httpx_client import prepare_httpx_client

spec = SlackReconnectSpec(
    client_id=registered_public_client_id,
    app_id=registered_app_id,
    redirect_uri=exact_registered_localhost_redirect,
    expected_team_id=known_workspace_id,
    expected_user_id=known_user_id,
    expected_enterprise_id=known_enterprise_id_or_none,
    requested_scopes=frozenset({"channels:read", "channels:history"}),
)
authorization = authorization_request(
    spec, state=owner_state, code_challenge=owner_s256_challenge,
)
# Only the host's protected presenter receives authorization.url.
# The native owner validates callback state, exact redirect, expiry,
# cancellation and one-time admission before invoking this protocol step.
prepare_httpx_client(host_client)  # Before sharing or starting any calls.
result = await exchange_and_observe(
    spec, code=admitted_code, code_verifier=owner_verifier,
    http_client=host_client, timeout_seconds=20,
)
# Keep result host-private. The owner decides whether to commit and use it.
```

The host must establish an appropriately configured Slack public client with
PKCE enabled. The helper does not inspect or change app registration. Slack made
[PKCE generally available](https://docs.slack.dev/changelog/2026/03/30/pkce/) in
March 2026; enabling it is a one-way app change without Slack support.
Desktop redirects cannot request bot scopes. The first helper profile accepts
only `http://localhost:<explicit-port>/<path>`, without credentials, query or
fragment. It deliberately does not accept IP loopback spellings, dynamic port
substitution, HTTPS redirects or custom URI schemes. This follows the documented
localhost case, not an assumption that Google's redirect rules apply.
[Slack PKCE contract](https://docs.slack.dev/authentication/using-pkce/)

Use exact, case-sensitive IDs. App, workspace, user and optional enterprise IDs
are bounded alphanumeric strings; the helper does not infer them from names,
email, token prefixes or workspace links. `expected_enterprise_id=None` means
unpinned, not a claim that the workspace is outside an enterprise.

The requested set must be a nonempty subset of `channels:read`, `groups:read`,
`im:read`, `mpim:read`, `channels:history`, `groups:history`, `im:history`,
`mpim:history` and `users:read`. Select only the operations and conversation kinds needed.
Explicit `users:read` supports the optional `get_user` / `users.info` operation
for author and mention profiles. It is independent workspace metadata access,
not proof of channel membership. The helper never adds this scope automatically.
Names do not require `users:read.email`; that scope, write scopes and unknown
scopes remain refused by this helper. See [Slack users.info](https://docs.slack.dev/reference/methods/users.info/).
`auth_test` and `get_permalink` themselves need no extra scope; they do not cause
this helper to add one. No scope is inferred from the selected operations.
The authorization URL sends this set as `user_scope`, with empty bot `scope`
and a workspace hint. The hint does not replace response identity checks.

## One explicit refresh

The host can renew an expiring user token issued to the same public PKCE client
under this helper's desktop/localhost profile:

```python
from meerkat_toolkit.auth.slack_oauth import SlackRefreshSpec, refresh_and_observe

refresh_spec = SlackRefreshSpec(
    client_id=registered_public_client_id,
    app_id=registered_app_id,
    expected_team_id=known_workspace_id,
    expected_user_id=known_user_id,
    expected_enterprise_id=known_enterprise_id_or_none,
    required_scopes=frozenset({"channels:read", "channels:history"}),
)
# The host admits and serializes this attempt for its selected credential.
refreshed = await refresh_and_observe(
    refresh_spec, refresh_token=owner_refresh_token,
    http_client=host_client, timeout_seconds=20,
)
# The host checks current admission/policy, then commits the replacement pair.
# A returned result is not a credential-store write or a permission grant.
```

`SlackRefreshSpec` has no redirect or browser parameters. Its `required_scopes`
uses the same nonempty read-scope subset as `SlackReconnectSpec`. It describes
what the host needs from the renewed credential, without requesting new scopes.
Include `users:read` explicitly when the renewed credential must support user
lookup. A token-endpoint or present introspection report missing that required
scope is refused; the helper does not drop the requirement or retry.
The request sends exactly `client_id`, `grant_type=refresh_token` and the supplied
`refresh_token` to the fixed token endpoint. There is no client secret, code,
redirect, PKCE verifier, scope parameter or alternate endpoint. This follows
Slack's [public-client refresh contract](https://docs.slack.dev/authentication/using-pkce/),
not the confidential-client example that includes a secret.
The host is responsible for selecting a refresh token with that issuance
provenance; opaque token text cannot establish it. Confidential-client refresh
is outside this operation's contract.

The refresh response uses **top-level** `access_token`, `token_type`, `scope`,
`refresh_token` and `expires_in`. It must identify a user token and the selected
app and team. The replacement refresh token must be nonempty, bounded and
different from the submitted token; its positive integer lifetime is mandatory.
Nested `authed_user` credentials are not a refresh fallback. Slack's official
[refresh response type and handling](https://github.com/slackapi/node-slack-sdk/blob/9b53a7c9e98ef8f06f1956ba88321e217eb642be/packages/oauth/src/install-provider.ts)
define this separate response shape.

The refreshed response need not contain a user ID. The required `auth.test`
request uses exactly the new access token and must observe the selected user
and team before a result is returned. Any top-level `user_id` that is present
must also match; the authenticated probe's `bot_id` is refused. Ancillary token
response fields such as `bot_user_id` do not replace the required user token
type or identity probe. No user ID is inferred from
token text, an earlier credential or a nested response branch. The returned
`SlackProtocolResult` keeps the same account, enterprise, scope and timing
fields as code exchange. For refresh, `exchange_scopes` means the top-level
token endpoint report; `user_id` is proved by the new token's identity probe.

## Exact protocol and observations

There are three fixed destinations:

- Authorization URL: `https://slack.com/oauth/v2/authorize`
- One token POST: `https://slack.com/api/oauth.v2.access`
- One identity POST: `https://slack.com/api/auth.test`

For code exchange, the token POST sends client ID, code, exact redirect, PKCE verifier and
`grant_type=authorization_code`. There is no client-secret argument or fallback.
Only `authed_user.access_token` is used. Its `token_type` must be `user`, its ID
must match, and the exchange's app and team IDs must match the selection. Any
top-level bot token, bot scopes, expiry or refresh fields are ignored.
[Slack exchange response](https://docs.slack.dev/reference/methods/oauth.v2.access/)

`auth.test` uses exactly that user token. Its user/workspace IDs must match.
Returned app/token-kind fields cannot contradict the selection, and a `bot_id`
is refused. Any reported enterprise IDs must agree; a pinned enterprise must be
reported by the authenticated probe. The result preserves presence and value
of enterprise observations. Missing installation flags remain `None` (unknown),
explicit false remains false, malformed flags and explicit true are refused.
The helper does not claim workspace-only access or support org-wide installs.
[Slack identity observation](https://docs.slack.dev/reference/methods/auth.test/)

`authed_user.scope` for code exchange, or top-level `scope` for refresh, is
mandatory and must include the requested or required set. The
optional `x-oauth-scopes` header on the identity response is a separate report
of the token's current grants. If present, it must parse strictly and include
the requested set; its complete set becomes `granted_scopes`, with local enum
`ScopeEvidence.PROVIDER_INTROSPECTION` (`ProviderIntrospection`). If absent,
the complete exchange set becomes `granted_scopes` with
`TOKEN_ENDPOINT_RESPONSE` (`TokenEndpointResponse`). Both reports are preserved
separately. Surplus scopes are retained for owner policy. Sets are never united
or intersected; a malformed or insufficient present header cannot fall back to
the exchange. `x-accepted-oauth-scopes` is not grant evidence.
[Slack scope reporting](https://docs.slack.dev/authentication/installing-with-oauth/)

These enum values are local evidence labels, not native objects. No OIDC
issuer/subject, nonce or `at_hash` is asserted. Modern Sign in with Slack and
the separate `oauth.v2.user.access` dialect are not supported or auto-detected.
The first uses separate identity scopes; the second has a different response
shape. This helper never retries through either.

## Lifetime, failures and transport custody

For code exchange, nested user expiry and refresh fields must either both be absent or form a
valid positive integer lifetime and nonempty refresh token. Absence stays
`None`; it does not prove the token cannot expire or be revoked. `expires_in`
is the original reported duration. `expires_at` starts conservatively at token
POST entry, and validity is checked before and after the identity request using
wall and monotonic time. Consumers use that absolute expiry, never restart the
duration at handoff. No refresh expiry timestamp is invented.

Slack's [rotation contract](https://docs.slack.dev/authentication/using-token-rotation/)
requires replacement refresh tokens and limits active access tokens. The owner
must serialize refresh and commit replacement credentials. Each invocation
performs at most one token POST and one identity POST. The helper has no timer,
automatic refresh, retry, old-token fallback, credential revocation or new
consent flow. If observation fails after the refresh POST, the old refresh token
may already have been consumed and a replacement may have been issued. Neither
the absence of a result nor `response_received` establishes that the old token
remains usable. The host owns recovery of that uncertain credential state.

The host supplies a prepared, open, borrowed `httpx.AsyncClient`; it remains
host-owned after success, failure or cancellation. Explicit absolute requests
and empty HTTPX authentication avoid client default auth, cookies, headers and
query parameters. Responses may still update the client's cookie jar. Redirect
following is disabled per request. TLS verification, proxy configuration and
network policy remain host responsibilities. Hooks and transports are trusted
code and must not inject credentials, replay or retarget requests, bypass
response bounds or detach cleanup. Client options are not a sandbox.

Token and identity bodies are bounded to 64 KiB and 32 KiB. JSON nesting is
limited to 12 and nodes to 4,096; duplicate keys, invalid UTF-8, fractional or
non-finite numbers, unsupported encoding and inconsistent content lengths are
refused. Scope reports are bounded to 16 KiB and 128 distinct tokens, with
optional surrounding HTTP spaces/tabs. No raw provider body enters a result.

`SlackOAuthError` contains fixed reason, phase and `ExchangeState` values, plus
optional `provider_code`: `bad_redirect_uri`, `invalid_code`,
`invalid_refresh_token`, `pkce_not_allowed` or `None`. Only a complete, bounded
token-endpoint HTTP 200 JSON refusal with `ok:false` can supply a code. The
refusal must contain an exact recognized `error` and no fields other than
optional string `error_description` and `error_uri`. Descriptions and URIs are
discarded, never followed or exposed. Unknown, mixed, malformed or oversized
responses do not supply a code; existing reason, phase, state and exception
string behavior is unchanged. Identity-probe errors never supply token codes.
The [documented Slack token errors](https://docs.slack.dev/reference/methods/oauth.v2.access/)
can help a host explain a refusal, but do not establish whether an earlier
credential effect occurred or select an automatic reconnect/retry action.

`SlackOAuthCancelled` preserves cancellation with the phase and exchange state.
Neither retains secret exception objects. The states mean:

- `not_started`: token send was not entered.
- `unconfirmed`: token send was entered; no response headers were observed by
  the helper. The code or refresh token may already have been consumed.
- `response_received`: token response headers were observed. This does not
  imply successful authentication, validated identity or stored credentials.

After send entry, no error recommends replaying the code or refresh token. A
rejected identity, missing scope, failed probe or cancelled cleanup can follow
an already consumed input credential.
There is no automatic retry. Deadline checks precede each fetch; deadlines and
cancellation still join admitted response cleanup, which can finish later.
Repeated task cancellation and outer AnyIO cancellation do not detach it. The
host must supply cleanup that eventually settles.

Result/spec/authorization representations hide their fields. `safe_view()`
contains only observations and timing, without tokens, codes, verifier or raw
provider display fields. It is a fresh projection; Python object access is not
a security boundary. Never pass the full credential result into agent context.

Local fixtures can verify exchange/refresh behavior and feed a prepared client into
selected Slack MCP reads. They do not prove live app approval, real account
access, native credential persistence or completion of the Slack-to-email task.
