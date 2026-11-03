# Experimental Agno MCP profiles

This source package implements thirty-six explicit service profiles through the
official MCP Python SDK 2.2.0. The PostgreSQL-era Agno36 predecessor has
[retained installed proof](../../docs/verification/20261003-agno-postgres-batch/root-acceptance.json)
for 36 profiles and 111 exact runtime pins. Its digest is
`a6965820306cd00978687559859a0854aa900bbe19b99a2ebf0e15be80779ef6`.
The [support profile](../../docs/adoption/AGNO-SUPPORT.md) selects the current
supported artifact; each later edition needs its own explicit evidence binding.
These bounded components
run the real Agno 3.0.11 toolkit functions through the [bounded Agno bridge](AGNO.md), with
selected operations and profile-specific input limits. No Agno agent runtime,
Meerkat runtime, HTTP listener, discovery resolver or global tool registration
is started.

This is a concrete local stdio setup component. It does not complete Toolkit's
onboarding or beyond-library `find_and_install` flow.
Controlled protocol tests establish only the contracts they exercise. They do
not establish live provider availability, native host acceptance, account
activation, grants, task continuation or deployment readiness.

## Install the local artifact

Use Python 3.11 or later in an isolated environment. From this repository's
root, install the local SDK and its exact optional dependency profile:

```bash
python3 -m venv .venv-agno-mcp
.venv-agno-mcp/bin/python -m pip install './sdk/python[agno-mcp]'
```

The `agno-mcp` extra uses Agno 3.0.11, JSON Schema 4.26.0, MCP 2.2.0,
AnyIO 4.15.1, referencing 0.37.0, Requests 2.34.2, tavily-python 0.8.4,
python-gitlab 8.6.0, pytz 2026.4, linkup-sdk 0.23.1, todoist-api-python 4.0.0
and dataclass-wizard 0.39.1. The eight newer profiles additionally pin
exa-py 2.25.0, firecrawl-py 4.46.2, py-trello 0.20.1,
simple-salesforce 1.12.10, twilio 9.11.2, webexpythonsdk 2.0.6,
plivo 4.63.0, arxiv 4.0.1 and pypdf 6.19.0. The last dependency is required
by the pinned Agno arXiv module's import; no PDF operation is selected.
Agno29 preserved the prior 63 runtime constraints and added 33, for 96 total.
The Agno35 milestone additionally pinned PRAW 8.0.3, PRAWcore 4.0.0,
Tweepy 4.17.0, Oxylabs 3.0.0, boto3/botocore 1.43.108,
BeautifulSoup4 4.15.0 and Soupsieve 2.10. It uses the reviewed OAuthlib
3.3.1 replacement because Tweepy requires >=3.2,<4. Its complete graph has
109 pins: 95 prior versions unchanged, one replacement and 13 additions.
The accepted Agno29 and Agno35 graphs are immutable. The Agno36 PostgreSQL milestone added
psycopg 3.3.6 and psycopg-binary 3.3.6, both LGPL-3.0-only external runtime
distributions. Its 111-pin source environment preserves all 109 Agno35 versions.
The adapter calls the real driver without copying or modifying its source. These optional dependencies are not
added to core Meerkat or MobKit. The [per-profile SDK closure audit](../../docs/verification/20261003-agno-library-batch/dependency-audit.json)
records the shared dependencies and each SDK's declared closure for future
optional install groups without duplicating transport ownership.
The [accepted Agno29 installed proof](../../docs/verification/20261003-agno-library-batch/INSTALLED-VERIFICATION.md)
records the exact wheel, dependency graph and donor-source comparisons. This
command builds a local artifact; it does not assume Toolkit is published to
PyPI. Keep the package's license notices and deployment policy in scope. The
Agno source commit reviewed for these functions is
`8c3d8ec52b4a13c410fca97d07fbd507790cf29a`; source-checkout and registry-artifact
verification are separate evidence records, even when their version labels
match.

## Select a profile

### Inspect configuration without credentials

This configuration-assistance edition includes three inert commands. The
PostgreSQL-era Agno36 predecessor with SHA-256
`a6965820306cd00978687559859a0854aa900bbe19b99a2ebf0e15be80779ef6` does not contain
them. The [support profile](../../docs/adoption/AGNO-SUPPORT.md) selects the current
supported artifact. These commands do not install, authenticate, activate or
contact a provider.

```sh
meerkat-toolkit-agno-mcp profiles
meerkat-toolkit-agno-mcp config-help --profile postgres
meerkat-toolkit-agno-mcp check-config --config /absolute/path/profile.json
```

`profiles` lists exact supported IDs and selectable operation names.
`config-help` reports top-level fields and the selected profile's `config_fields`,
including primitive kinds, required/optional presence and credential references.
Choose the account and values explicitly. A credential-reference field contains
an environment-variable name, never its secret value. The help is not a complete
JSON Schema; the profile sections below describe semantic restrictions and defaults.

This edition also reports `prerequisites` as `{kind, instruction}` rows
and `unresolved_details` as strings. They use the existing Toolkit newcomer-prerequisite
vocabulary and describe the exact selected profile. For example, Spotify's
`api_key_env` means an already acquired OAuth access token, Oxylabs uses that
same field name for an account password, and AWS Lambda requires a matching
access key ID/secret key pair with an optional session token. Public profiles
declare that they have no credential input; PubMed's contact email is not a
credential. PostgreSQL instead requires a separately provisioned principal
and local SCRAM configuration.

This guidance distinguishes access to the local stdio MCP process from the
connector's credential relationship with its provider. It does not turn a
service token into authorization for a remote MCP server. Reference names are
for host-owned injection, not requests to paste secrets into a model or tool.
Registration, administrator approval, OAuth application/callback ownership and
scope details that the retained guide does not establish remain explicit
unresolved prerequisites. No account lookup, OAuth application, registration,
token acquisition/refresh, browser launch or verification is supplied by help.

`check-config` uses the same configuration validator as `describe` and `serve`,
without constructing a toolkit, importing runtime dependencies or reading a
credential. Success returns one JSON object with `valid: true` and
`scope: "configuration_only"`; failure returns a nonzero exit code and a generic
diagnostic without submitted values. It leaves the file unchanged. It does not
check whether a referenced credential exists or whether the account can access
the requested resource. Continue with the existing `describe` and `serve` commands
only after the host supplies the selected credential. `describe` still constructs
the selected toolkit and therefore needs credentials for authenticated profiles.

The current source also adds `credential_environment_references` to a successful
`check-config` result. This additive field needs its own accepted artifact; it
is not present merely because an earlier frozen release supports `check-config`.
For example, a selected Trello configuration reports:

```json
{"credential_environment_references":[{"field_path":["config","api_key_env"],"environment_variable":"SELECTED_TRELLO_KEY"},{"field_path":["config","token_env"],"environment_variable":"SELECTED_TRELLO_TOKEN"}]}
```

Each path is an array of literal JSON keys, not a dotted path or expression.
Rows come only from the existing help's `credential_reference` declarations and
the same validated configuration. Public profiles return `[]`; absent optional
references are omitted. Names retain their exact spelling and case, and aliases
produce one row per field. No credential value or ambient environment is read.
Use `config-help` for credential meaning and prerequisites. The host freezes the
configuration and owns lookup, secret handling and protected-environment policy;
even a reference named `PATH` does not authorize overwriting it. This projection
does not bind an account or establish authorization, authentication or readiness.

The standalone component retains this guide as
`meerkat_toolkit_agno/PROFILE-GUIDE.md`. The combined source module exposes the
same commands through `python -m meerkat_toolkit.adapters.agno_mcp`.

Save a JSON file outside model-writable input. This Hacker News example selects
only top stories:

```json
{
  "profile": "hackernews",
  "operations": ["get_top_hackernews_stories"],
  "config": {"timeout": 10}
}
```

Use `get_user_details` as an additional selected operation when needed. The
PubMed configuration requires an explicit operator-supplied contact email;
replace the example before any real provider call:

```json
{
  "profile": "pubmed",
  "operations": ["search_pubmed"],
  "config": {
    "timeout": 10,
    "email": "operator@example.invalid",
    "max_results": 10,
    "results_expanded": true
  }
}
```

The parser reads at most 64 KiB. It rejects unknown keys, duplicate JSON keys,
nonfinite numbers, missing or duplicate operations, unknown profile names and
invalid types. Configuration failures report a fixed bounded message without
the file's values. The accepted profile names are `hackernews`, `pubmed`,
`searxng`, `tavily`, `financial-datasets`, `openweather`, `bitbucket`,
`gitlab`, `clickup`, `discord`, `shopify`, `spotify`, `unsplash`, `calcom`,
`jina`, `openrouteservice`, `zendesk`, `bright-data`, `linkup`, `todoist`,
`zoom`, `exa`, `firecrawl`, `trello`, `salesforce`, `twilio`, `webex`, `plivo`,
`arxiv`, `reddit`, `x`, `youtube`, `oxylabs`, `wikipedia` and `aws-lambda`;
there is no arbitrary module or factory string.

| Setting | Required or default | Accepted values |
| --- | --- | --- |
| `timeout` | Required for every profile | Integer 1-60 seconds per HTTP request |
| `email` | Required for PubMed | Explicit non-whitespace contact address with one `@`, at most 254 characters |
| `max_results` | PubMed default 10 | Integer 1-50 |
| `results_expanded` | PubMed default `true` | Boolean |
| `host` | Required for SearXNG and GitLab | Explicit HTTPS origin; no userinfo, path, query or fragment |
| `api_key_env` | Required for every credentialed profile | Exact host-supplied environment variable name; 1-128 ASCII letters, digits or `_`, starting with a letter or `_` |
| `units` | OpenWeather default `metric` | `standard`, `metric` or `imperial` |
| `account_email` | Required for Bitbucket and Zendesk API-token mode; forbidden for Zendesk OAuth | Explicit ASCII account email, at most 254 characters; no colon or whitespace |
| `workspace`, `repo_slug` | Required for Bitbucket | Each 1-128 ASCII letters, digits, `_`, `.` or `-`, starting with a letter or digit; no URL or path separators |
| `workspace_id` | Required for ClickUp | Explicit workspace/team ID, 1-20 ASCII digits; passed as the donor's `master_space_id` |
| `shop_name`, `api_version` | Required for Shopify | Lowercase DNS label, at most 63 characters, without `.myshopify.com`; explicit `20YY-01`, `20YY-04`, `20YY-07` or `20YY-10` version |
| `market` | Required for Spotify | Explicit two-uppercase-letter market code; syntax does not establish provider availability |
| `event_type_id`, `user_timezone` | Required for Cal.com | Integer event type 1-2147483647 and explicit IANA timezone supported by pinned pytz |
| `max_content_length`, `search_query_content` | Jina defaults 10000 and `true` | Integer character limit 1-100000 and Boolean full-search-content choice |
| `auth_mode` | Zendesk defaults to `api_token` | `api_token` or `oauth_access_token`; select the credential type explicitly |
| `subdomain` | Required for Zendesk | One lowercase DNS label, at most 63 characters, without `.zendesk.com` |
| `web_unlocker_zone`, `serp_zone` | Required for Bright Data | Each 1-64 ASCII letters, digits, `_` or `-`; explicit selection overrides ambient donor zone variables |
| `depth`, `output_type` | Linkup defaults `standard` and `searchResults` | `standard` or `deep`; `searchResults` or `sourcedAnswer` |
| `token_env` | Required for Trello in addition to `api_key_env` | Exact host-supplied reference to the selected user token; same variable-name syntax as `api_key_env` |
| `instance_url` | Required for Salesforce | Explicit HTTPS origin; no userinfo, path, query or fragment; hostname case and default port 443 are normalized |
| `account_sid` | Required for Twilio | `AC` followed by 32 hexadecimal characters |
| `auth_id` | Required for Plivo | `MA` or `SA` followed by 18 ASCII letters or digits, matching the pinned SDK's account-ID check |
| `user_agent` | Required for Reddit | 7-200 printable ASCII characters, starting without whitespace; explicit application/contact identity |
| `include_post_metrics` | X default `false` | Boolean |
| `username` | Required for Oxylabs | 1-128 ASCII letters, digits, `_`, `.`, `@` or `-` |
| `language` | Required for Wikipedia | 2-3 lowercase ASCII letters, optionally followed by `-` and 2-8 lowercase letters |
| `region` | Required for AWS Lambda | Explicit commercial region matching the profile's syntax; not a provider availability check |
| `access_key_id_env`, `session_token_env` | AWS Lambda requires access key reference; session token reference is optional | Same environment-reference syntax as `api_key_env`; the latter references the secret access key |

The email check is a bounded format check, not verification of ownership or
deliverability. PubMed receives that configured email in its search request.

For SearXNG, select an existing instance whose JSON API is enabled. Replace the
placeholder origin with that explicit instance; no instance is deployed or
chosen automatically:

```json
{
  "profile": "searxng",
  "operations": ["search_web", "news_search"],
  "config": {"timeout": 10, "host": "https://search.example.invalid"}
}
```

Tavily and Financial Datasets take a nonsecret reference to an explicit key
provided in the MCP child process's environment. Obtain the corresponding
provider account key and configure the MCP host to inject that value. The JSON
contains only its chosen variable name:

```json
{
  "profile": "tavily",
  "operations": ["web_search_using_tavily"],
  "config": {"timeout": 10, "api_key_env": "TOOLKIT_TAVILY_KEY"}
}
```

```json
{
  "profile": "financial-datasets",
  "operations": ["get_income_statements", "get_balance_sheets"],
  "config": {"timeout": 10, "api_key_env": "TOOLKIT_FINANCIAL_DATASETS_KEY"}
}
```

The configured variable must exist and contain 1-4096 printable ASCII
characters without spaces. No default provider variable is selected when it
is absent. The key is resolved only while constructing the selected profile;
it is not a tool argument, schema field or description value. `describe` also
requires that explicit key because it constructs the real toolkit. It makes
no provider request and does not substitute a dummy production key.

