# Slack 0.3.0: agent-selected service setup

Package `meerkat-toolkit/slack-mcp-setup` retains export
`slack-mcp-setup.service` and the `service_connector` role. Versions 0.1.0 and
0.2.0 remain available unchanged. The agent selects a route and exact artifact;
this package supplies guidance, not an intent resolver or activated agent tools.

| Route | Starting point | Host work |
| --- | --- | --- |
| `provider-mcp-setup` | [Slack-hosted MCP](https://docs.slack.dev/ai/slack-mcp-server/) | Eligible app, selected user, OAuth, current schemas and operation/lifecycle binding |
| `toolkit-rest-guidance` | Local Python or TypeScript `SlackOperations` | Selected credential/transport, account/resource policy and published operation binding |

Both recipes remain `guidance_only`; neither includes executable `mcp.json`.
A denied route does not silently switch to another. These are service-access
routes. A Slack frontend, its agent routing, inbound admission and outbound
audience checks are separate capabilities.

For MCP, the documented endpoint is `https://mcp.slack.com/mcp` over Streamable
HTTP. A Marketplace or internal app with fixed identity and confidential user
OAuth is required; unlisted distribution and dynamic client registration are
not supported. The host's credential owner supplies the protected client secret,
registered redirect and secure user authorization UI. See the current
[host setup instructions](https://docs.slack.dev/ai/slack-mcp-server/connect-to-harnesses/).
The agent receives connection/action references and outcomes, not credentials.
An endpoint description does not prove this host is registered or ready.

## Five explicit REST operations

| Provider method | Python method | TypeScript method |
| --- | --- | --- |
| `auth.test` | `auth_test()` | `authTest()` |
| `conversations.list` | `list_conversations_page(...)` | `listConversationsPage(...)` |
| `conversations.info` | `get_conversation(channel, ...)` | `getConversation(channel, ...)` |
| `conversations.history` | `history_page(channel, ...)` | `historyPage(channel, ...)` |
| `conversations.replies` | `replies_page(channel, ts, ...)` | `repliesPage(channel, ts, ...)` |

Each call performs one request. Python uses a borrowed authenticated
`httpx.AsyncClient` prepared once with `prepare_httpx_client`; TypeScript uses
the host's selected Fetch function and base URL. The normal base is
`https://slack.com/`. Account selection, credentials, refresh, grants and
transport lifetime belong to the host. See the exact profiles in
[Python SLACK.md](../../../../../../sdk/python/SLACK.md) and
[TypeScript SLACK.md](../../../../../../sdk/typescript/SLACK.md).

The agent first asks the existing credential owner for the independently selected
workspace/principal binding. Compare `auth_test`/`authTest` results with those
expected IDs. A mismatch or unsupported identity response ends that attempt;
do not silently select another account. The supported profile requires
`team_id` and `user_id` and does not cover every organization-token response.
An authentication observation is not a grant or readiness proof for other
methods. The provider documents [auth.test](https://docs.slack.dev/reference/methods/auth.test/)
as requiring authentication without an additional method scope.

If discovery is needed, the agent chooses conversation types, filters and each
page, then uses exact IDs for detail and history. Listing and detail use the
applicable `channels:read`, `groups:read`, `im:read` or `mpim:read` permission;
message reads use the corresponding `:history` permission. Token subject,
membership and actual resource access still matter. A `team_id`/`teamId` query
can be ignored for workspace tokens, so it cannot establish identity. See
[list](https://docs.slack.dev/reference/methods/conversations.list/),
[info](https://docs.slack.dev/reference/methods/conversations.info/),
[history](https://docs.slack.dev/reference/methods/conversations.history/) and
[replies](https://docs.slack.dev/reference/methods/conversations.replies/).

Use a policy-permitted read of the selected resource when establishing that
operation's binding. Report success, identity mismatch, missing access,
unsupported response, rate limit or unavailable data to the existing readiness
owner. This guidance adds no health-check API, account registry or readiness
state. Provider conversation metadata does not attest current audiences.

The original agent chooses the user's time interval and timezone, conversations,
further pages and thread coverage. Cursors and decimal timestamps stay exact;
ordering, duplicates and unknown fields are retained. Empty or short pages can
have more data. A history window may omit recent replies to older threads.
Report retention, permission, pagination and budget limits rather than promising
all updates. Neither SDK traverses, resolves names, searches, sends Slack
messages or fetches attachments automatically. The original business task also
owns any later Gmail send; setup continuation must not repeat it.

Rate limits depend on the selected installation and method. Recheck
[current provider limits](https://docs.slack.dev/apis/web-api/rate-limits/)
and the [pagination contract](https://docs.slack.dev/apis/web-api/pagination/).
SDK defaults are request sizes, not quota evidence. Errors remain errors;
the libraries do not automatically delay, retry or change strategy.

## Exact local artifacts and evidence

Python requires 3.11 or later with the local `slack` extra. The verified private
wheel SHA-256 is
`0b3a80aa4ae1ae2ae01e459cb3cd4acce4c7a69bdd7241f02bda3dfcc111180e`.
Its acceptance is repository path
`docs/coordination/20260930-upstream-review/slack-service-discovery-root-review/installed-acceptance.json`,
SHA-256 `b2dc76a8cb92c6fcaf10817bc912e05686b9bf9d6a645df06b49e0c2f7453dc6`.
The unchanged 80 author and 45 independent cases passed from the installed wheel.

TypeScript requires Node 22 or later. The verified private `@rkat/toolkit` npm
archive SHA-256 is
`2cc66fba9db02c5c96f6e35d519faf43b672f6d32a154b70f77bbc574cfdbec4`.
Its acceptance is repository path
`docs/coordination/20260930-upstream-review/slack-typescript-discovery-root-review/installed-acceptance.json`,
SHA-256 `1f1aba3618fe813a89898e7a490531ab4279cafeefb270c86e2c331e89e555cf`.
The unchanged 45 author and 45 independent cases and two declaration checks
passed against installed root/subpath exports. Browser execution is not proven.

These groups overlap and do not represent a combined unique-feature count.
They establish bounded local operation and packaging behavior, not live Slack,
human OAuth, current authorization, agent continuation or complete retrieval.
Both packages remain unpublished. Use exact inspected source or these verified
private artifacts; version `0.1.0` alone does not identify their bytes. Do not
turn these records into assumed public package installation commands.

The Slack modules and this metadata are Toolkit-authored; no Slack implementation
was copied. Their licensing is MIT OR Apache-2.0. Consult each artifact's notices
for other included modules. Slack documentation and services retain their own
terms. `recipes.json` binds eleven official documentation response bodies
observed on 2026-10-02. Those hashes identify observations, not immutable hosted
behavior. Refresh prerequisites before host activation.

This additive version has passed bounded independent source review. Native registration,
parser/discovery execution and any catalog inventory revision remain with the
existing native owner. Package presence is not activation or supported-service
status; both strategies remain experimental.
