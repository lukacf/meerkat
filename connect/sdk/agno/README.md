# Agno integration component

`meerkat-toolkit-agno` packages the shared Agno adapter as an independent Python distribution. It supports 36 explicit MCP profiles, including PostgreSQL table and column metadata over a selected local Unix socket. The PostgreSQL-era Agno36 predecessor's retained proof covers 220 installed methods in the full consumer, 29 PostgreSQL methods in a fresh portable consumer, 111 exact runtime pins and all 36 CLI descriptions in both; those are historical evidence counts, not a claim that each successor reruns them. PostgreSQL uses a selected SCRAM principal, one active invocation owner and at most one MCP server per profile, real read-only metadata statements, rollback and joined cleanup. Remote TCP/TLS and arbitrary SQL are excluded. Inventory adoption is specific to a verified artifact. The canonical implementation remains under `sdk/python/src/meerkat_toolkit/adapters`; the staging command copies an exact allowlist into the separate `meerkat_toolkit_agno` namespace without rewriting modules. There is one maintained source implementation.

This component excludes unrelated private paired host modules. Its distribution name and namespace do not overlap the combined SDK, so either can be installed without overwriting the other's files. No PyPI publication is claimed.

## Build a local wheel

Version `0.1.2` preserves the Wikipedia API's source URL, canonical title and
page ID in the existing Document metadata, alongside the selected language and
the local UTC time at which the response was received. It adds no request or
dependency. The retrieval time is an observation, not a content freshness claim;
the extract is not represented as bound to a revision. Independent installed
review precedes canonical library selection.

Version `0.1.1` introduced bounds on the existing Bitbucket response body and checks its
deadline between available body reads. It rejects compressed responses and
HTTP failures before consuming their bodies, preserves one request per call
and joins cleanup. Synchronous DNS, connection setup, HTTP framing and custom
transports remain subject to their own blocking behavior; this is not a hard
whole-operation wall-clock guarantee. The candidate requires independent
installed-component review before canonical library selection.

From the repository root, choose a new staging directory whose parent exists:

```sh
python3 sdk/agno/stage.py /tmp/my-toolkit-agno-source
python3 -m pip wheel --no-deps --wheel-dir /tmp/my-toolkit-agno-wheels /tmp/my-toolkit-agno-source
```

Staging installs nothing, does not modify canonical modules and refuses an existing destination. The build backend is pinned. The wheel contains a source map and retained notices. Install the exact resulting wheel into the consuming Python environment, retain its digest and freeze that environment's dependency graph. Python 3.11 or later is required.

The staged `PROFILE-GUIDE.md` supplies every profile's configuration and runtime limits. It is generated from the canonical `sdk/python/AGNO-MCP.md`, retaining its configuration sections and adapting the module namespace. For Hacker News, a minimal public profile is:

```json
{"profile":"hackernews","operations":["get_top_hackernews_stories"],"config":{"timeout":10}}
```

`config-help` also returns `selected_guide`: complete applicable guidance for the
exact selected profile, composed at build time from checked manual spans and the
package's existing field/operation metadata. The installed validator remains
authoritative. Existing prerequisites and unresolved details remain separate
fields. The optional setup UI uses this selected guidance by default; its
existing `setup_help` tool accepts `full_guide=true` for the whole family manual.
Packages without selected guidance retain their full-manual default.

`guide.package_resource` identifies the full manual using an importlib resource
package and name in either Python distribution. The selected guide's
`full_guide_sha256` hashes those exact projected manual bytes, not the unprojected
repository document. The legacy standalone/source-document locators remain.
Missing or malformed selected package data causes a bounded command failure;
it does not quietly remove constraints or imply account readiness.

To regenerate both source data files after reviewing a canonical manual change,
run from the repository root:

```sh
guide_stage_parent=$(mktemp -d "${TMPDIR:-/tmp}/toolkit-agno-guides.XXXXXX")
python3 -B sdk/agno/stage.py "$guide_stage_parent/component"
cp "$guide_stage_parent/component/src/meerkat_toolkit_agno/CONFIGURATION-GUIDES.json" sdk/python/src/meerkat_toolkit/adapters/CONFIGURATION-GUIDES.json
cp "$guide_stage_parent/component/src/meerkat_toolkit_agno/PROFILE-GUIDE.md" sdk/python/src/meerkat_toolkit/adapters/PROFILE-GUIDE.md
python3 -B -m unittest discover -s sdk/agno/tests -v
```

Do not edit these derived files independently. The maintained test recomposes
their bytes and compares the combined SDK with standalone staging. Composition
fails if the full manual digest, checked spans or exact profile/field/operation
coverage drift; update `sdk/agno/profile-guide-selections.json` through review
when intended source changes alter those contracts. The accepted older artifact
and current source manual are explicit digest-bound variants. Their respective
selected guides retain their own runtime semantics.

Launch the installed `meerkat-toolkit-agno-mcp` executable with `serve --config /absolute/path/profile.json`, or use:

```sh
python -m meerkat_toolkit_agno.agno_mcp serve --config /absolute/path/profile.json
```

`describe --config /absolute/path/profile.json` uses the same implementation. Authenticated profiles require their documented selected environment-key references; the host owns provider consent, credential storage, account scope and process lifetime. PubMed requires an operator contact email. Configuration and profile startup do not establish account readiness.