These profiles use the fixed official API origins. Tavily performs basic
search without answer generation, extraction or context mode. Financial
Datasets exposes the two selected statement reads and requires the relevant
dataset entitlement. Account setup and provider billing remain the host's
and account owner's responsibility; loading a key does not establish a grant.

The three new account profiles use the same explicit environment-reference
setup. The values below are configuration examples, not credentials:

```json
{
  "profile": "openweather",
  "operations": ["get_current_weather", "get_forecast", "get_air_pollution", "geocode_location"],
  "config": {"timeout": 10, "api_key_env": "TOOLKIT_OPENWEATHER_KEY", "units": "metric"}
}
```

Create an OpenWeather account key with access to current weather, the
[5-day forecast](https://openweathermap.org/api/forecast5),
[air pollution](https://openweathermap.org/api/air-pollution) and
[direct geocoding](https://openweathermap.org/api/geocoding-api) endpoints
needed by the selected operations. These use `api.openweathermap.org`.
Each weather read geocodes the supplied location and uses the first returned
match; ambiguous names are not resolved by an additional agent or matcher.
Use the geocoding operation to inspect matches when needed. Provider key
activation, plan access and billing remain account-owner facts.

```json
{
  "profile": "bitbucket",
  "operations": ["list_repositories", "get_repository_details", "list_all_pull_requests", "get_pull_request_details"],
  "config": {
    "timeout": 10,
    "api_key_env": "TOOLKIT_BITBUCKET_API_TOKEN",
    "account_email": "operator@example.invalid",
    "workspace": "your-workspace",
    "repo_slug": "your-repository"
  }
}
```

Replace the email/workspace/repository with the explicitly selected account
and resources. Bitbucket Cloud uses an API token with the account email in
[HTTP Basic authentication](https://developer.atlassian.com/cloud/bitbucket/rest/intro/#api-tokens),
at `api.bitbucket.org/2.0`. Select the
[`read:repository:bitbucket`](https://developer.atlassian.com/cloud/bitbucket/rest/api-group-repositories/)
and, for pull-request reads,
[`read:pullrequest:bitbucket`](https://developer.atlassian.com/cloud/bitbucket/rest/api-group-pullrequests/)
scopes. The token must also have access to those resources. Repository and
pull-request lists return only their first page. The preserved upstream
name `list_all_pull_requests` does not mean complete enumeration. Commits,
diff redirects, writes and the retired Bitbucket issue API are excluded.

The current source bounds each Bitbucket response body to 1 MiB and closes it
after reading or refusal. It requests `Accept-Encoding: identity`; unexpected
compressed responses are refused before reading. Status failures are also
refused before body consumption. A missing Content-Length does not remove the
byte limit. Fixed-length and chunked bodies use the installed urllib3's public
`read1` interface so continuously arriving body bytes cannot postpone the
elapsed-time check until a large requested chunk is full.

The selected `timeout` remains the socket inactivity timeout and also supplies
the elapsed body-read budget, starting before the request. A read already in
progress can finish after that budget, then fails before returning a result.
This is not a hard total wall-clock bound on synchronous DNS, connection setup,
response headers, HTTP chunk framing, custom transports or cleanup. MCP
cancellation waits for the owned worker and response cleanup; it does not
detach a still-running request. These source changes require a separately
accepted successor artifact; earlier frozen wheels retain their original behavior.

```json
{
  "profile": "gitlab",
  "operations": ["list_projects", "get_project", "list_merge_requests", "get_merge_request", "list_issues"],
  "config": {
    "timeout": 10,
    "api_key_env": "TOOLKIT_GITLAB_TOKEN",
    "host": "https://gitlab.com"
  }
}
```

Supply a GitLab access token accepted as `PRIVATE-TOKEN`, with API read access
to the chosen [projects](https://docs.gitlab.com/api/projects/),
[merge requests](https://docs.gitlab.com/api/merge_requests/) and
[issues](https://docs.gitlab.com/api/issues/). A personal access token with
[`read_api`](https://docs.gitlab.com/security/tokens/access_token_scopes/)
is the supported setup. For self-managed GitLab, set its explicit
HTTPS origin; deployments below a URL subpath are outside this profile.
Project arguments are IDs or unencoded `group/project` paths. The SDK encodes
them; do not pass URLs or percent-encoded paths. Only synchronous donor
methods are selected. The donor's async client and runtime are not started.

## Describe, then attach to a stdio host

Describe the selected schemas without a provider request:

```bash
.venv-agno-mcp/bin/python -m meerkat_toolkit.adapters.agno_mcp \
  describe --config /absolute/path/hackernews.json
```

`describe` constructs only the selected reviewed toolkit and prints one JSON
metadata document. It does not invoke an operation. Configure the MCP host to
launch the same interpreter with these arguments:

```text
-m meerkat_toolkit.adapters.agno_mcp serve --config /absolute/path/hackernews.json
```

Use an absolute interpreter path in the host configuration, such as
`/absolute/path/repository/.venv-agno-mcp/bin/python`. The host owns the child
process, environment, configuration file and MCP connection. The server
claims stdio before constructing the selected Agno toolkit. MCP frames use the
claimed transport; ordinary upstream stdout is redirected to stderr by MCP
2.2. Do not add shell pipelines or banners to the transport command.

Hacker News, PubMed and SearXNG perform public reads and require no provider
credential. They use upstream `httpx.get` directly with per-request clients. They do not accept
a borrowed authenticated transport. The host process must supply network
policy, proxy/CA environment policy and any outbound restrictions. Upstream
logging can contain provider diagnostics; the host also owns stderr storage
and access. The adapter's redacted MCP error envelope does not sanitize
upstream logs.

Tavily owns its actual upstream SDK client. The profile configures that
instance to ignore ambient netrc/proxy/CA settings, removes ambient Tavily
organisation/project headers and refuses redirects and automatic retries.
Financial Datasets uses a narrow subclass that overrides only constructor
transport plumbing and the request helper to borrow a profile-owned Requests
session with the same settings. Its selected upstream operation bodies remain
inherited. The helper keeps the provider result and fixed failure convention,
but omits raw exception diagnostics. It is an attributed Apache-2.0 source
adaptation, not an unchanged upstream artifact. Neither profile provides a
custom proxy or CA option; choose a different supported host setup if one is
required. The CLI closes owned sessions after its worker/server has joined.

OpenWeather and Bitbucket use the same borrowed-session helper pattern and
retain all selected donor method bodies. GitLab retains those bodies plus
the real python-gitlab managers and serializers, with an instance subclass
that disables SDK rate-limit/transient retries and redacts request exceptions.
These three profiles own one Requests session each: normal TLS verification,
no ambient proxy/netrc/CA override, no redirects, no automatic retries. All
credential constructor inputs are explicit, so upstream environment fallbacks
are not selected. They also provide no custom proxy or CA option. The host
owns session cleanup after the joined worker/server stops. Original donor
logging can still disclose location/project/filter values or errors outside
the adapted request helper; the host must manage stderr accordingly.

### ClickUp, Discord, Shopify, Spotify and Unsplash

These profiles use the same configuration format and `api_key_env` reference.
ClickUp needs its selected workspace and API key. Discord needs a bot token
with access to the requested channel or guild. Shopify needs the selected
shop's Admin API token; product reads require access to products. Spotify
needs an already acquired OAuth access token; top-track reads use the donor's
documented `user-top-read` scope, and playlist visibility follows that token.
Unsplash needs its selected API access key. The component does not acquire or
refresh tokens, discover accounts, or validate scopes during construction.

For example, this Shopify configuration selects product reads:

```json
{
  "profile": "shopify",
  "operations": ["get_products"],
  "config": {
    "timeout": 10,
    "api_key_env": "TOOLKIT_SHOPIFY_TOKEN",
    "shop_name": "your-shop",
    "api_version": "2025-10"
  }
}
```

The version is an explicit example, not an assertion that the provider still
supports it. Choose a version supported by the selected account. For ClickUp,
replace the shop fields with `"workspace_id": "123"`; for Spotify use
`"market": "SE"`; Discord and Unsplash need no additional configuration
fields. Select operations from the table below and a credential reference for
that specific service. An absent reference never falls back to donor defaults.

All five keep their selected Agno operation bodies and response formatting.
Only request helpers are adapted to one profile-owned Requests session,
normal TLS verification, no ambient proxy/netrc/CA overrides, no redirect or
automatic retry, and a 1 MiB cap on decoded response bytes. Responses close
on every path, including a Requests redirect exception raised before a
response context is entered. The timeout bounds socket waits; elapsed-time
checks reject a response that finishes late but cannot interrupt a blocked
worker. Existing worker joining and profile cleanup rules still apply.
GraphQL errors in an HTTP-success response remain failures. Shopify uses
POST for its selected read-only queries.

These are service connectors. Discord history access does not install an
agent frontend or router. Writes, image downloads, playback, ClickUp name
resolution/first-list selection, and automatic pagination are not exposed.
Shopify returns the first product page and first 10 variants per product;
Spotify returns first pages. Its playlist projection accepts current `items` and legacy `tracks` count containers; unavailable counts are `null`, and conflicting or malformed counts are refused. Unsplash preserves attribution links and returns
image metadata, not image bytes or permission to reuse a photograph.

### Cal.com, Jina Reader, OpenRouteService and Zendesk Help Center

These four donors perform HTTP inside their operation bodies. The component
therefore contains explicit source ports of seven operations, identified by
source digest in `agno_transports.py` and the third-party notices. They use
the real installed Agno Toolkit classes and their pure parsing/formatting
helpers. No donor globals are patched. The declared deltas are the selected
owned Requests transport, additional bounds, fixed errors without provider
diagnostics, and the validation described below. Each response has the same
1 MiB decoded-byte cap, actual socket timeout, closure and joining contract as
the five service helpers above. No next-page URL is followed.

Cal.com requires a selected API key, explicit numeric event type and IANA
timezone. The 2026-10-04 compatibility candidate below is awaiting component
review and promotion; the accepted Zendesk-era wheel retains its earlier Cal.com
contract. See the [candidate evidence](../../docs/verification/20261004-calcom-api/component/REPORT.md).

```json
{
  "profile": "calcom",
  "operations": ["get_available_slots", "get_upcoming_bookings"],
  "config": {
    "timeout": 10,
    "api_key_env": "TOOLKIT_CALCOM_TOKEN",
    "event_type_id": 123,
    "user_timezone": "Europe/Stockholm"
  }
}
```

`event_type_id` is an integer from 1 to 2147483647. Availability queries retain
the donor's inclusive **UTC dates**: `start_date` at `00:00:00Z` through
`end_date` at `23:59:59Z`. They are not selected-timezone calendar boundaries.
The port checks real dates, ordering and at most 31 inclusive dates before
HTTP. Schema checks cover date spelling; cross-field/calendar checks occur
inside the port before HTTP. Returned instants are formatted by the inherited
timezone helper, including daylight-saving transitions. Naive provider
timestamps are rejected. Query years, returned UTC years and formatted local
years must fall in 1970-2037 because the retained `pytz` helper has a
[documented post-2037 limitation](https://pypi.org/project/pytz/2026.4/).
The pinned dependency is `pytz==2026.4`, a pure-Python dependency of this
successor, not a change to any earlier accepted wheel or dependency lock.
The candidate uses `GET /v2/slots` with `cal-api-version: 2024-09-04`, explicit
`start`/`end` UTC boundaries, `eventTypeId`, `timeZone=UTC` and `format=range`.
Each date in `data` must contain a list of objects with aware `start` and `end`
timestamps. Ranges must increase and their starts must match the requested UTC
date bucket. Malformed ranges are errors; an empty mapping or empty list is valid.
Selecting range format avoids the provider page's conflicting descriptions of
the default format. See the [official slots contract](https://cal.com/docs/api-reference/v2/slots/get-available-time-slots-for-an-event-type).

Upcoming bookings retain one page for the selected account, optionally filtered
by `email`. The configured event type does **not** filter that operation. The
candidate uses `GET /v2/bookings`, `cal-api-version: 2026-05-01`, `status=upcoming`
and `limit=100`. It validates `pagination.hasMore` and `nextCursor` and explicitly
marks a nonterminal page as incomplete, including an empty nonterminal page.
It never follows or returns the cursor. UID, title, local time and status keep
the donor's formatting. See the [official bookings contract](https://cal.com/docs/api-reference/v2/bookings/get-all-bookings).
Both reads require HTTP 200 and a successful response status. No booking writes
are exposed, and offline contract checks do not establish live account access.

Jina Reader requires an explicit API key and uses fixed `https://r.jina.ai/`
and `https://s.jina.ai/` origins:

```json
{
  "profile": "jina",
  "operations": ["read_url", "search_query"],
  "config": {
    "timeout": 10,
    "api_key_env": "TOOLKIT_JINA_TOKEN",
    "max_content_length": 10000,
    "search_query_content": false
  }
}
```

`max_content_length` defaults to 10000 and accepts 1-100000 characters.
`search_query_content` defaults to the donor's `true`; `false` sends its
`X-Respond-With: no-content` header. Both operations request and parse JSON,
then apply the donor's Python `str(value)` conversion and inherited character
truncation, including the `... (content truncated)` suffix. The output is not
promised to be JSON text. A plaintext provider response is an error; there is
no new plaintext fallback. The configured timeout is enforced on HTTP as well
as sent in the donor's `X-Timeout` header. `read_url` accepts an explicit
absolute HTTP(S) URL, without userinfo or malformed ports, sent through Jina;
the Toolkit process does not fetch that target directly.

OpenRouteService uses `"profile": "openrouteservice"`, operations
`["geocode_location", "get_directions"]`, and only the common required
`timeout`/`api_key_env` settings. Geocoding retains the donor's first-feature
selection and coordinates/label output. Directions geocode the start and end,
then request one route and retain the donor's rounded distance/time summary.
The maximum is one request for geocoding or three for directions. Coordinate
results must be finite and within longitude/latitude bounds. Route summaries
require the provider's explicit finite, nonnegative distance and duration; absent,
boolean or invalid metrics are errors instead of successful zero-length routes.
An explicit zero for both metrics remains valid. All requests use
the fixed `https://api.heigit.org` origin: geocoding uses `/pelias/v1/search`,
and directions use `/openrouteservice/v2/directions/{profile}`. This replaces
the previous origin and requires permission under the host's network policy.
There is no legacy-host fallback or arbitrary-origin configuration. The donor's geocoding
protocol carries the selected key in its query; error diagnostics never
reflect that URL. The source's road-routing profile enum is exposed, with no
distance matrix or public-transit claim.

Zendesk accepts an existing OAuth access token for the selected Help Center:

```json
{
  "profile": "zendesk",
  "operations": ["search_zendesk"],
  "config": {
    "timeout": 10,
    "api_key_env": "TOOLKIT_ZENDESK_ACCESS_TOKEN",
    "subdomain": "your-site",
    "auth_mode": "oauth_access_token"
  }
}
```

`api_key_env` names the host-supplied credential in both modes. OAuth mode sends
`Authorization: Bearer` and forbids `account_email`. The host owns token
acquisition, renewal, instance, identity and scope selection. The credential is
captured when the profile is built; the host must rebuild it to replace a token.
There is no automatic authentication fallback.

For an eligible legacy account, omit `auth_mode` or select `api_token`, and add
`account_email`. This preserves HTTP Basic authentication using the explicit
email plus `/token` and the selected API token. `subdomain` is one lowercase DNS
label. Neither mode expands the selected operation or its provider permissions.
Only `https://<subdomain>.zendesk.com/api/v2/help_center/articles/search.json`
is used. The first response's article bodies retain the donor's regex-based
HTML removal and JSON-list serialization. That regex is not an HTML sanitizer
and does not decode entities. The response must contain an actual article list;
an empty list is valid, but malformed objects or strings are errors. No ticket
API or agent frontend is included.

### Bright Data, Linkup, Todoist and Zoom

These four source profiles use the same selected key reference and owned
Requests session. Existing JSON profiles retain their strict UTF-8/JSON
behavior above the shared bounded byte reader. New SDK clients borrow that
I/O owner; no extra socket transport, redirect or automatic retry is started.
The three new exact dependencies are `linkup-sdk==0.23.1` (MIT),
`todoist-api-python==4.0.0` (MIT) and `dataclass-wizard==0.39.1` (Apache-2.0).
The prior 60 runtime constraints remain unchanged.

Bright Data requires the explicitly selected Web Unlocker and SERP zones:

```json
{
  "profile": "bright-data",
  "operations": ["scrape_as_markdown", "search_engine"],
  "config": {
    "timeout": 10,
    "api_key_env": "TOOLKIT_BRIGHT_DATA_TOKEN",
    "web_unlocker_zone": "your_web_zone",
    "serp_zone": "your_serp_zone"
  }
}
```

Its original Agno operation bodies construct the request payload. The helper
sends it to fixed `https://api.brightdata.com/request`, with an HTTP-200
condition, a 1 MiB byte cap before UTF-8 decoding and a JSON `content` envelope
for successful Markdown. This envelope keeps actual page text distinct from
the donor's error strings, including a page whose title starts with an error
prefix. Target URLs are passed to Bright Data and are not fetched directly
by the Toolkit process. Absolute HTTP(S) URLs are required; userinfo, malformed
ports, backslashes, controls and lone surrogates are rejected. The real donor
search supports Google, Bing and Yandex. Its count/language/country hints
apply only to Google; other engines ignore them. There is no screenshot,
media attachment, dataset-job trigger or polling support in this profile.
The constructor's ambient-zone override is explicitly replaced with the
selected configuration after initialization. Donor logs still contain the
requested target URL or query; the host owns stderr access.

Linkup needs only the common settings; optional configuration chooses
`depth` (`standard` or `deep`) and `output_type` (`searchResults` or
`sourcedAnswer`). Its inherited `web_search_with_linkup` also accepts either
choice per call. The real Linkup SDK constructs parameters and validates
its Pydantic search/answer model. An SDK subclass replaces only request I/O
and projects the validated model with `model_dump(mode="json")`, because the
original donor returns a model object despite its string annotation. Empty
result lists remain valid; malformed response collections are failures. The
fixed endpoint is `https://api.linkup.so/v1/search`. No structured-output
schema, async research job, x402 payment or image download is exposed.

Todoist uses `"profile": "todoist"`, operations `["get_task"]`, and the
common settings. The constructor is an explicit small port so it can inject
the real SDK's public `client=` argument before any I/O; the actual inherited
Agno `get_task` and `_task_to_dict` bodies are unchanged. The real SDK's
`get_task` constructs its `Task` model through an HTTPX transport that borrows
the sole Requests session. HTTPX has no independent socket route. Response
identity and consumed scalar/collection shapes are checked before the SDK
can coerce malformed fields. SDK parsing errors are sanitized before the
donor can log them. The result remains the donor's JSON-encoded task string,
including its due/date conversion. This exposes one task lookup, without
list pagination, project enumeration or writes.

Zoom uses `"profile": "zoom"`, operations `["list_meetings", "get_meeting"]`,
and the common settings. The key reference must select an already acquired
Zoom bearer token. Its constructor and two read bodies are explicit source
ports of the pinned donor, with fixed `https://api.zoom.us/v2` paths and no
OAuth endpoint, token refresh, scheduling, deletion or recording access.
Listing returns the first provider page and never follows its next token.
Malformed meeting collections/items fail. A detail response must contain the
requested ID and topic. Optional metadata retains provider absence, rather
than the donor's fabricated empty/zero defaults. The inherited donor's JSON
string output shape is otherwise retained. Tokens expiring during use produce
ordinary request failures; the host owns any subsequent authentication work.

### Exa, Firecrawl, Trello, Salesforce, Twilio, Webex, Plivo and arXiv

These eight profiles run actual SDK operation bodies and response models
through one shared, byte-bounded Requests owner. SDK clients borrow that
owner; the selected routes never use their default socket transport. They
require an actual HTTP 200, reject redirects and do not retry. JSON or Atom
shape checks run before the SDK can coerce malformed data into success.
All profile failures use the existing redacted MCP error surface.

Exa, Firecrawl and Webex need the common timeout and key reference only:

```json
{
  "profile": "exa",
  "operations": ["search_exa"],
  "config": {"timeout": 10, "api_key_env": "TOOLKIT_EXA_KEY"}
}
```

For Firecrawl select `"profile": "firecrawl"`, operations
`["scrape_website", "map_website"]`, and its own key reference. For Webex use
`"profile": "webex"`, operations `["list_rooms"]`, and a reference to an
already acquired Webex access token.

Exa inherits `search_exa`; its real SDK constructs search parameters and
`SearchResponse`/`Result` instances. The donor requests text, truncates each
result's text at 10,000 characters and returns a JSON string. The SDK request
method is replaced, while its actual search/model code remains in use. Its
thread-pool timeout does not forcefully interrupt blocking I/O; the shared
server joins work before retiring resources. No research jobs or answer
generation are included.

Firecrawl inherits both selected operations and uses the real v2 `Document`
and map-result models. An instance-local HTTP-client subclass supplies bounded
I/O through `/v2/scrape` and `/v2/map`. Markdown scrape results require an actual
Markdown string, including valid empty content. Map results require a link
list of at most 100 entries. The requested target URL is sent as provider
data, never fetched by this process. Crawls, browser actions, polling and
downloads are excluded. The SDK's output includes optional model fields as
produced by that SDK; it is not flattened into an invented response shape.

Trello requires two distinct selected references:

```json
{
  "profile": "trello",
  "operations": ["list_boards"],
  "config": {
    "timeout": 10,
    "api_key_env": "TOOLKIT_TRELLO_KEY",
    "token_env": "TOOLKIT_TRELLO_USER_TOKEN"
  }
}
```

Its constructor is a small explicit port to use the real `TrelloClient` public
`http_service` argument. The pinned SDK's non-OAuth mode takes the selected user
token in its legacy `api_secret` slot and emits the actual `key`/`token` query
pair. No OAuth signature is fabricated. A narrow operation port retains the
real SDK's `fetch_json` request and `Board.from_json` model construction. The
pinned model discards `starred` and `idOrganization`; the inherited donor then
invents false/null fallbacks. The port projects required fields from the real
Board and preserves description, starred and organization from the validated
provider payload only when supplied. Explicit null, false and empty strings
remain distinct from omission. Responses over 100 boards fail instead of being
silently truncated. Provider diagnostics and
credential-bearing URLs are omitted from MCP and error logs.

Salesforce takes an existing session token and an explicit instance:

```json
{
  "profile": "salesforce",
  "operations": ["get_record"],
  "config": {
    "timeout": 10,
    "api_key_env": "TOOLKIT_SALESFORCE_SESSION_TOKEN",
    "instance_url": "https://your-instance.my.salesforce.com"
  }
}
```

Its constructor port injects the shared session into the actual `Salesforce`
client. The inherited `get_record` calls the real `SFType.get` and returns the
complete record. Omit `fields` or pass `""`; nonempty field projection enters
the donor's SOQL branch and is intentionally outside this profile. The
response's `Id` must match the selected record, allowing Salesforce's 18-character
form of the same selected 15-character ID. No password login, refresh, query,
write or donor agent instructions are installed.

Twilio and Plivo use selected account IDs with their respective auth tokens:

```json
{
  "profile": "twilio",
  "operations": ["get_call_details"],
  "config": {
    "timeout": 10,
    "api_key_env": "TOOLKIT_TWILIO_AUTH_TOKEN",
    "account_sid": "AC00000000000000000000000000000000"
  }
}
```

```json
{
  "profile": "plivo",
  "operations": ["get_call_details"],
  "config": {
    "timeout": 10,
    "api_key_env": "TOOLKIT_PLIVO_AUTH_TOKEN",
    "auth_id": "MA000000000000000000"
  }
}
```

The all-zero IDs are placeholders, not verified accounts. Twilio's constructor
port injects its public HTTP-client seam and fixes account, origin and Basic
authentication. The pinned Agno body has an SDK incompatibility: it reads
`call.from_`, while Twilio 9.11.2 exposes `CallInstance._from`. An explicit
operation port changes that lookup while retaining the real SDK's
`calls(...).fetch()` and the donor's eight-field result projection. Its
`str(start_time)`/`str(end_time)` convention, including `"None"` for absent
times, remains visible. A present timestamp must match this complete supported
wire form before the actual SDK parser runs: `Wdy, DD Mon YYYY HH:MM:SS +0000`,
or the same form ending in `GMT`. English weekday/month abbreviations are
case-sensitive, the day has two digits, the year four digits, and separators
are single ASCII spaces. The calendar date, clock time and weekday must agree.
The actual SDK parser must preserve that exact instant; years below 0100 are
rejected because the pinned parser silently rewrites them to modern years.
No leading/trailing whitespace, line folding, comments or extra content is
accepted. Other RFC 2822 forms are outside this profile. Empty strings,
malformed dates, missing timezone and non-UTC offsets fail rather than becoming
successful `"None"` or a falsely relabeled UTC time. Absent fields and JSON null retain the
documented `"None"` projection. Plivo retains the inherited operation, actual SDK
request preparation and `Call` model, replacing its `send_request` method.
The duplicate `call_uuid` query must match the selected path ID. Its otherwise
unused SDK sessions are retired with the profile. Neither profile sends
messages, places calls, refreshes tokens or follows regional fallbacks.

Webex retains the real `RoomsAPI.list`/`Room` path through the inherited
operation. The constructor closes the SDK's unused Requests session and
replaces that instance's session with the borrowed transport. Enumeration is
supported only when the first page contains at most 100 rooms and no pagination
`Link` header. Any nonempty pagination header or oversized list is an explicit
failure. An incomplete list is never returned as a successful all-rooms result.
The host must provide an existing token; consent and refresh remain host work.

arXiv is public and takes only a timeout:

```json
{
  "profile": "arxiv",
  "operations": ["search_arxiv_and_return_articles"],
  "config": {"timeout": 10}
}
```

Its constructor port selects the real `arxiv.Client`, Atom parser and `Result`
models without setting up the donor's PDF path. The unused SDK session is
closed and replaced on that instance. One bounded feed page supplies up to
20 articles. Invalid XML, inconsistent feed counts or entries the SDK would
silently skip fail; an explicitly empty feed is valid. The existing donor
formatter retains article title, authors, summary and links. The pinned donor
imports `pypdf`, which is therefore an import-only dependency here. No PDF is
downloaded or parsed, and no download directory is created.

The component's retained notices identify these constructor/client injection
ports and exact donor bytes. Configuration alone does not establish live
provider behavior, account authorization or readiness.

### Reddit, X, YouTube metadata, Oxylabs, Wikipedia and AWS Lambda

These six source profiles await independent review and installed acceptance.
They use the same serialized MCP worker, joined retirement and one owned
Requests session. The actual provider response must be HTTP 200 and at most
1 MiB of decoded bytes before UTF-8/JSON parsing. No HTTP redirect or retry is
followed. Provider failures and malformed consumed fields are errors, not
empty-success fallbacks. The configured timeout applies to each bounded
request; Wikipedia makes at most five requests per operation.

| Profile | Selected operations | Additional explicit configuration |
| --- | --- | --- |
| `reddit` | `get_top_posts`, `get_subreddit_stats`, `get_user_info` | `api_key_env` for an existing OAuth access token; `user_agent` with contact/application identity |
| `x` | `get_user_info`, `search_posts` | `api_key_env` for an existing bearer token; optional `include_post_metrics` boolean, default false |
| `youtube` | `get_youtube_video_data` | Public metadata; only `timeout` |
| `oxylabs` | `search_google`, `get_amazon_product` | `username`; `api_key_env` for that account's password |
| `wikipedia` | `search_wikipedia` | Explicit lowercase `language`, such as `en` or `sv`; no credential |
| `aws-lambda` | `list_functions` | `region`, `access_key_id_env`, `api_key_env` for the secret key, optional `session_token_env` |

For example, a selected AWS profile references credentials without embedding
them in the JSON file:

```json
{
  "profile": "aws-lambda",
  "operations": ["list_functions"],
  "config": {
    "timeout": 10,
    "region": "us-east-1",
    "access_key_id_env": "SELECTED_AWS_ACCESS_KEY_ID",
    "api_key_env": "SELECTED_AWS_SECRET_ACCESS_KEY",
    "session_token_env": "SELECTED_AWS_SESSION_TOKEN"
  }
}
```

Reddit preserves the three actual Agno methods and PRAW's real listing,
objectification, Subreddit, Redditor and Submission models. An explicit SDK
constructor/request port loads only PRAW's packaged defaults and avoids user
config files, environment credentials, auth acquisition, SDK retry/sleep and
update checks. Identifiers are single ASCII names of at most 32 characters;
post limits are 1-25 with the donor's time-filter enum. It reads one response.
A nonterminal page shorter than the requested limit fails rather than chasing
pagination or claiming complete results. The actual SDK's `[deleted]` author
representation becomes the donor string `None`; absent/null author fields
are unsupported response shapes. Subreddit/user response identities must
match the selected request. These operations do not use the donor's unused
allowed-subreddit field as an authorization policy.

X preserves real Tweepy user/tweet models and parameter preparation. Its
request port uses only the selected bearer token and omits the SDK boundary
that logs complete authorization headers. The selected Client's otherwise
unused Requests session closes immediately. Tweepy import also allocates an
idle global legacy API session; this component never borrows, mutates or
performs I/O through that SDK global. User names are 1-15 ASCII characters;
search queries are 1-512 characters and requested page sizes 10-100. Search
adds the author expansion required by the donor's author-link formatter.
Absent expanded authors, malformed metrics/timestamps or inconsistent counts
fail before the SDK can fabricate defaults. Consumed user, tweet and author
IDs must be canonical positive decimal strings without leading zeros, so
Tweepy integer conversion cannot collapse distinct wire identities. The error-only wrapper preserves
the actual donor's success formatter but normalizes its multi-key error object.
The inherited success formatter returns only query/count/posts and discards
provider metadata, including pagination tokens. No other page is followed. OAuth acquisition,
OAuth1 user-context API calls, token refresh and writes are excluded.

YouTube is an explicit source port of Agno's metadata operation and URL parser.
It accepts exactly HTTPS `youtube.com`/`www.youtube.com` watch (`?v=ID`),
embed or v paths, and `youtu.be/ID`, with an 11-character ASCII video ID.
Extra query fields, credentials, ports, fragments, encoded paths and foreign
hosts fail before provider I/O. Only the fixed YouTube oEmbed endpoint is
called. The original ten-field result projection is retained with consumed
field/type checks. Captions, timestamps, downloads, browser access and the
unused transcript SDK are outside this profile.

Oxylabs executes the original Agno read bodies through the actual Google and
Amazon SDK source helpers and Response models. The two instance API boundaries
borrow the existing bounded transport. Credentials bind the selected username
and password with Basic auth only at the fixed realtime API. Google returns at
most 50 organic results; the accepted provider job contains at most one result,
with an actual successful result status. If job metadata is supplied, its status
must be `done`, the pinned SDK's terminal success status; absent metadata is
allowed. Product ASINs are exactly ten ASCII
alphanumeric characters. Domains are the published finite schema choices.
The Google validator follows the donor's actual branch selection: an empty
parsed organic list may select raw-content results, which receive the same
field and 50-result checks. Unused raw content is not rejected when a nonempty
parsed list is selected. Malformed envelopes, parser errors and failed source
jobs cannot become empty results or fabricated products. Valid explicit empty result lists remain empty.
The inherited error helper is narrowed to the existing MCP error convention.

Wikipedia is an attributable port of Agno's Document projection and
wikipedia 1.4.0's page/summary/disambiguation algorithm, using actual Agno
Document and BeautifulSoup. It fixes HTTPS and the selected language, avoids
the original global cache/locale/HTTP client, and disables auto-suggestion and
knowledge ingestion. Exact titles are 1-256 characters without control or
multi-title separators. At most three logical API title redirects are followed;
at most 50 disambiguation options are returned, with explicit failure if
incomplete. Secondary summaries and revisions must retain the canonical title
and page-map ID from initial resolution; an explicit numeric pageid must agree
with that map key. Initial title normalization remains supported. Explicit
continuation markers, empty/missing extracts and malformed page/revision
payloads fail. The MIT and BSD source notices for the attributed helper ports are retained.

Successful summaries retain the requested query in `name` and add
`source_url`, `canonical_title`, `page_id`, `language` and `retrieved_at` in
the existing Document `meta_data`. The URL and page identity come from the same
API response as the extract. The URL must identify that title on the selected
Wikipedia language site. `retrieved_at` is the local UTC observation time after
the response, not the article's update time or proof that its cached content is
current. No revision ID or revision permalink is claimed: the TextExtracts API
does not establish that association. Missing or inconsistent source metadata
fails through the existing error result; the connector never invents a URL.
Disambiguation results retain their existing option-list format.

AWS Lambda executes the original list formatter and real botocore service
serialization, SigV4 canonicalization/HMAC and protocol parsing. Explicit
constructor configuration prevents shared credential files, ambient profiles,
endpoint overrides, metadata credentials and token refresh. A narrow endpoint
port borrows the same Requests owner; a narrow signer entry port omits logging
canonical credential/session-token headers while retaining the actual SDK's
signing methods. Tests compare its final Authorization header with the original
botocore signer. `MaxItems=50` is explicit; a continuation marker or an
oversized/malformed list fails, so the all-functions wording cannot conceal a
partial enumeration. The selected commercial region format excludes GovCloud,
China, FIPS/custom endpoints and aliases. Function invocation is not exposed.

The successor [dependency audit](../../docs/verification/20261003-agno-next-batch/dependency-audit.json)
records each new/replaced wheel, installed bytes/notices and each profile's
SDK closure. OAuthlib verification covers real Requests-OAuthlib OAuth1
serialization and X bearer-only reads, not OAuth authorization-server flows.
No native dependency or core Meerkat/MobKit version changes.

### PostgreSQL metadata over a selected local Unix socket

```json
{
  "profile": "postgres",
  "operations": ["show_tables", "describe_table"],
  "config": {
    "socket_directory": "/run/postgresql",
    "port": 5432,
    "database": "selected_database",
    "user": "metadata_reader",
    "password_env": "SELECTED_POSTGRES_PASSWORD",
    "schema": "selected_schema",
    "timeout": 10,
    "max_rows": 100
  }
}
```

All configuration fields above are required. `socket_directory` is one absolute
Unix socket directory, at most 80 UTF-8 bytes, without control characters or
commas (libpq multi-host syntax). Database, user and schema are exact nonempty
identifiers of at most 63 UTF-8 bytes, without NUL. Port is 1-65535, timeout is
1-60 seconds and `max_rows` is 1-1000. The selected password reference is resolved
once at profile construction; its nonempty value may contain Unicode or spaces,
must fit 4096 UTF-8 bytes and must not contain NUL. Password reference names must
not start with `PG`, because this profile refuses every `PG*` environment variable
at construction and before opening a connection. It never clears process globals.
The host must not concurrently mutate libpq environment state during connection.

This profile supports only an explicitly selected local Unix socket with SCRAM
SHA-256 password authentication. A trust, peer or other authentication downgrade
fails. It sets a null password file (`/dev/null`), disables SSL/GSS negotiation and
never selects a TCP host, connection URI, service file or credential fallback.
It does not support or claim remote PostgreSQL/TLS identity verification. Use a
separately provisioned database principal with the intended metadata visibility;
there is no role creation, credential acquisition or schema provisioning here.

The real, unchanged Agno `PostgresTools.show_tables` and `describe_table` bodies
construct their original bound-parameter metadata statements. A declared lazy
connection and `_execute_query` helper port uses the actual psycopg connection
and server cursor. Description and profile construction perform no database I/O.
One profile owns one connection, and its selected database principal and schema
remain fixed. A profile has one active invocation owner: attach at most one MCP
server to it, and serialize any direct adapter calls. The server uses the shared
MCP lock; concurrent raw first calls or sharing one profile between servers is
unsupported. Separate profiles may run concurrently. Close a profile only after
all its calls and workers have joined. The helper admits only those two original statements, uses
`search_path=pg_catalog` and read-only transactions, then always rolls back after
reading or failure. It exposes no arbitrary SQL, data export, inspection or write
operation. Database permissions still govern which metadata is visible; an empty
result is not proof that an inaccessible table does not exist.

The server cursor transfers at most `max_rows + 1` rows. An extra row is a failure,
never a truncated claim to have listed all tables or columns. These fixed metadata
fields are checked for the expected column names and string shape, with 256 UTF-8
bytes per cell. CSV output uses LF record separators and proper quoting for commas, quotes,
carriage returns and newlines on the qualified Python3.11.15 interpreter;
this deliberately corrects the donor's unescaped joining. Empty results retain
the donor's explicit `Query returned no results` message. The result is bounded to
1 MiB. This is a row/materialized-output bound on standard PostgreSQL metadata,
not a claim to stream arbitrary database values or defeat a malicious server's
unbounded protocol field before libpq decodes it.

The selected timeout configures psycopg connection establishment (libpq's minimum
connection timeout is two seconds), statement timeout, lock timeout and idle
transaction timeout. These are per-stage limits, not a hard overall wall-clock
supervisor. Request cancellation uses the existing MCP joined-worker contract:
cleanup waits for the actual database operation to retire, normally on its server
timeout. The profile does not add a second cancellation owner or asynchronously
close an active connection. A nonresponsive local server is not covered by a hard
client-side query deadline. Explicit driver cancellation, rollback, recovery and
close are exercised on the disposable fixture; direct connection cancellation is
not exposed as another MCP tool.

Database failures are raised after rollback and projected through the shared
adapter error envelope. The helper deliberately removes the donor's logging and
success-shaped strings containing raw SQL, account names or database diagnostics.
A failed rollback closes the connection; profile close is idempotent and prevents
reopening. The source proof uses a fresh, private PostgreSQL 16.2 fixture with two
real restricted principals and no TCP listener. It does not qualify an existing
user database, remote account, native host or deployed service.

## Provider setup and permissions

The following static setup guidance was checked against provider documentation on 2026-10-03. It is also available from `config-help --profile <exact-id>` and the setup workbench. Use the permissions for the operations actually selected. The host owns registration, protected authentication, credential storage and renewal. These instructions do not establish live account access or native attachment. Public profiles and local PostgreSQL retain their earlier prerequisites.

### aws-lambda

Reviewed 2026-10-03. Have the AWS account/security owner provide an approved explicit credential set. Prefer short-lived credentials acquired by the host's existing role or federation workflow, including their matching session token. If a long-lived IAM-user key is approved instead, use IAM, Users, the selected user, Security credentials, Create access key; save the secret once through host custody and never use a root key. Configure references for the access key ID and secret, plus session_token_env whenever the supplied credentials are temporary. The connector does not run STS, discover a role, read shared profiles or refresh credentials.

list_functions requires lambda:ListFunctions. This list action does not support a function ARN resource restriction, so the IAM allow uses Resource='*'; account/region controls and applicable policy conditions can still constrain access. No lambda:InvokeFunction, lambda:GetFunction, IAM PassRole or write grant is required. Identity policies, permissions boundaries, SCPs and session policies can still deny the read. Select a supported commercial AWS region explicitly; GovCloud/China partitions and arbitrary endpoint overrides are not supplied by this profile. The connector rejects a response requiring a further page, so this is not an unbounded account inventory.

Account-specific checks: The intended AWS principal/account, effective allow and deny policies, commercial region, matching session token and expiration remain account-owner facts.

References: [Provider documentation 1](https://docs.aws.amazon.com/lambda/latest/api/API_ListFunctions.html), [Provider documentation 2](https://docs.aws.amazon.com/service-authorization/latest/reference/list_lambda.html), [Provider documentation 3](https://docs.aws.amazon.com/IAM/latest/UserGuide/id_credentials_access-keys.html), [Provider documentation 4](https://docs.aws.amazon.com/IAM/latest/UserGuide/access-keys-admin-managed.html), [Provider documentation 5](https://docs.aws.amazon.com/IAM/latest/UserGuide/securing_access-keys.html).

### bitbucket

Reviewed 2026-10-03. In the intended Atlassian account, open Account settings, Security, Create and manage API tokens, then Create API token with scopes. Choose Bitbucket, a name and expiry, and only the scopes needed below. Keep the one-time token in the host credential store. Pair it with that account's email for Basic authentication; this route does not use an account password, legacy app password or workspace access-token Bearer authentication.

For list_repositories or get_repository_details, select read:repository:bitbucket. For list_all_pull_requests or get_pull_request_details, select read:pullrequest:bitbucket; add repository read when repository operations are also selected. API-token scopes do not implicitly grant one another. The PR read scope itself also permits commenting according to Atlassian, although this connector only reads. The account must already see the chosen workspace/repository; a configured repo_slug does not grant access. This route returns bounded first pages, not a complete workspace inventory.

Account-specific checks: The selected account's workspace/repository membership, organizational token restrictions, token expiry and actual selected-operation scopes remain account-owner facts.

References: [Provider documentation 1](https://support.atlassian.com/bitbucket-cloud/docs/create-an-api-token/), [Provider documentation 2](https://support.atlassian.com/bitbucket-cloud/docs/api-token-permissions/), [Provider documentation 3](https://developer.atlassian.com/cloud/bitbucket/rest/).

### bright-data

Have a Bright Data account administrator open Account settings, Users and API keys, then Add API key. Select the intended user, API-usage permission and expiry; retain the displayed key in the host credential store. In Web Access, Create API creates named Web Unlocker or SERP zones. Complete any account verification requested there.

scrape_as_markdown needs the selected web_unlocker_zone; search_engine needs the selected serp_zone. Both use the regular REST API key with /request. The User permission permits API usage without granting billing or product administration. Both zone names are mandatory configuration fields, but only selected operations require their corresponding working product. A proxy password or separately issued Agent API key is not proven interchangeable with this REST credential.

Account-specific checks: The account administrator must verify key permission and expiry, selected zone ownership, product activation, target restrictions and current allowance. The documentation describes differing free-tier verification cases; the account's actual enrollment requirements remain unverified.

References: [Provider documentation 1](https://docs.brightdata.com/api-reference/authentication), [Provider documentation 2](https://docs.brightdata.com/products/web-unlocker/quickstart), [Provider documentation 3](https://docs.brightdata.com/products/serp-api/quickstart).

### calcom

Reviewed 2026-10-03. On the intended Cal.com account, open Settings, Security and create/manage an API key, then store it through the host credential owner. Select the actual event type ID and IANA timezone. This route uses Bearer API-key authentication; it does not register or refresh OAuth clients. Cal.com recommends OAuth for integrations and requires a verified OAuth client for an App Store partner. Its deprecated Platform offering is not a new-customer prerequisite for this account-key route.

get_available_slots reads availability for event_type_id. get_upcoming_bookings reads the selected account's upcoming bookings and can optionally filter attendeeEmail; event_type_id does not narrow that operation. The API key carries broad account privileges, not a two-operation read-only grant. Calendar connections, schedules and event-type availability must exist in the account. The retained transport uses /v2/slots/available and cal-api-version 2024-08-13; current booking docs require 2026-05-01 and describe a different pagination contract. These instructions do not establish continuing live compatibility of the older selected route, and changing credentials cannot repair an API-version mismatch.

Account-specific checks: The account's API-key availability, plan/policy, event-type ownership, calendar connections and actual availability/bookings access remain account-owner facts.

References: [Provider documentation 1](https://cal.com/docs/api-reference/v2/introduction), [Provider documentation 2](https://cal.com/docs/api-reference/v2/bookings/get-all-bookings), [Provider documentation 3](https://cal.com/docs/api-reference/v2/oauth).

### clickup

Reviewed 2026-10-03. For this personal-token route, open the intended ClickUp user's avatar, Settings, Apps and generate the personal API token. Store it through the host credential owner. The connector sends it directly in Authorization. ClickUp's OAuth app registration is a separate route for applications used by other people, requiring a Workspace owner or admin to register the app; this profile does not register an OAuth client or run that flow.

get_task reads the specified task and list_spaces reads spaces under workspace_id (called team_id in the API). A personal token has the same access as its user rather than a selectable read-only scope for these methods. Choose a user permitted to see the task/Workspace and private spaces; workspace_id is a resource selector, not a token restriction. This connector has no task-write operation, but the underlying personal token can have broader authority.

Account-specific checks: The user's membership, guest/private-resource access, personal-token availability under Workspace policy and chosen task/Workspace remain account-owner facts.

References: [Provider documentation 1](https://developer.clickup.com/docs/authentication), [Provider documentation 2](https://developer.clickup.com/reference/gettask), [Provider documentation 3](https://developer.clickup.com/reference/getspaces).

### discord

Reviewed 2026-10-03. Create or select an application in the Discord Developer Portal, use its Bot page to obtain/reset the bot token, and place that token in host custody. Install the bot in the intended server using a Guild Install link with the bot scope; an installing member needs Manage Guild. Request only required bot permissions, not Administrator. This is a REST connector, not an agent frontend, user self-bot, slash-command server or Gateway listener.

get_channel_info and list_channels require the bot to have access to the addressed channel/guild; listing guild channels does not include threads. For get_channel_messages in a guild channel, grant View Channel and Read Message History through the bot role and channel overrides; voice-channel message access also requires Connect. To receive message content, embeds, attachments and components where Discord's exceptions do not apply, enable Message Content intent and obtain approval when required for a verified application. Without it the API can return empty content fields. This route does not need Send Messages or Manage Messages, and a successful metadata read does not prove history/content access.

Account-specific checks: The actual server installation, role/channel overrides, private-thread access if addressed, and Message Content approval/configuration remain application/server-owner facts.

References: [Provider documentation 1](https://docs.discord.com/developers/quick-start/getting-started), [Provider documentation 2](https://docs.discord.com/developers/resources/message), [Provider documentation 3](https://docs.discord.com/developers/resources/guild), [Provider documentation 4](https://docs.discord.com/developers/events/gateway#message-content-intent).

### exa

Create or sign in to the selected Exa account, open its dashboard and create an API key. Have the host credential owner bind api_key_env. This profile uses the direct API key even where Exa offers a separate keyless hosted MCP route.

search_exa calls /search and can request result text through the same search call. Confirm Search access and credits for that request. This selected adapter does not expose separate Contents, Agent, Websets, monitor or key-management operations; it requests no OAuth scope or callback.

Account-specific checks: The key's team/account, Search allowance and remaining credits are not verified. Provider key authority may be broader than the operations exposed here.

References: [Provider documentation 1](https://exa.ai/docs/get-started/quickstart), [Provider documentation 2](https://exa.ai/docs/reference/search).

### financial-datasets

Sign up or sign in at financialdatasets.ai and generate an API key in the account dashboard. Have the host credential owner supply that key through api_key_env. The selected transport sends X-API-KEY, not an OAuth access token.

get_income_statements reads /financials/income-statements; get_balance_sheets reads /financials/balance-sheets. Confirm that the selected account permits the requested dataset, tickers, reporting periods and volume. These reads do not need an OAuth application or redirect URL.

Account-specific checks: The account's active key, dataset coverage, paid entitlements and remaining limits require account-owner verification.

References: [Provider documentation 1](https://docs.financialdatasets.ai/quickstart), [Provider documentation 2](https://docs.financialdatasets.ai/api/financials/income-statements), [Provider documentation 3](https://docs.financialdatasets.ai/api/financials/balance-sheets).

### firecrawl

Create or sign in to a Firecrawl cloud account and obtain an API key from its dashboard. Store it with the host credential owner for api_key_env. This pinned profile requires a key even where current Firecrawl offers keyless requests; its origin is the hosted api.firecrawl.dev service.

scrape_website uses /v2/scrape and map_website uses /v2/map. Check the selected team's allowance for each enabled endpoint. These operations submit URLs to Firecrawl; they do not supply a website login session. No OAuth client or redirect URL is requested by this API-key route, and a self-hosted service is not selected by this profile.

Account-specific checks: The key's team, enabled endpoint access, current credits/limits and authority to submit the target URLs remain unverified.

References: [Provider documentation 1](https://docs.firecrawl.dev/introduction), [Provider documentation 2](https://docs.firecrawl.dev/api-reference/endpoint/scrape), [Provider documentation 3](https://docs.firecrawl.dev/api-reference/endpoint/map).

### gitlab

Reviewed 2026-10-03. On the selected GitLab host, open the user's profile settings, Access, Personal access tokens, and create a legacy personal access token with a name and expiry. Current GitLab distinguishes legacy from fine-grained tokens. Put the PAT in the host credential store for PRIVATE-TOKEN authentication. An enterprise group or instance administrator can disable PAT use or limit lifetime. Supply an HTTPS origin, without a URL subpath, for self-managed GitLab.

The selected list_projects, get_project, list_merge_requests, get_merge_request and list_issues calls are covered by read_api plus the user's resource permissions. read_repository alone does not cover these general API reads. Merge-request and issue methods first look up their project, so a replacement fine-grained grant would also need that project read; this profile does not derive a fine-grained permission set. Do not request api write access merely to use these reads. Private projects and confidential issues remain subject to GitLab membership and visibility rules.

Account-specific checks: The host version, PAT policy/expiry, selected user's project roles and access to confidential issues must be verified for the intended account.

References: [Provider documentation 1](https://docs.gitlab.com/user/profile/personal_access_tokens/), [Provider documentation 2](https://docs.gitlab.com/security/tokens/access_token_scopes/), [Provider documentation 3](https://docs.gitlab.com/user/permissions/).

### jina

Open Jina Reader's API Key & Billing page, obtain the selected Jina API key and retain it with the host credential owner. Signing in allows management of the key. Supply its reference as api_key_env; this profile does not implement the separate hosted Jina MCP setup.

read_url sends the selected URL to r.jina.ai; search_query sends the query to s.jina.ai. Both selected operations use the key even though Jina also offers keyless Reader access. Search requires a key and consumes tokens. One Jina key can authorize other Jina products too; selecting these operations narrows this adapter, not the provider credential. No destination-site login or cookie grant is supplied.

Account-specific checks: The selected key's balance, tier and live limits, and permission to send the chosen URLs or queries to Jina, remain host/account facts.

References: [Provider documentation 1](https://jina.ai/reader/).

### linkup

Create or sign in to a Linkup account using its Get Started portal and obtain the account API key. Supply that key through the host's api_key_env reference. This selected adapter uses the API-key route, not the separate x402 payment workflow.

web_search_with_linkup calls /v1/search with the explicitly configured standard or deep depth and searchResults or sourcedAnswer output. Confirm Search access and allowance for the selected depth. No OAuth application, callback, wallet or payment signature is required by this key-based route.

Account-specific checks: The key's account, Search entitlement, selected-depth allowance and remaining usage are not established by help or config validation.

References: [Provider documentation 1](https://docs.linkup.so/pages/documentation/get-started/quickstart), [Provider documentation 2](https://www.linkup.so/), [Provider documentation 3](https://www.linkup.so/blog/linkup-web-search-for-ai-agents-no-human-account-needed).

### openrouteservice

Use the HeiGIT account portal at account.heigit.org to register, complete account verification and obtain the selected API key. Store it with the host credential owner and refer to it through api_key_env. Existing OpenRouteService keys are documented to work on the replacement service; this does not verify any particular account.

geocode_location needs geocoding access. get_directions performs two geocoding requests before its directions request, so it needs both services even when geocode_location is not separately exposed. This transport uses the fixed https://api.heigit.org origin: GET /pelias/v1/search and POST /openrouteservice/v2/directions/{profile}. Compared with the earlier api.openrouteservice.org artifact, this changes the egress origin. The host must permit the replacement origin under its existing network policy before use. No legacy-host fallback or arbitrary-host override is supplied.

Account-specific checks: The selected key's validity, account entitlement, remaining quota and live service availability are unverified. Host permission for the replacement origin is required; changing a credential reference does not establish that permission.

References: [Provider documentation 1](https://account.heigit.org/), [Provider documentation 2](https://ask.openrouteservice.org/t/deprecating-api-openrouteservice-org-in-favour-of-api-heigit-org/7912), [Provider documentation 3](https://ask.openrouteservice.org/t/reducing-the-quota-of-deprecated-api-api-openrouteservice-org/8013).

### openweather

Create an OpenWeather account, complete email verification and retrieve or generate a key in the account's API keys page. New keys can take up to two hours after registration to activate. Have the host credential owner bind api_key_env after activation; no OAuth client or callback is involved.

Match the subscription to each selected endpoint: get_current_weather uses /data/2.5/weather, get_forecast uses /data/2.5/forecast, get_air_pollution uses /data/2.5/air_pollution, and geocode_location uses /geo/1.0/direct. Forecast here means the 5-day endpoint, not One Call or historical weather. Usage limits apply across the account's keys; another key is not another quota.

Account-specific checks: Actual key activation, selected product access and remaining account allowance are unverified; a newly generated key alone is not ready access.

References: [Provider documentation 1](https://openweathermap.org/appid), [Provider documentation 2](https://openweathermap.org/faq), [Provider documentation 3](https://openweathermap.org/api/current), [Provider documentation 4](https://openweathermap.org/forecast5), [Provider documentation 5](https://openweathermap.org/api/air-pollution), [Provider documentation 6](https://openweathermap.org/api/geocoding-api).

### oxylabs

Reviewed 2026-10-03. Create/select an Oxylabs account, activate Web Scraper API through its plan or trial, and create/select an API user in the dashboard. The API user's username and password authenticate the realtime endpoint with Basic authentication. Configure username and store that API-user password through api_key_env; despite the field name, it is not a bearer API key. Use the dedicated API-user credential, not an assumed website-login password.

search_google submits source=google_search and get_amazon_product submits source=amazon_product, both with parse=true to the fixed realtime Web Scraper API endpoint. The account must have the applicable target/product entitlement and sufficient quota or credit. These are paid/usage-metered scrape requests even though the connector retrieves data. This credential does not provide a per-selected-operation OAuth scope and is not interchangeable with a proxy-zone username/password or another Oxylabs product's key.

Account-specific checks: The API user's active state, selected product/target entitlement, account quota/credit and current usage restrictions remain account-owner facts.

References: [Provider documentation 1](https://developers.oxylabs.io/help-center/getting-started/start-using-web-scraper-api), [Provider documentation 2](https://developers.oxylabs.io/help-center/most-popular-questions/getting-started-web-scraper-api), [Provider documentation 3](https://developers.oxylabs.io/help-center/dashboard/web-scraper-api-101-navigating-the-dashboard).

### plivo

Reviewed 2026-10-03. Select the intended Plivo account in Console and obtain its Auth ID and Auth Token from the dashboard through an authorized operator. For isolation, use an existing subaccount's own credentials; each subaccount has a separate Auth ID and token. Store the token through host custody and configure the matching auth_id. This is HTTP Basic authentication, not a generic bearer API key or an OAuth login.

get_call_details reads an existing call detail record by call UUID under that account. It does not call the separate live-call endpoint, create calls, send messages or provision numbers. The Auth Token is an account credential, not a selectable read-only call scope. The requested UUID must belong to the chosen account/subaccount; outbound geo-permission and caller-verification setup should not be imposed merely to read an existing record.

Account-specific checks: The intended account/subaccount, authorized credential operator, credential validity and record ownership/retention remain account-owner facts.

References: [Provider documentation 1](https://www.plivo.com/docs/voice/api/overview), [Provider documentation 2](https://www.plivo.com/docs/voice/api/calls), [Provider documentation 3](https://docs.plivo.com/docs/account/concepts/subaccounts).

### reddit

Request and obtain explicit Reddit Data API approval before use. The current Responsible Builder Policy directs developers to Devvit; if the intended external application is not supported there, use its linked support request and describe the actual use case. Commercial use requires explicit written approval. Only after Reddit authorizes the external client should the host register/configure the permitted OAuth app at reddit.com/prefs/apps: web for a confidential hosted client, installed for a public client, or script for a controlled personal-account client. Those legacy app types are technical choices, not current eligibility guarantees. The host supplies the resulting bearer through api_key_env and a truthful descriptive user_agent; it does not send client secret or user password to this adapter.

get_top_posts reads /r/{subreddit}/top; get_subreddit_stats reads /r/{subreddit}/about; get_user_info reads /user/{username}/about. Reddit's endpoint reference assigns read to all three, so request read for any selected subset. get_user_info is public-user information, not /api/v1/me and does not need identity. No history, submit, vote, moderator or private-message scope is needed. Scope does not override private/quarantined community restrictions, account restrictions or the approved use case. Use a User-Agent of the documented platform:app-id:version (by /u/contact) form. OAuth acquisition/refresh stays with the host, and no scraping or alternate endpoint fallback is supplied.

Account-specific checks: Reddit's explicit approval for this external client and intended use, permitted app type, commercial status and assigned quota remain unverified. Token validity/read grant, target subreddit or user visibility and compliance with the approved data-retention conditions require the account/application owner to establish.

References: [Provider documentation 1](https://support.reddithelp.com/hc/en-us/articles/42728983564564-Responsible-Builder-Policy), [Provider documentation 2](https://support.reddithelp.com/hc/en-us/articles/16160319875092-Reddit-Data-API-Wiki), [Provider documentation 3](https://github.com/reddit-archive/reddit/wiki/OAuth2), [Provider documentation 4](https://www.reddit.com/dev/api/).

### salesforce

Have the Salesforce administrator create an External Client App from Setup > App Manager > New External Client App (or External Client App Manager), enable OAuth and select a flow appropriate to the host. Register the host-owned callback only when the selected flow needs one; do not copy a Salesforce CLI or Mobile SDK example callback. New Connected App creation is restricted from Spring '26; existing Connected Apps may continue, and a new exception requires Salesforce Support. For admin-preauthorized use, assign the app to the intended profile or permission set. The host acquires an API access token for the chosen org and supplies it through api_key_env with that org's instance_url. No password login, token acquisition or refresh is performed by this adapter.

get_record performs one REST sObject Rows GET by object name and record ID. Enable OAuth scope api (Manage user data via APIs) and give the token's user API Enabled plus Read on the selected object, visibility of the needed fields and access to the particular record through sharing. The api scope itself is not a read-only provider grant; object/field/record permissions constrain it. Do not grant Modify All Data, object write permissions, web or full solely for this read. With no fields query, Salesforce returns accessible record fields, not hidden fields. refresh_token/offline_access is a host renewal decision, not an operation requirement. The org edition must support API access.

Account-specific checks: Org API entitlement, the user's license/API Enabled permission, External Client App policy/approval and token validity are not verified. The selected instance_url, API version, object/record identity, field visibility, sharing and any org IP/session restrictions require the org owner to confirm.

References: [Provider documentation 1](https://developer.salesforce.com/docs/platform/api-rest/guide/intro-oauth-and-connected-apps.html), [Provider documentation 2](https://developer.salesforce.com/docs/platform/mobile-sdk/guide/eca-create.html), [Provider documentation 3](https://developer.salesforce.com/docs/platform/sfdx-dev/guide/sfdx-dev-auth-eca.html), [Provider documentation 4](https://developer.salesforce.com/docs/platform/salesforce-developer-environment-tipsheet/guide/salesforce-developer-environment-overview.html), [Provider documentation 5](https://developer.salesforce.com/docs/platform/api/guide/calls.html), [Provider documentation 6](https://developer.salesforce.com/docs/platform/api-rest/guide/dome-get-field-values.html), [Provider documentation 7](https://developer.salesforce.com/docs/data/data-cloud-dev/guide/dc-quick-start.html).

### shopify

Reviewed 2026-10-03. Create a new app through Shopify's Dev Dashboard, configure its Admin API scopes, release the app version and install it on the intended store using an authorized store operator. For an app and store owned by the same organization, the host can use the documented client-credentials grant to obtain the Admin access token; that token lasts 24 hours and must be replaced by its owner. For other merchants, use the appropriate Shopify app authorization flow. New legacy admin-created custom apps are unavailable from January 1, 2026, although existing legacy apps remain a separate case. Supply the resulting token, never the app's client secret, to api_key_env.

get_products needs read_products, including the selected variants and inventoryQuantity fields; this query does not read inventory levels by location or request inventory writes. get_shop_info reads basic shop/contact/address/plan fields via an authenticated Admin API token; do not invent a read_shop scope or require product access when only that operation is selected. The token must belong to shop_name, and api_version must support the exact selected GraphQL fields. The retained query uses the currently deprecated billingAddress field. A token generated for another shop or the Storefront API is not interchangeable.

Account-specific checks: The organization's app/store ownership, staff installation/development permissions, approved scopes, selected supported API version and current token expiry remain shop/application-owner facts.

References: [Provider documentation 1](https://shopify.dev/docs/apps/build/authentication-authorization/client-credentials-grant), [Provider documentation 2](https://help.shopify.com/en/manual/apps/about-apps), [Provider documentation 3](https://help.shopify.com/en/manual/apps/managing-apps), [Provider documentation 4](https://shopify.dev/docs/api/admin-graphql/latest/objects/ProductVariant), [Provider documentation 5](https://shopify.dev/docs/api/admin-graphql/latest/objects/Shop).

### spotify

Create a Spotify developer app in the Developer Dashboard and configure the redirect URI actually owned by the host's authorization flow. For user data, the host uses Authorization Code with a safely stored client secret, or Authorization Code with PKCE for a public client; this adapter only receives the resulting access token through api_key_env. Current Development Mode requires the app owner's active Premium subscription, limits newly created access to one client ID and five authorized users, and requires users in Dashboard Settings > Users Management. Existing excess clients/users are grandfathered; Extended Quota Mode has separate approval. Do not infer usable access from successful consent alone.

Select scopes by operation: get_my_top_tracks requires user-top-read. get_user_playlists calls /me/playlists for the token's user, not an arbitrary user ID; playlist-read-private covers private playlists and playlist-read-collaborative includes collaborative playlists. get_current_user calls /me: user-read-email governs email and user-read-private governs private subscription/country fields where those fields are available; these are not blanket scopes for all four operations. Development Mode no longer returns email, country, product or explicit_content, so requesting these scopes does not restore those fields. search_tracks calls catalog /search; its endpoint reference lists no user scope, while the general scope index also associates Search with user-read-private. A search-only app-token route therefore remains a documented-source discrepancy, not verified account access. Development Mode search now permits at most 10 results per call; this MCP profile enforces that bound before dispatch. Playlist counts accept current items and legacy tracks fields; unavailable counts are null. No playback or write scope is required by these selected reads.

Account-specific checks: The selected app's quota mode, owner Premium status, authorized-user list, granted token scopes and token expiry have not been observed. The application owner must establish that the intended use of Spotify content is permitted, including Spotify's restrictions on content use with AI; this help does not establish approval or an exception.

References: [Provider documentation 1](https://developer.spotify.com/documentation/web-api/concepts/apps), [Provider documentation 2](https://developer.spotify.com/documentation/web-api/concepts/authorization), [Provider documentation 3](https://developer.spotify.com/documentation/web-api/concepts/quota-modes), [Provider documentation 4](https://developer.spotify.com/documentation/web-api/tutorials/february-2026-migration-guide), [Provider documentation 5](https://developer.spotify.com/documentation/web-api/reference/get-current-users-profile), [Provider documentation 6](https://developer.spotify.com/documentation/web-api/reference/get-users-top-artists-and-tracks), [Provider documentation 7](https://developer.spotify.com/documentation/web-api/reference/get-a-list-of-current-users-playlists), [Provider documentation 8](https://developer.spotify.com/documentation/web-api/reference/search), [Provider documentation 9](https://developer.spotify.com/documentation/web-api/concepts/scopes).

### tavily

Create or sign in to the selected Tavily account at app.tavily.com and obtain its dashboard API key. Pass it to the host credential owner for api_key_env; do not paste it into configuration or chat. This pinned profile requires a key even where newer Tavily products offer keyless access.

web_search_using_tavily uses the account's Search API allowance. Confirm available credits and request limits for the fixed basic search depth. This key-based route does not request OAuth scopes or register a callback.

Account-specific checks: The selected Tavily key's account, active status, remaining Search credits and applicable limits are not established by this help.

References: [Provider documentation 1](https://docs.tavily.com/documentation/quickstart), [Provider documentation 2](https://docs.tavily.com/documentation/api-reference/endpoint/search).

### todoist

For your own account, obtain the personal API token under Todoist Settings, Integrations, Developer. For other users, the application operator registers an app and redirect URL in Todoist's App Management Console and runs user OAuth consent outside this adapter. Supply only the resulting API/access token through api_key_env; retain any client secret and refresh token with the host credential owner.

get_task reads one active task at /api/v1/tasks/{task_id}. A user OAuth grant can use data:read; this operation does not require data:read_write or task:add. A personal token is broader than this one-operation adapter. Newly created OAuth applications issue expiring tokens and refresh tokens, so the host must own renewal; the adapter only consumes the supplied access token.

Account-specific checks: Selected-user task/project visibility, actual granted scope and token validity are unverified. If OAuth is used, application approval, callback handling and renewal must be supplied by the host; no managed Todoist application is included.

References: [Provider documentation 1](https://developer.todoist.com/), [Provider documentation 2](https://developer.todoist.com/api/v1/#tag/Authorization), [Provider documentation 3](https://developer.todoist.com/api/v1/#tag/Tasks/operation/get_task_api_v1_tasks__task_id__get).

### trello

Reviewed 2026-10-03. Create or select a Power-Up in Trello's Power-Up admin portal and generate its API key on the API Key tab. Have the intended Trello user authorize that key with scope=read and an explicit expiry through Trello's documented /1/authorize flow. The host credential owner stores the resulting user token separately from the API key. api_key_env and token_env name that pair; this route does not use an OAuth app secret or construct OAuth signatures.

list_boards reads /1/members/me/boards and needs read, not write or account (email) scope. Results are boards visible to the authorizing user, including their applicable Workspace/board membership. The filter parameter chooses returned boards; it is not a provider authorization boundary. read grants broader Trello read access than this one operation, while the adapter exposes no board mutations. The selected API key and user token must belong to the same authorization.

Account-specific checks: The intended user's Workspace/board visibility, organization Power-Up restrictions and chosen token expiry/revocation state remain account-owner facts.

References: [Provider documentation 1](https://developer.atlassian.com/cloud/trello/guides/rest-api/authorization/), [Provider documentation 2](https://developer.atlassian.com/cloud/trello/rest/api-group-members/).

### twilio

Reviewed 2026-10-03. Select the intended Twilio account or subaccount in Console and obtain its Account SID and Auth Token from Account Info through an authorized operator. Put only the Auth Token in the host credential store and use the matching account_sid in configuration. Twilio recommends API keys for production, but an API-key SID/secret is a different credential pair and is not implemented by this selected Account-SID/Auth-Token route. This component does not create a Twilio account or perform credential rotation.

get_call_details only fetches a known Call SID under the configured account. The account Auth Token has broad account authority rather than a read-only Call permission, so use an appropriately isolated account/subaccount when needed. No number purchase, messaging permission, outbound caller verification or call-creation grant is required by this read operation itself. The fixed api.twilio.com endpoint and explicit account pair do not implement a regional/edge credential selector or arbitrary parent-account/subaccount credential mix.

Account-specific checks: The chosen account/subaccount, operator permission to obtain its token, credential validity and whether the requested Call SID belongs to it remain account-owner facts.

References: [Provider documentation 1](https://help.twilio.com/articles/223136027-Auth-Tokens-and-How-to-Change-Them), [Provider documentation 2](https://www.twilio.com/docs/usage/requests-to-twilio), [Provider documentation 3](https://www.twilio.com/docs/voice/api/call-resource).

### unsplash

Register a developer account with Unsplash, open Your apps and create a New Application. Supply its Access Key through api_key_env, not its Secret Key or a user's password. New applications start in demo mode; request production approval when the intended usage needs it.

search_photos and get_photo use public authentication with Authorization: Client-ID and need no end-user OAuth scope. Demo applications have a 50-request hourly limit. Image display must follow Unsplash's attribution and hotlinking guidance; downloading or selecting an image for use may require download tracking, which these two operations do not implement. Decentralized installations need their own approved key arrangement, not a shared embedded developer key.

Account-specific checks: Application approval, current rate allowance and the consuming application's attribution, hotlinking and any download-tracking implementation are not verified here.

References: [Provider documentation 1](https://unsplash.com/documentation#creating-a-developer-account), [Provider documentation 2](https://unsplash.com/documentation#public-authentication), [Provider documentation 3](https://help.unsplash.com/en/articles/2511245-unsplash-api-guidelines).

### webex

For user-authorized operation, sign in to Webex for Developers, open My Webex Apps > Create a New App > Create an Integration, register the host's real OAuth redirect URI and select only the required integration scope. Retain the client secret with the host credential owner; the host handles user consent, callback, exchange and refresh and supplies only the access token through api_key_env. A developer personal access token is suitable for short-lived exploration, not a durable production credential. An existing bot token represents the bot, not the user's account; do not expect it to list the user's spaces. No Webex token is created by setup help.

list_rooms calls GET /v1/rooms and needs spark:rooms_read for an integration. It lists spaces the authenticated principal belongs to, not every space in the organization; ensure that user or bot has membership in the required spaces. Use the narrow room-read grant rather than substituting spark:all. No room write, message-read, organization admin or compliance scope is needed for this selected listing. The adapter requests at most 100 rooms and refuses a paginated response, so membership in many rooms is a coverage limitation, not a reason to expand privileges.

Account-specific checks: The token principal, actual room memberships, token lifetime and organization's integration approval restrictions have not been observed. Whether the principal's visible room list fits the adapter's single-response limit is unverified; this help does not certify complete organization coverage.

References: [Provider documentation 1](https://developer.webex.com/admin/docs/integrations), [Provider documentation 2](https://developer.webex.com/admin/docs/integration-scopes), [Provider documentation 3](https://developer.webex.com/meeting/docs/getting-your-personal-access-token), [Provider documentation 4](https://developer.webex.com/messaging/docs/api/v1/rooms/list-rooms), [Provider documentation 5](https://developer.webex.com/blog/announcing-the-official-webex-postman-workspace), [Provider documentation 6](https://developer.webex.com/messaging/docs/bots).

### x

Obtain a developer account and create/select a Project and App in the X Developer Console. For these public reads, the host can use the app's Bearer Token from its Keys and Tokens/credentials page. Current self-service API access uses prepaid pay-per-usage credits; review endpoint pricing, available balance and spending limit before enabling requests. If acting for a user instead, the host configures OAuth 2.0 Authorization Code with PKCE and its actual registered callback, completes user consent and supplies that access token through api_key_env. API key/secret and OAuth 1.0a access-token/secret pairs are not interchangeable with this single bearer input.

get_user_info calls GET /2/users/by/username/{username}; search_posts calls GET /2/tweets/search/recent. Both accept OAuth 2.0 app-only bearer access for public data. If using an OAuth 2.0 user-context bearer, the authentication mapping requires tweet.read and users.read for either selected operation; these are not scopes to attach to an app-only token. No tweet.write, follow, direct-message or email scope is needed. offline.access is relevant only if the host separately implements renewal. search_posts covers recent search (last seven days), not full archive. include_post_metrics only adds public_metrics, not private analytics. Billing and endpoint rate limits still apply to read-only requests.

Account-specific checks: Developer/app approval, current endpoint entitlement, credit balance/spending limit, granted user scopes if applicable and token validity remain unverified. The pinned adapter still sends requests to api.twitter.com, while current documentation uses api.x.com. Reachability and compatibility of that retained origin for this account have not been tested; these records do not change the runtime origin.

References: [Provider documentation 1](https://docs.x.com/x-api/users/lookup/introduction), [Provider documentation 2](https://docs.x.com/make-your-first-request), [Provider documentation 3](https://docs.x.com/x-api/getting-started/pricing), [Provider documentation 4](https://docs.x.com/fundamentals/authentication/overview), [Provider documentation 5](https://docs.x.com/fundamentals/authentication/oauth-2-0/authorization-code), [Provider documentation 6](https://docs.x.com/fundamentals/authentication/guides/v2-authentication-mapping), [Provider documentation 7](https://docs.x.com/x-api/posts/search/introduction).

### zendesk

Reviewed 2026-10-03. Set `auth_mode=oauth_access_token` for a host-owned OAuth access token, supplied through `api_key_env`. Do not supply `account_email` in OAuth mode. Zendesk documents Bearer authentication for this article-search route. The selected instance, client registration, subject, scope, expiry and article visibility must be verified by the host; this adapter does not acquire or refresh tokens. Multi-customer integrations require global OAuth. The Search Articles documentation does not establish an endpoint-specific scope list; the scopes listed for Embeddable Widget Search belong to a different endpoint.

Omitting `auth_mode`, or selecting `api_token`, preserves the legacy Basic-token route and requires `account_email`. This route is unavailable to accounts created on or after July 28, 2026. An eligible older account can obtain a token through Admin Center, Apps and integrations, APIs, API tokens. New token creation ends October 27, 2026; all tokens stop April 30, 2027. Keep the credential in host custody. An API token can authenticate as other verified users when paired with their email, so its authority is broader than this connector's selected article-search operation.

`search_zendesk` searches Help Center articles only. Use the intended brand's subdomain. Account-specific checks include the intended subject's article visibility and, for legacy mode, continued token eligibility and activity state. The connector performs no mode guessing, retry with another credential, or account verification.

References: [Authentication](https://developer.zendesk.com/api-reference/introduction/security-and-auth/), [Search Articles](https://developer.zendesk.com/api-reference/help_center/help-center-api/help_center_search/), [Help Center introduction](https://developer.zendesk.com/api-reference/help_center/help-center-api/introduction/), [API-token retirement](https://support.zendesk.com/hc/en-us/articles/10840968198042-Announcing-the-removal-of-API-tokens-as-an-authentication-method-for-API-requests), [OAuth migration](https://developer.zendesk.com/documentation/authentication/oauth-migration/).

### zoom

In Zoom App Marketplace, open Develop/Created Apps and Build App. Use a General OAuth app with user-managed access for each consenting user's own meetings, or admin-managed access for authorized account-wide access. Configure the host's actual OAuth redirect URL and complete applicable account approval/distribution requirements. For unattended access solely within your own Zoom account, an authorized account developer can instead create and activate a Server-to-Server OAuth app with the selected scopes; that route uses account credentials rather than a user callback. The host obtains and renews the access token and supplies api_key_env. Client ID, client secret and account ID are not bearer tokens for this adapter.

For list_meetings, request granular meeting:read:list_meetings for user-level access, or meeting:read:list_meetings:admin for approved account-level access. User-level calls must use user_id=me; account access to another host requires the corresponding account authority. For get_meeting, request meeting:read:meeting or meeting:read:meeting:admin respectively. Do not request the other operation's granular scope when that operation is disabled. Existing classic meeting:read/meeting:read:admin grants are documented alternatives, but are broader. No meeting write, recording, webinar or master-account scope is needed. get_meeting requires a host with Zoom Meetings Basic or higher. list_meetings is the scheduled/unexpired-meeting endpoint and returns only the retained first-page view, not all historical or instant meetings.

Account-specific checks: The selected app's activation/approval, user-managed or account-managed authority, granted scopes, account developer permissions and access-token validity remain unverified. The requested meeting/host must belong to the access actually granted to this token; host license, account restrictions and current meeting visibility are not established by configuration help.

References: [Provider documentation 1](https://developers.zoom.us/docs/integrations/create/), [Provider documentation 2](https://developers.zoom.us/docs/build-flow/basic-info/app-mgmt/), [Provider documentation 3](https://developers.zoom.us/docs/internal-apps/create/), [Provider documentation 4](https://developers.zoom.us/docs/integrations/oauth-scopes-granular/), [Provider documentation 5](https://developers.zoom.us/docs/api/meetings/).

## Standard Meerkat configuration

For a project whose model provider is already configured, add the selected server through the ordinary Meerkat CLI:

```bash
rkat mcp add toolkit-hackernews --scope project -- \
  /absolute/path/repository/.venv-agno-mcp/bin/python \
  -m meerkat_toolkit.adapters.agno_mcp serve --config /absolute/path/hackernews.json
```

This configures the project; it does not mutate an existing running session. A new run with `--wait-for-mcp` waits for initial MCP discovery. Meerkat exposes these tools through its standard deferred catalog: the agent uses `tool_catalog_search` and `tool_catalog_load` before calling the selected tool.

The installed `rkat 0.8.47` path was verified in fresh explicit context, state and user-config roots. A deterministic loopback model fixture drove search, load and one actual Hacker News top-story read through the installed Toolkit wheel. [The retained receipt](../../docs/verification/20261002-agno-mcp/native-acceptance.json) binds the binary, wheel and observed tool result. This does not prove agent reasoning, dynamic discovery/install, MobKit attachment, requester authority or PubMed native/live execution. The first fixture's incorrect inline-tool assumption is retained separately.

## Available operations and limits

Each MCP server admits at most four selected calls: one executing and three
waiting. Admission precedes argument validation and worker creation. Excess
calls return `server_busy`; a call still waiting after 30 seconds returns
`queue_timeout`. Both refusals have `entered=false`, with `not_started` for a
write. Neither refusal causes an automatic retry. The queue deadline ends
before execution and does not replace the selected provider's operation budget.
Cancellation retains the execution slot until its worker joins. Listing and
protocol control remain available while calls wait or execute. Hosts retain
profile lifetime and any coordination between separate servers or direct calls.

The selected MCP tool list preserves each upstream name and input schema,
with these additional profile constraints. The server validates them before
entry into the upstream function:

| Profile and operation | MCP input limits | Provider behavior |
| --- | --- | --- |
| Hacker News `get_top_hackernews_stories` | Optional `num_stories`: integer 1-50, upstream default 10 | Fetches the top-story IDs, then at most that many story records |
| Hacker News `get_user_details` | Required `username`: 1-64 ASCII letters, digits, `_` or `-` | Fetches one selected public user record |
| PubMed `search_pubmed` | Required `query`: 1-4096 characters; optional `max_results`: integer 1-50 | Searches PubMed, then fetches article details; omitted count uses configuration |
| SearXNG `search_web`, `news_search` | Required `query`: 1-4096 characters; optional `max_results`: integer 1-5, default 5 | One selected instance request; count truncates results after response reading |
| Tavily `web_search_using_tavily` | Required `query`: 1-4096 characters; optional `max_results`: integer 1-5, default 5 | One basic-search request; donor also limits accumulated serialized result characters to 6000 |
| Financial Datasets `get_income_statements`, `get_balance_sheets` | Required `ticker`: 1-32 ASCII letters/digits/`.`/`-`, starting with a letter/digit; optional `period`: `annual`, `quarterly` or `ttm`; optional `limit`: integer 1-10 | One statement request; defaults are `annual` and 10, and pagination links remain data |
| OpenWeather `get_current_weather`, `get_air_pollution` | Required `location`: 1-256 characters | One geocoding request, then one data request using the first location match |
| OpenWeather `get_forecast` | Same location bound; optional `days`: integer 1-5, default 5 | Geocoding, then at most 40 requested 3-hour forecast entries |
| OpenWeather `geocode_location` | Same location bound; optional `limit`: integer 1-5, default 1 | One request for up to that many location matches |
| Bitbucket `list_repositories` | Optional `count`: integer 10-50, default 10 | First repository page only; pagination links are returned as data |
| Bitbucket `get_repository_details` | No arguments | One read of the configured repository |
| Bitbucket `list_all_pull_requests` | Optional `state`: `OPEN`, `MERGED`, `DECLINED` or `SUPERSEDED`, default `OPEN` | First page only, at most 20 entries; does not enumerate all PRs or follow `next` |
| Bitbucket `get_pull_request_details` | Required `pull_request_id`: integer 1-2147483647 | One PR detail request in the configured repository |
| GitLab `list_projects` | Optional `search`: 1-256 characters; `owned` and `membership`: booleans; `page`: 1-10000; `per_page`: 1-50 | One requested page, default page 1 and size 20; no automatic pagination |
| GitLab `get_project` | Required `project_id_or_path`: 1-256 characters, ASCII letters/digits/`_`/`.`/`-` segments starting with a letter/digit and separated by `/` | One project request; selected upstream fields are serialized |
| GitLab `list_merge_requests` | Same project bound; `state`: `opened`, `closed`, `merged` or `all`; optional source/target branch and author filters: 1-256 characters; same page bounds | One project request, then one selected page of merge requests |
| GitLab `get_merge_request` | Same project bound; required `merge_request_iid`: integer 1-2147483647 | One project request, then one merge-request detail request |
| GitLab `list_issues` | Same project bound; `state`: `opened`, `closed` or `all`; optional labels, author, assignee and search filters: 1-256 characters; same page bounds | One project request, then one selected issue page; labels are a comma-separated string |
| ClickUp `get_task`, `list_spaces` | `get_task` requires `task_id`: 1-128 ASCII letters/digits/`_`/`-`; `list_spaces` takes no arguments | One selected task read, or one space-list request for the configured workspace; no name matching |
| Discord `get_channel_info`, `get_channel_messages`, `list_channels` | Required `channel_id` or `guild_id`: 1-20 ASCII digits; messages accept `limit`: 1-100, default 100 | One bot-authenticated read; no message sends or frontend lifecycle |
| Shopify `get_shop_info`, `get_products` | Shop info takes no arguments; products accept `max_results`: 1-50, default 50, and optional `status`: `ACTIVE`, `ARCHIVED` or `DRAFT` | One GraphQL POST; first product page and first 10 variants per product |
| Spotify `get_current_user`, `get_my_top_tracks`, `get_user_playlists`, `search_tracks` | Current user takes no arguments; top tracks use `limit`: 1-50, default 20, and `time_range`: `short_term`, `medium_term` or `long_term`; playlists use `max_results`: 1-50, default 20; search uses `max_results`: 1-10, default 10, and requires `query`: 1-1024 characters, optional two-uppercase-letter `market` | One selected-token request per operation; first pages, no playlist updates or playback |
| Unsplash `search_photos`, `get_photo` | Search requires `query`: 1-1024 characters; `per_page`: 1-30, default 10; `page`: 1-10000, default 1; orientation/color use the schema's fixed enums. Photo requires `photo_id`: 1-128 ASCII letters/digits/`_`/`-` | One metadata request; author attribution and links retained; no image download |
| Cal.com `get_available_slots` | Required `start_date`/`end_date`: `YYYY-MM-DD` in 1970-2037; real dates, ordering and at most 31 inclusive UTC dates checked before HTTP | One event-type availability read, formatted in the selected timezone |
| Cal.com `get_upcoming_bookings` | Optional `email`: 3-254 characters, one non-whitespace local/domain pair | One first-response account read, optionally attendee-filtered; not event-type filtered |
| Jina `read_url`, `search_query` | URL: 1-4096 characters, HTTP(S); query: 1-1024 characters | One fixed-provider request, JSON-to-string conversion and configured character truncation |
| OpenRouteService `geocode_location`, `get_directions` | Location/start/end: 1-512 characters; directions use the donor's fixed road-profile enum | One or at most three requests, respectively; original first-geocode and route-summary formatting |
| Zendesk `search_zendesk` | Required `search_string`: 1-1024 characters | One Help Center article response, original regex HTML removal; no pagination |
| Bright Data `scrape_as_markdown`, `search_engine` | URL: 1-4096 characters, HTTP(S); query: 1-1024 characters; engine: `google`, `bing` or `yandex`; count: 1-50; language/country: two ASCII letters | One fixed-provider request; successful UTF-8 Markdown under `content`; count hints only apply to Google |
| Linkup `web_search_with_linkup` | Query: 1-1024 characters; optional depth/output choices match configuration enums | One synchronous search; actual SDK model projected to plain JSON |
| Todoist `get_task` | Task ID: 1-128 ASCII letters/digits/`_`/`-` | One real SDK task read and original donor serialization |
| Zoom `list_meetings`, `get_meeting` | User ID: 1-128 ASCII letters/digits/`_`/`-`, default `me`; type: `scheduled`, `live`, `upcoming` or `previous`; meeting ID: 1-20 ASCII digits | First meeting-list page or one detail response; no OAuth/refresh or pagination |
| Exa `search_exa` | Required `query`: 1-4096 characters; `num_results`: 1-20; optional `category`: fixed donor enum, omit rather than pass `null` | One SDK search-and-contents POST; donor text cap 10000 characters per result; no jobs |
| Firecrawl `scrape_website`, `map_website` | Required `url`: 1-4096 characters, absolute HTTP(S), no userinfo, controls or backslash | One fixed-provider POST; real Markdown document or map model; maps over 100 links fail |
| Trello `list_boards` | Optional `board_filter`: `all`, `open`, `closed`, `organization`, `public` or `starred` | One key/token-authenticated board-list request; real SDK request/model port preserves optional metadata presence; lists over 100 fail |
| Salesforce `get_record` | Required `sobject`: uppercase initial plus ASCII letters/digits/`_`, at most 80 characters; required `record_id`: 15 or 18 ASCII alphanumerics; optional `fields` must be `""` | One complete record in the configured instance using an existing session token; no SOQL |
| Twilio `get_call_details` | Required `call_sid`: `CA` plus 32 hex characters | One call read in the configured account, actual SDK model with the documented `_from` compatibility port; malformed present timestamps fail |
| Webex `list_rooms` | No arguments | One page requested with `max=100`; any pagination `Link` or more than 100 rooms fails instead of returning an incomplete list |
| Plivo `get_call_details` | Required `call_uuid`: hexadecimal UUID in 8-4-4-4-12 form | One call read in the selected account; duplicate SDK query ID must equal the path ID |
| arXiv `search_arxiv_and_return_articles` | Required `query`: 1-4096 characters; optional `num_articles`: 1-20 | One bounded Atom page using actual SDK parsing; no PDF/file operations |
| Reddit `get_top_posts` | Required `subreddit`: 1-32 ASCII letters/digits/`_`/`-`; optional `limit`: 1-25 and `time_filter`: `hour`, `day`, `week`, `month`, `year` or `all` | One real PRAW listing response; a short nonterminal page fails |
| Reddit `get_subreddit_stats`, `get_user_info` | Required `subreddit` or `username` with the same identifier bounds | One actual SDK object read; response identity must match the request |
| X `get_user_info` | Required `username`: 1-15 ASCII letters/digits/`_` | One real Tweepy user read with the selected bearer token |
| X `search_posts` | Required `query`: 1-512 characters; optional `max_results`: 10-100 | One recent-search page using real SDK models and author expansions; no pagination |
| YouTube `get_youtube_video_data` | Required exact supported HTTPS watch/embed/v or youtu.be URL with an 11-character ASCII video ID | One public oEmbed response; metadata only |
| Oxylabs `search_google`, `get_amazon_product` | Required nonblank `query`: 1-512 characters, or `asin`: exactly ten ASCII alphanumerics; optional `domain_code`: finite published enum | One real synchronous SDK response with at most one result and 50 organic entries; supplied job metadata must say `done` |
| Wikipedia `search_wikipedia` | Required exact title `query`: 1-256 characters, no controls or `|` | At most five bounded API requests including three title redirects; up to 50 disambiguation options |
| AWS Lambda `list_functions` | No arguments | Actual botocore serialization, SigV4 and parsing; at most 50 functions and no continuation marker |
| PostgreSQL `show_tables` | No arguments | Visible table metadata in the selected local schema; bound failure instead of partial enumeration |
| PostgreSQL `describe_table` | Required `table`: nonempty identifier at most 63 UTF-8 bytes, without NUL | Visible column metadata with bound SQL parameters and correctly escaped CSV |

These limits belong to the MCP profile. Calling `profile.adapter.invoke`
directly uses the generic bridge's original schema, not the extra MCP limits.
The username restriction describes this supported profile, not all usernames
that the upstream service might accept.
The upstream-generated PubMed schema rejects an explicit `null` count even
though the Python function annotation is optional; omit the argument to use
the configured default.
GitLab's optional filter schemas likewise reject explicit `null`; omit filters
that are not needed. List state defaults to `opened` for merge requests/issues.

Hacker News can make up to 51 sequential requests for one call. `timeout` is a
per-request HTTP timeout, not a whole-operation deadline. Input limits cap
requested work. The five ClickUp/Discord/Shopify/Spotify/Unsplash helpers and
four Cal.com/Jina/OpenRouteService/Zendesk ports and four
Bright Data/Linkup/Todoist/Zoom profiles and eight Exa/Firecrawl/Trello/Salesforce/
Twilio/Webex/Plivo/arXiv profiles and six Reddit/X/YouTube/Oxylabs/Wikipedia/
AWS Lambda profiles add a network response byte limit; earlier profiles retain their existing
response-reading behavior. The generic
bridge's 64 KiB argument and 1 MiB result bounds apply to plain JSON values;
the result bound runs after upstream response reading and parsing. There is
no adapter-generated truncation, pagination completion claim or automatic
retry. The donor-specific result selection described in the table still
applies, including Tavily's serialized-character budget and SearXNG's count
selection.

## Results and failures

Successful values are preserved under MCP `structuredContent.value`. A string
also appears unchanged in text content. Many upstream functions return JSON
encoded strings, which callers may parse for fields. OpenRouteService returns
an object, Cal.com returns formatted text, and Jina retains Python string
conversion of the parsed JSON value. The server does not silently replace
strings with objects.

Bright Data and Linkup apply the successful output projections documented
above. Reviewed upstream implementations also catch provider exceptions and return
strings with these exact prefixes:

- `Error fetching stories: `
- `Error getting user details: `
- `Could not fetch articles. Error: `
- `Error fetching results from searxng: `
- `API key not set` or `Error making request to ` for Financial Datasets
- `Error getting channel info: `, `Error getting messages: ` or `Error listing channels: ` for Discord
- `Error:`, `Error searching Unsplash: ` or `Error getting photo: ` for Unsplash
- `Error: ` for the two Cal.com source ports and Linkup
- `Please provide `, `Error scraping URL ` or `Error searching for query ` for Bright Data
- `Error: ` for Exa
- `Error getting top posts: `, `Error getting subreddit stats: ` or `Error getting user info: ` for Reddit
- `Error listing functions: ` for AWS Lambda

The profile maps its own operation's prefix to `isError: true`, a fixed
`Upstream operation failed` text and bounded `upstream_operation_error`
metadata. It omits the raw diagnostic string because it may contain a contact
email, query or provider URL. This is a mapping of the reviewed return
conventions, not a general natural-language result classifier. Tavily SDK
exceptions use the bridge's bounded error envelope. The generic
Agno bridge continues to preserve upstream return values.

OpenWeather, Bitbucket, GitLab, Todoist, Zoom, the eight library-batch profiles
and the five service-helper
profiles, plus the selected X/YouTube/Oxylabs/Wikipedia ports, have a separate
exact reviewed convention:
their selected methods return a JSON-encoded object containing only the key
`error` on failure. For these operations only, that exact object shape maps
to the same redacted MCP error envelope. Nested `error` fields, ordinary text
and objects with additional fields are not classified using this rule. It
does not search message text or infer business success from a successful
Python return. Successful serialized provider values remain unchanged.

Jina, OpenRouteService, Zendesk and Trello source-port failures raise fixed exceptions
and use the bridge's bounded error envelope. Successful Jina content beginning
with an error-like sentence remains content; it is not classified by prose.

Other bridge failures use a bounded `error` object with `kind`, `effect`,
`entered` and `write_outcome`. For these selected reads, `write_outcome` is
`not_applicable`. A trusted custom profile may expose writes only by explicitly
classifying them in the underlying bridge. A rejected write has
`not_started`; a failure after crossing its invocation boundary has `unknown`.
An error never requests replay. MCP call completion is not a business-task
completion receipt.

## Concurrency and cancellation

This server supports the asyncio backend only. It offloads synchronous calls
to a worker and serializes them per server. The reviewed profiles use per-call
HTTP clients or exclusively owned Requests sessions and explicitly permit that
worker handoff. Use one profile per server, without concurrent external
borrowers. The server refuses a custom profile unless the trusted host
sets `thread_handoff_safe=True`; that assertion is the host's responsibility.
Do not assume arbitrary borrowed SDK clients are safe across threads.

A canceled call waiting for the lock does not enter the upstream function.
Once a worker has started, the server holds the lock until the actual worker
returns, including repeated direct `asyncio.Task.cancel()` calls. This keeps
another call from overlapping an unfinished operation. Tool listing remains
responsive while the worker runs. Ping is supported in MCP's legacy mode;
the current MCP 2.2 protocol removes it.

Python cannot force a synchronous upstream function to stop safely. A client
may stop awaiting a result before the server finishes cleanup. Process loss,
forced termination or transport loss can also remove the receipt. These
events do not prove that a started operation had no effect. The host owns
deadlines, shutdown policy and reconciliation of missing outcomes; this
component supplies no durable job or grant authority.

## Python composition

For an existing asyncio MCP host, the caller owns the built profile and closes
it only after the server and all calls have joined:

```python
from meerkat_toolkit.adapters.agno_mcp import create_server
from meerkat_toolkit.adapters.agno_profiles import build_profile

async def serve_selected(read_stream, write_stream):
    profile = build_profile(
        "hackernews",
        operations=["get_top_hackernews_stories"],
        config={"timeout": 10},
    )
    try:
        server = create_server(profile)
        await server.run(
            read_stream, write_stream, server.create_initialization_options()
        )
    finally:
        profile.close()
```

Use the official MCP SDK to run that server on a host-owned transport. For
stdio, prefer `serve_stdio(configuration)`, where `configuration` comes from
`load_configuration(path)` or `validate_configuration(...)`. That entrypoint
builds the toolkit inside the claimed stdio context. Constructing a profile
before handing it to an unrelated stdio transport cannot provide that ordering
guarantee.

`create_server` borrows the profile and does not close it. `profile.close()` is
idempotent; it is not a cancellation primitive and must not race an active
worker. Do not reuse a closed profile. The CLI owns construction and cleanup,
including cleanup after partial construction or adapter rejection. Its
asynchronous shutdown also joins a pending close operation through caller
cancellation. Forced process loss still provides no cleanup guarantee.

## Verification

The focused tests use actual Agno toolkits/functions, actual MCP clients and
the real stdio subprocess transport. They replace `httpx.get` or the Requests
HTTP adapter only inside controlled test fixtures and never contact a provider. Production code does
not patch global HTTP behavior.

```bash
PYTHONPATH=sdk/python/src python -m pytest \
  sdk/python/tests/test_agno_adapter.py sdk/python/tests/test_agno_mcp.py \
  sdk/python/tests/test_agno_expansion.py sdk/python/tests/test_agno_next_batch.py \
  sdk/python/tests/test_agno_service_batch.py sdk/python/tests/test_agno_research_batch.py \
  sdk/python/tests/test_agno_next_services.py sdk/python/tests/test_agno_library_batch.py \
  sdk/python/tests/test_agno_http_readers.py -q
```

Coverage includes schema selection and bounds, actual upstream parsing,
failure-prefix redaction, write uncertainty, result bounds, invalid config,
clean stdout, current MCP client calls, repeated direct cancellation, canceled
queues, legacy stdio ping while a worker is active, and a raw `2024-11-05`
initialize/list/rejected-call exchange. That last exchange establishes wire
compatibility only, not Meerkat or MobKit runtime acceptance. Installed-wheel
and real-provider evidence must identify the exact artifact, operation and
host profile separately before any adoption claim.
The OpenWeather/Bitbucket/GitLab batch exercises all 13 inherited read methods, actual
python-gitlab parsing, explicit credential binding, ambient account/transport
canaries, fixed first-page limits, exact JSON errors, SDK `429` non-retry,
partial-construction cleanup and session shutdown through cancellation.
The five-service batch adds another 13 inherited read methods, exact donor
source hashes, schema rejection before dispatch, distinct selected accounts,
actual Requests streaming/closure, HTTP and GraphQL failures, response bounds,
and joined cancellation. These are offline source tests; they do not extend
the earlier installed wheel's evidence to newly changed source bytes.

The eight-profile library batch has 19 new author methods exercising nine
selected operations with real SDK methods/models, both MCP client modes,
selected account isolation, config/schema boundaries, malformed collections,
no retries/redirects, credential-diagnostic redaction, Webex pagination failure,
arXiv skipped-entry rejection, exact donor bytes and joined cancellation.
Trello regressions check true/false/null/absent optional metadata through actual
SDK calls. Twilio regressions check full-input grammar, trailing/control text,
comments, whitespace, weekday/calendar agreement and empty/malformed/non-UTC
failures, plus valid UTC/missing/null results before lossy SDK parsing.
All 106 author methods and 32 unchanged retained independent methods pass
against source with 96 pinned dependencies, alongside the 11 corrected reviewer
methods and two additional correction methods. The original failing evidence
and its diagnostic-only test successor remain separately preserved. These are
local controlled-provider tests. The accepted Agno29 installed proof now binds those 151 methods in two
detached consumers; that evidence remains specific to its frozen artifact.
The [corrected source handoff](../../docs/verification/20261003-agno-library-batch/CORRECTION-V3-HANDOFF.md)
binds those results, SDK wheel payloads and per-profile dependency closures.

The six-profile HTTP source batch adds 21 author methods covering all ten
operations in both official MCP client modes, real SDK types/serialization,
full URL checks, explicit account/config isolation, malformed/empty responses,
HTTP/error/size bounds, selected identities, Wikipedia disambiguation/redirect
bounds, real OAuth1 and SigV4 signatures, debug diagnostic redaction and joined
cancellation. Its independent review and installed-artifact proof are pending;
source tests do not promote these six integrations by themselves.
