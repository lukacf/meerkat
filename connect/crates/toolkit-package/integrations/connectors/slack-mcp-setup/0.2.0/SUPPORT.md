# Slack 0.2.0: two explicit guidance strategies

This retains package `meerkat-toolkit/slack-mcp-setup`, export
`slack-mcp-setup.service` and role `service_connector`. Version `0.1.0` remains
unchanged. An agent chooses an exact version/digest and a strategy; rejection
never selects the other strategy. Both new recipes are `guidance_only`, without
an executable `mcp.json`. Neither creates an agent frontend or tool activation.

| Strategy | Concrete starting point | Remaining host work |
| --- | --- | --- |
| `provider-mcp-setup` | [Slack-hosted MCP documentation](https://docs.slack.dev/ai/slack-mcp-server/) | Eligible host app, user OAuth, selected account, live schemas and operation/lifecycle binding |
| `toolkit-rest-guidance` | Repository `sdk/python/src/meerkat_toolkit/connectors/slack.py` and [SLACK.md](../../../../../../sdk/python/SLACK.md) | Host-authenticated client, exact workspace/account/resource access and service-export binding |

The MCP route documents `https://mcp.slack.com/mcp` with Streamable HTTP. Slack
requires an eligible Marketplace or internal app and a fixed client identity;
unlisted distribution and dynamic client registration are not supported.
[Host setup](https://docs.slack.dev/ai/slack-mcp-server/connect-to-harnesses/)
requires this host's registration, protected client secret, redirect and any
workspace approval. The existing credential owner must handle user OAuth and
minimal selected tool scopes. Channel/thread reads use the relevant history
scope listed below on a user token; search/send permissions are separate. A documented endpoint does not establish account
or schema readiness. This recipe does not import every provider tool as an
allowed operation. It retains no live MCP execution evidence.

The REST slice exposes only `SlackOperations.history_page` and `replies_page`.
Each sends one read request through a borrowed authenticated `httpx.AsyncClient`.
The host fixes the base URL and credential binding, owns refresh/TLS/client
lifetime, and must use a separate client/operation instance for another account.
The library supplies no account-label registry, OAuth owner or ambient token
lookup. Host auth/hooks/transports must not retarget, replay or leak requests.
Redirects are refused; cancellation and deadline unwind retain their HTTP owner.
See the SDK document for the exact cooperative-cleanup limits and safe errors.

Select scopes for the actual conversation kind: `channels:history`,
`groups:history`, `im:history` or `mpim:history`. Current
[history](https://docs.slack.dev/reference/methods/conversations.history/) and
[replies](https://docs.slack.dev/reference/methods/conversations.replies/)
references list bot and user token scopes. History visibility differs by token
subject and membership; private conversations require the relevant membership,
and bot access is limited to conversations the bot can access. Do not apply an
old blanket bot/replies restriction or infer access from a token prefix. Verify
the chosen method, token, membership and resource against the current provider
and installation. [OAuth installation](https://docs.slack.dev/authentication/installing-with-oauth/)
distinguishes user and bot scope requests; [token types](https://docs.slack.dev/authentication/tokens/)
are provider subjects, not requester authorization supplied by this recipe.

Both methods preserve opaque cursors, exact timestamp strings, message ordering,
duplicates and nested payloads. The SDK does not discover channels or threads,
follow cursors, search, send messages or fetch attachments. A history time window
can miss replies to older threads. Short pages are not completion; the caller
must follow the provider's [pagination contract](https://docs.slack.dev/apis/web-api/pagination/)
and account for retention, access and concurrent changes. No page or collection
of selected conversations here proves "all updates for last week."

[Rate limits](https://docs.slack.dev/apis/web-api/rate-limits/) depend on the app,
workspace, method and installation category. Current method docs restrict new
commercial non-Marketplace installations to one request/minute and 15 items;
Marketplace/internal installations use Tier 3, and existing non-Marketplace
installations are described separately. Recheck the selected installation.
The SDK default of 15 is not quota evidence. HTTP 429 remains a typed failure
with bounded retry metadata; the SDK never retries or sleeps automatically.

## Local availability and retained proof

Python >=3.11 source is local and unpublished. The local `slack` extra declares
`httpx>=0.28.1,<0.29` and `anyio>=4.13,<5`. Select and verify repository source or
a private build artifact; this document does not instruct installing an assumed
PyPI release. No TypeScript Slack implementation or npm release is claimed.

The retained private `meerkat_toolkit-0.1.0-py3-none-any.whl` has SHA256
`af9b206f27fa2b9c175a40517216f219ed0a1c384704cf197ee85a49a8aa3452`.
Its Slack module SHA256 is
`266f9356d54e0ec6674c1c8f87bcf3e68993c06ff75d2722ac97840ab40baa18`.
The installed-wheel receipt is repository path
`docs/coordination/20260930-upstream-review/slack-python-operations/package-verification-1/installed-successor-1/summary.json`
(SHA256 `bc295bbdbd993aaa56ff1693270db6ba3aa577af2917b0c01ffcb19237151384`).
It records 29 actual loopback protocol cases against the exact installed wheel;
the source suite also passed 29 cases. These are offline transport/operation
results, not live Slack, authentication, agent activation or complete retrieval.
The original missing-test-fixture assembly failure remains in that evidence.

Toolkit-authored code and metadata are MIT OR Apache-2.0; both license texts are
retained in the private wheel. No Slack SDK implementation was copied. Slack
documentation/service terms remain separate. `recipes.json` binds eight exact
unauthenticated official documentation observations captured on 2026-10-01. The
hashes identify inspected response bodies, not immutable provider behavior.
Refresh current prerequisites before a separately authorized host activation.