This configuration-assistance edition supplies `profiles`, `config-help --profile <exact-id>`
and `check-config --config <file>` without runtime imports or credential reads.
They list explicit choices and validate nonsecret configuration, not account
readiness. The packaged `PROFILE-GUIDE.md` describes the interface and semantic
restrictions. These commands are absent from the PostgreSQL-era Agno36 predecessor
with SHA-256 `a6965820306cd00978687559859a0854aa900bbe19b99a2ebf0e15be80779ef6`.
The [support profile](../../docs/adoption/AGNO-SUPPORT.md) selects the current
supported artifact by digest; building an edition alone does not promote it.

All 29 authenticated remote profiles include dated provider registration and operation-specific permission guidance in `config-help` and the packaged guide. The six public profiles and local PostgreSQL keep their original prerequisites. Current-provider restrictions are explicit. OpenRouteService now uses the fixed api.heigit.org origin with separate geocoding and directions paths; the host must permit this changed origin under its existing network policy. No legacy-host fallback is supplied. Guidance does not implement token acquisition, renewal or native continuation.

Cal.com, Jina, OpenRouteService and Zendesk use explicit source ports of seven operations from pinned Agno 3.0.11, with the real Toolkit classes and original pure helpers. Their transport and error changes are documented in the profile guide and retained notices. Cal.com additionally pins `pytz==2026.4`; its date/time support is limited to 1970-2037 and availability to 31 inclusive UTC dates. Jina preserves JSON-to-string conversion and character truncation. These ports do not expose calendar writes, route matrices, Zendesk tickets or agent frontends.

The accepted 2026-10-04 Cal.com correction uses the documented `/v2/slots`
range response with version `2024-09-04` and `/v2/bookings` with version
`2026-05-01`. Bookings remain one page of at most 100; nonterminal pages are
explicitly marked incomplete. The [accepted predecessor evidence](../../docs/verification/20261004-calcom-api/REPORT.md)
records installed component and library verification. This successor retains
that correction; neither artifact claims live Cal.com account access.

Bright Data and Linkup retain real donor operation bodies, with explicit text/model output projections. Todoist retains its real donor operation and formatter with a constructor port that injects the actual SDK HTTPX client; that client borrows the same bounded Requests I/O owner. Zoom ports two preauthenticated reads without acquiring or refreshing tokens. The guide and notices identify these deltas and the three added exact dependency pins.

Exa, Firecrawl, Salesforce, Webex, Plivo and arXiv retain their selected donor operation bodies and run real provider SDK methods/models through one borrowed bounded transport. Their constructor and instance-client injection seams are documented in the guide. Trello uses an explicit operation port retaining the real SDK request and Board construction while preserving optional metadata presence, null and false values that the original model/donor pair loses. Twilio uses the real SDK's call fetch and model with an explicit operation compatibility port: its pinned model exposes `_from`, while the donor reads `from_`. It also requires each present timestamp to match the complete supported `Wdy, DD Mon YYYY HH:MM:SS +0000` or `GMT` form, with valid calendar/time and matching weekday. Extra text, whitespace, comments and other forms fail before lossy SDK parsing. Missing/null times retain the documented donor projection. Webex returns no successful incomplete enumeration; a pagination header or more than 100 rooms is an error. Salesforce exposes complete-record reads without SOQL. arXiv has no PDF/download/filesystem operation; `pypdf` is required only by the donor import.

The accepted Agno29 component's SDK closure added 33 exact dependency pins while preserving all 63 earlier versions. The retained audit records every added wheel, installed payload, notice and each profile's declared dependency closure so optional install groups can later share one implementation. None of these service SDK dependencies is added to core Meerkat or MobKit. The source handoff does not replace installed-artifact acceptance.

The library exposes selected read operations, with explicit input/result bounds, normal TLS, no ambient account fallback and joined cleanup. Synchronous provider calls may delay cancellation until their actual I/O settles. Other donor operations, OAuth acquisition, automatic installation, native activation and original-task continuation remain separate work. An immutable installed proof must cover the exact successor component before its evidence can replace an earlier component wheel.

The accepted Agno35 predecessor added Reddit, X, YouTube public metadata,
Oxylabs, Wikipedia and AWS Lambda reads. It retained actual donor/SDK execution
or explicitly attributed narrow ports, documented in the guide and notices.
Agno36 preserves all 109 predecessor dependency versions and adds Psycopg and
Psycopg Binary 3.3.6 as unmodified, separately installed LGPL-3.0-only dependencies.
The [support profile](../../docs/adoption/AGNO-SUPPORT.md) identifies the current
supported artifact. The [PostgreSQL-era acceptance](../../docs/verification/20261003-agno-postgres-batch/root-acceptance.json)
binds the predecessor digest above. Earlier artifacts and live account receipts
retain their original scope.

Zendesk accepts an existing OAuth access token with `auth_mode=oauth_access_token` and `api_key_env`; omit `account_email` in this mode. Legacy API-token mode remains the default and requires an account email. The host owns OAuth acquisition, renewal and account verification. See the installed profile guide for configuration and provider eligibility.
