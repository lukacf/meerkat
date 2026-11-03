# Experimental Activepieces MCP, readers and writers

This optional Node package calls 63 real, pinned Activepieces actions: 59 reads and four writes. Its 55 prior read profiles cover
Tally, Asana, NocoDB, Baserow, Bitly, Clockify, Coda, PagerDuty, Postmark and
Typeform, Google Contacts, Figma, Acuity Scheduling, Confluence, Brevo, Jotform,
Kit, MailerLite, Calendly, Google Tasks, Campaign Monitor, Okta, Help Scout,
Airtable, Toggl Track, Stripe, Pipedrive, WordPress, Google Calendar, Notion and
Intercom, Freshdesk, Google Forms, Google Slides, OneDrive, Google Docs,
Google Drive, Contentful, Outlook, HubSpot, Supabase, SendGrid, Webflow, Jira Cloud,
Dropbox, Google Sheets, Microsoft Excel 365, Microsoft Teams, QuickBooks Online,
Xero, Algolia, Ghost, monday.com, Mailchimp and Amazon S3. The latest source profiles add ActiveCampaign, Freshservice, Gitea and Datadog writes, plus Gmail, Slack, Linear and Google Cloud Storage reads. It retains their action and helper source and the real framework
constructors. A generated factory gives
each invocation its own host transport binding. It includes no Activepieces
workflow engine, global tool registration or OAuth acquisition workflow. The optional MCP
entrypoint resolves only explicitly configured credential environment variables.
Most profiles select one token; S3 additionally selects an access key ID and an
optional session token.

The CLI starts a standard MCP server from a small nonsecret configuration file.
The user or host must already own the selected credential. Dynamic dropdowns,
auth validators, connection-label callbacks, OAuth
acquisition/refresh and native Meerkat/MobKit registration are not implemented.
Adopted status is tied to the exact installed artifact and support disposition, not this source package alone.

## Build and install a local artifact

Use Node 22.19.0 or later. From this directory:

```bash
npm ci --ignore-scripts
npm test
npm pack
```

Install the resulting local tarball in the consuming host. No published Toolkit
package is assumed. The MCP entrypoint has pinned direct runtime dependencies:
the official `@modelcontextprotocol/server` and `@modelcontextprotocol/core`
2.3.0, and Undici 8.11.2. Donor SDK dependencies are bundled; their remaining
external imports are Node built-ins. Build dependencies are pinned in package.json and
package-lock.json. Build verifies all vendored
source SHA256 values and installed dependency versions before producing output.

The artifact includes donor source, the source manifest, license notices and a
build manifest with bundle and lockfile hashes. Exact statement projections for
nine index-exported auth definitions and one token helper are retained under
`dist/SOURCE-PROJECTIONS/`, with original source slices, line positions and hashes
in `dist/SOURCE-PROJECTIONS.json`. Vendor files themselves remain unchanged.
Its exact dependency lock is
retained as `dist/BUILD-DEPENDENCY-LOCK.json`, since npm excludes package-lock.json
from tarballs. See [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
Do not remove donor notices when redistributing a derived artifact.
Runtime dependencies retain their own license notices in their installed npm
packages. The retained build lock documents this build; a consuming project
must retain its own dependency lock to freeze transitive npm resolution.

## Start a configured MCP server

This configuration-assistance edition includes the following commands. They are
absent from the AP63 effects-expansion predecessor with SHA-256
`233906bc21c8a8f1e7d86f7a81b62fdc71120da7e82b1cacea2b16cb8e774ccf`.
The [support profile](../../docs/adoption/ACTIVEPIECES-SUPPORT.md) selects the
current supported artifact by digest:

```sh
meerkat-toolkit-activepieces-mcp profiles
meerkat-toolkit-activepieces-mcp config-help --profile amazon-s3.list-files
meerkat-toolkit-activepieces-mcp check-config --config /absolute/path/profile.json
```

`profiles` lists exact profile IDs, exposed operation names and read/write effects.
`config-help` reports accepted fields, primitive kinds, required/optional presence
and credential-reference fields. The latter contain environment-variable names,
never credentials. This is field help, not a complete JSON Schema; the profile
sections in this packaged guide retain semantic restrictions and account prerequisites.
It also returns `prerequisites` as `{kind, instruction}` rows and
`unresolved_details` for the exact selected profile. These explain whether
`token_env` means an API key, OAuth access token, application password, service
role key or another selected credential. For S3, the guidance distinguishes the
secret access key from its separate access key ID and optional session token.
All 63 profiles include provider setup guidance checked against the selected calls
and primary documentation on 2026-10-03. The guidance names the applicable token
or app registration route, documented permission options and account/resource
restrictions. Where the provider does not establish a granular read-only token,
the help says so; a read-only tool does not narrow a broader credential. The nine
Google Workspace records below remain unchanged.

Some reads require broader permissions: Excel's table-column API uses delegated
Files.ReadWrite, and Acuity and QuickBooks expose broad API scopes. OneDrive's
selected /me route requires a delegated user; Teams per-team application consent
differs from a tenant-wide application grant. Legacy API choices and unsupported
clouds remain explicit. Live account, tenant, actual grant and resource permission
checks remain unresolved for every profile. These static instructions neither
detect scopes nor approve a connection or execute an authorization flow.

These provider credentials belong to the local connector's outbound requests.
The MCP client launches a local stdio process; remote MCP endpoint authentication
does not acquire the credentials used by this connector. The host's existing
secret/auth owner handles issuance, consent, expiry, refresh and the selected
child environment. Guidance neither creates that owner nor checks account access.
This edition includes the prerequisite rows. Earlier configuration-assistance
artifacts contain field help without these rows.

### Google Workspace read setup

Checked against the selected donor calls and official Google documentation on
2026-10-03. `config-help` carries these instructions through the existing
`prerequisites` and `unresolved_details` fields. They guide the agent and host;
they do not acquire credentials, inspect token scopes or enforce a grant.

Use an existing approved OAuth application, or have its operator
[create a Google Cloud project](https://developers.google.com/workspace/guides/create-project)
and [enable the required API](https://developers.google.com/workspace/guides/enable-apis).
Configure [Branding, Audience and Data Access](https://developers.google.com/workspace/guides/configure-oauth-consent):
Internal is for the project's Workspace/Cloud Identity organization; otherwise
use External. Testing needs selected test users and has token-lifetime/use limits.
The application operator must resolve any required verification or applicable
[personal, testing or internal exception](https://support.google.com/cloud/answer/13464323).

Choose [client credentials](https://developers.google.com/workspace/guides/create-credentials)
for the host's actual authorization flow: registered redirects for a web client,
or the supported [desktop loopback flow](https://developers.google.com/identity/protocols/oauth2/native-app)
for a desktop client. The connector does not dictate a callback or implement
login, token exchange, refresh or service-account signing. Its `token_env` remains
an environment-variable reference supplied by the host's existing secret owner.

For managed accounts, [Workspace API controls](https://support.google.com/a/answer/7281227)
can block a client or its requested data despite user consent. Ask the administrator
to review access when policy requires it; broad Trusted access is not automatically
required. This administrator step does not apply to personal Google accounts.

The table lists read-scope options for the actual selected calls, not all scopes
requested by the original multi-action Activepieces pieces. All scope suffixes
below use `https://www.googleapis.com/auth/`.

| Profile | API and selected call | Scope option and important limit |
| --- | --- | --- |
| `google-contacts.search-contact` | People API, [people.searchContacts](https://developers.google.com/people/api/rest/v1/people/searchContacts) | `contacts.readonly`; personal contacts, not the Workspace Directory. The selected action makes one search request; it does not automate the recommended empty-query cache warmup. |
| `google-tasks.get-task` | Tasks API, [tasks.get](https://developers.google.com/workspace/tasks/reference/rest/v1/tasks/get) | `tasks.readonly`; caller supplies task-list and task IDs. |
| `google-calendar.get-event` | Calendar API, [events.get](https://developers.google.com/workspace/calendar/api/v3/reference/events/get) | `calendar.events.readonly`; calendar/event access is still required. `primary` is an alias, not proof of account identity. |
| `google-forms.get-form` | Forms API, [forms.get](https://developers.google.com/workspace/forms/api/reference/rest/v1/forms/get) | `forms.body.readonly`; reads structure, not submitted responses. |
| `google-slides.get-presentation` | Slides API, [presentations.get](https://developers.google.com/workspace/slides/api/reference/rest/v1/presentations/get) | `presentations.readonly`; ID/response field mask does not reduce token permission to one file. |
| `google-docs.get-document` | Docs API, [documents.get](https://developers.google.com/workspace/docs/api/reference/rest/v1/documents/get) | `documents.readonly`; selected call omits `includeTabsContent`, so default first-tab behavior applies. |
| `google-drive.get-file` | Drive API, [files.get](https://developers.google.com/workspace/drive/api/reference/rest/v3/files/get) | `drive.metadata.readonly` for general accessible-file metadata, or `drive.file` only with existing app-specific file authorization. No download or explicit field mask; [method defaults apply](https://developers.google.com/workspace/drive/api/guides/fields-parameter). |
| `google-sheets.get-values` | Sheets API, [spreadsheets.values.get](https://developers.google.com/workspace/sheets/api/reference/rest/v4/spreadsheets.values/get) | `spreadsheets.readonly`; selected A1 range limits the request, not the token grant. No file-discovery call. |
| `gmail.get-thread` | Gmail API, [users.threads.get](https://developers.google.com/workspace/gmail/api/reference/rest/v1/users.threads/get) | `gmail.readonly` for `full` (default); `gmail.metadata` is an alternative only when each call explicitly uses `metadata` or `minimal`. It cannot serve `full`. |

Gmail format is an operation argument, not a persistent restricted profile. Both
[Gmail scope options are restricted](https://developers.google.com/workspace/gmail/api/auth/scopes).
The host must keep later calls consistent with its grant. Sending, composing and
modifying mail need not be authorized for this read.

[Drive distinguishes](https://developers.google.com/workspace/drive/api/guides/api-specific-auth)
restricted `drive.metadata.readonly` from non-sensitive, file-scoped `drive.file`.
The latter permits writes to the app-authorized files as well as reads. A file ID
or ordinary user sharing alone does not authorize that file for the OAuth app;
this connector provides no picker/open authorization flow. Do not substitute it
as a universal read-only grant. For restricted scopes, the application operator
must assess verification and any applicable security assessment for server-side
storage/transmission, including Google's exceptions.

These instructions do not establish the current project's configuration,
verification status, granted scopes, account identity, resource sharing or
organization policy. Broader existing tokens can still carry more authority
than these selected read calls. Cloud Storage IAM is outside this guidance batch.

Select the profile, account and resource explicitly. The commands do not infer
choices, edit configuration, import the MCP or donor runtime, resolve credentials,
install anything or contact a provider.

`check-config` invokes the existing semantic validator and returns one JSON object
with `valid: true` and `scope: "configuration_only"`. Failure has a nonzero exit code
and a generic diagnostic without submitted values. Success does not mean a named
credential exists or that authentication, authorization or readiness is established.
Continue through the existing `describe` and `serve` commands with the host-owned
credential when ready. Building a successor does not promote its artifact into the
supported inventory.

The current source additionally emits `credential_environment_references` after
successful `check-config`. This additive field requires successor artifact
acceptance; existing frozen releases are unchanged. For a selected S3
configuration without an optional session-token reference, the added field is:

```json
{"credential_environment_references":[{"field_path":["token_env"],"environment_variable":"SELECTED_AWS_SECRET"},{"field_path":["access_key_id_env"],"environment_variable":"SELECTED_AWS_ACCESS"}]}
```

Activepieces configuration is flat, so each path contains one literal JSON key.
Rows use only the existing help's credential declarations and the same validated
configuration. Absent optional references are omitted; names keep exact spelling
and case, with one row per selected field even when names alias. No credential
value or ambient environment is read. `profile`, `valid` and
`scope: "configuration_only"` are unchanged. Use `config-help` for credential
meaning and prerequisites. The host freezes the configuration and owns lookup,
secret handling and protected-environment policy; a name such as `PATH` is not
permission to overwrite it. This field provides no account binding, grant,
authentication or readiness assertion.

Create a UTF-8 JSON file such as `tally.json`. It contains an environment
variable name, never the credential itself:

```json
{"profile":"tally.get-form","token_env":"MY_TALLY_TOKEN"}
```

From the package source directory, inspect its tool schema without reading a
token or contacting a provider:

```bash
node dist/mcp-cli.js describe --config ./tally.json
```

Configure your MCP client to run the installed command
`meerkat-toolkit-activepieces-mcp` with arguments
`serve --config /absolute/path/to/tally.json`, and supply `MY_TALLY_TOKEN` in
that child process's environment through the host's secret configuration.
For a source checkout, the equivalent command is:

```bash
node dist/mcp-cli.js serve --config ./tally.json
```

The token must already be valid for the selected service and account. Startup
does not validate it remotely. The first selected business operation establishes whether that
specific request is accepted; starting a process does not establish readiness.
There is no fallback to other environment variables or token tool arguments.
The selected value is snapshotted at startup. Restart with a newly selected
credential to rotate it; no refresh, account switching or credential discovery
occurs inside the server.

Asana and NocoDB configurations are:

```json
{"profile":"asana.get-task","token_env":"MY_ASANA_TOKEN"}
```

```json
{"profile":"nocodb.get-record","token_env":"MY_NOCODB_TOKEN","origin":"https://noco.example.invalid","version":4}
```

Each process exposes exactly one tool, named by replacing the period in its
profile with `_`, for example `baserow_get-row`. Supply the manual IDs in the
table below; Postmark delivery statistics takes no arguments. The configuration
must be a plain JSON object of at most 8192 bytes. Unknown fields are refused.
`token_env` accepts an uppercase letter followed by uppercase letters, digits
or underscores, at most 128 characters. NocoDB requires `origin` and `version`;
Baserow requires `origin` and refuses `version`; Confluence requires `origin` and
`username`; Jotform requires `region`; Okta and Pipedrive require `origin`;
WordPress requires `origin` and `username`; Intercom requires `region`.
The content and read expansions below list their additional configuration fields.
Each profile refuses fields belonging only to other profiles.
Optional `timeout_ms` and
`max_response_bytes` use the same limits and defaults as the reader below.

The seven additional MCP profile names are `baserow.get-row`,
`bitly.get-bitlink-details`, `clockify.find-running-timer`, `coda.get-row`,
`pagerduty.get-incident`, `postmark.get-delivery-stats` and
`typeform.get-form-messages`. Their configuration follows the same explicit
`profile` and `token_env` shape. For example:

```json
{"profile":"baserow.get-row","token_env":"MY_BASEROW_DATABASE_TOKEN","origin":"https://api.baserow.io"}
```

Baserow uses only a database token. Email/password login and JWT acquisition
are unsupported. Typeform and Asana receive an already selected access token;
the host owns any OAuth consent, scope selection, expiry and refresh. A token
environment variable is not an OAuth setup implementation.

The official MCP SDK handles framing and protocol negotiation, including
modern revision 2026-07-28 and legacy initialization with 2024-11-05. Tool
schemas are the selected operation's actual bounded input schemas. Readers advertise
read-only, nondestructive, idempotent and open-world annotations. The four explicit
writers advertise readOnlyHint=false, destructiveHint=true and idempotentHint=false.
There are no background tasks or MCP input requests. Stdout
is reserved for MCP messages during `serve`. Wrapper diagnostics use fixed stderr text.
The unchanged Help Scout helper also logs safe invocation errors and stack traces
to stderr; provider bodies and original transport exception text never reach it.
See the official SDK's [stdio serving contract](https://ts.sdk.modelcontextprotocol.io/v2/serving/stdio.html)
and [legacy-client behavior](https://ts.sdk.modelcontextprotocol.io/v2/serving/legacy-clients.html).

## Embedded MCP ownership

```typescript
import { loadConfiguration, serveMcpStdio } from "@rkat/toolkit-activepieces/mcp";

const configuration = await loadConfiguration("/absolute/path/to/tally.json");
const serving = serveMcpStdio(configuration);
try {
  await serving.closed;
} finally {
  await serving.close();
}
```

`createMcpServer(configuration, {environment?, httpFactory?})` returns a server
for an embedding host to connect to an official SDK transport. Use
`serveMcpStdio` for the SDK's modern/legacy stdio negotiation. Tests can supply an
official SDK transport and a fresh HTTP owner via `httpFactory`; the factory
transfers ownership to that server. No global transport or environment patch
is needed. Calling `server.close()` or awaiting `serving.closed` joins owned
cleanup. Do not share an HTTP owner between independently closed servers.

The default HTTP owner uses a fresh Undici dispatcher per server, normal TLS
certificate verification, no ambient proxy/netrc/account discovery, no redirect
interceptor, and no retry or credential replay. The reader still validates
every exact request URL and header. Normal Node CA and host DNS/egress policies
apply. The server permits at most four simultaneous calls; excess calls receive
a fixed `server_busy` error. It reuses the reader's whole-call cancellation and
response limits, and the default stdio buffer is bounded to 65536 bytes.

The SDK aborts request handlers on transport closure but does not itself await
their completion. This package retains active promises, aborts them on EOF or
shutdown, waits for borrowed response cleanup, and then awaits dispatcher
closure. Its thin transport wrapper delegates all protocol processing to the
SDK and additionally joins server construction and cleanup. CLI SIGINT/SIGTERM
use that same close path. An injected host transport or cleanup callback that
ignores cancellation can still delay shutdown indefinitely; cleanup is joined
rather than abandoned. Forced process termination cannot guarantee cleanup.

MCP failures use `isError: true`, fixed text and safe operation metadata. They
do not expose provider error bodies, token values or upstream exception text.
Successful service data is returned within the documented donor/profile shape and
may contain private business data. Cancellation suppresses the response under
the SDK protocol but is not evidence that no HTTP request reached the service.

## Small host API

```typescript
import { createReader } from "@rkat/toolkit-activepieces";

// Both arguments belong to the calling host's selected connection.
export function createTallyReader(apiKey: string, selectedFetch: typeof fetch) {
  return createReader({
    profile: "tally.get-form",
    auth: { apiKey },
    transport: selectedFetch,
    timeoutMs: 10_000,
    maxResponseBytes: 1_048_576,
  });
}
```

`reader.describe()` returns the supported input schema without contacting a
provider. `reader.invoke(inputs, {signal})` calls the selected donor action once
and returns a detached JSON value. The host chooses whether and when to expose
that operation to a user or agent. Auth values are not part of the tool schema.
The reader snapshots its configuration; create a new reader when the host
changes the selected token, connection or version. No refresh or account
retargeting occurs inside a reader.

| Profile | Explicit auth | Invocation inputs |
| --- | --- | --- |
| `tally.get-form` | `{apiKey}` | `{form_id}` |
| `asana.get-task` | `{accessToken}` | `{task}` |
| `nocodb.get-record` | `{origin, apiToken, version}` | `{baseId, tableId, recordId}` |
| `baserow.get-row` | `{origin, databaseToken}` | `{table_id, row_id}` |
| `bitly.get-bitlink-details` | `{accessToken}` | `{bitlink}`, for example `bit.ly/abc123` |
| `clockify.find-running-timer` | `{apiKey}` | `{workspaceId}` |
| `coda.get-row` | `{apiKey}` | `{docId, tableId, rowIdOrName}` |
| `pagerduty.get-incident` | `{apiKey}` | `{incidentId}` |
| `postmark.get-delivery-stats` | `{serverToken}` | `{}` |
| `typeform.get-form-messages` | `{accessToken}` | `{form_id}` |
| `google-contacts.search-contact` | `{accessToken}` | `{query, readMask, pageSize?}` |
| `figma.get-comments` | `{accessToken}` | `{file_key}` |
| `acuity.find-appointments` | `{accessToken}` | Optional client/date/calendar/type/status/limit/sort filters below |
| `confluence.get-page-content` | `{origin, username, apiToken}` | `{pageId}` |
| `brevo.find-contact` | `{apiKey}` | `{identifier, identifier_type?}` |
| `jotform.get-submission` | `{apiKey, region}` | `{submissionId}` |
| `kit.get-subscriber` | `{apiSecret}` | `{subscriber_id}` |
| `mailer-lite.get-campaign` | `{apiKey}` | `{campaign_id}` |
| `calendly.get-scheduled-event` | `{personalToken}` | `{scheduledEvent}`, an ID or exact provider URI |
| `google-tasks.get-task` | `{accessToken}` | `{tasks_list, task_id}` |
| `campaign-monitor.find-subscriber` | `{apiKey}` | `{listId, email}` |
| `okta.find-user-by-email` | `{origin, apiToken}` | `{email}` |
| `help-scout.find-conversation` | `{accessToken}` | At least one filter described below |
| `airtable.get-record` | `{accessToken}` | `{base_id, table_id_or_name, record_id}` with manual IDs |
| `toggl.find-project` | `{apiToken}` | `{workspace_id, name?, active?, billable?, only_me?, only_templates?, page?, per_page?}` |
| `stripe.get-invoice` | `{apiKey}` | `{invoice_id}` |
| `pipedrive.get-activity` | `{origin, accessToken}` | `{activityId}` |
| `wordpress.get-page` | `{origin, username, applicationPassword}` | `{page_id}` |
| `google-calendar.get-event` | `{accessToken}` | `{calendar_id, event_id, max_attendees?, time_zone?}` |
| `notion.get-page` | `{accessToken}` | `{page_id}` |
| `intercom.get-conversation` | `{accessToken, region}` | `{conversationId}` |

Tally, NocoDB base/table, Clockify workspace, Coda document, PagerDuty incident
and Typeform form IDs accept 1-128 ASCII letters, digits, `_` or `-`. Asana task
GIDs accept 1-64 digits as a string. NocoDB record IDs and both Baserow IDs must
be positive safe integers. Bitly takes a lowercase DNS domain of at most 253
characters, `/`, and a 1-128 character hash using ASCII letters, digits, `_`
or `-`. Supply the domain/hash ID without a URL scheme.

Coda table and row names are nonempty, bounded to 256 Unicode code points and
must be well-formed Unicode without control characters; `.` and `..` are
refused. The donor encodes each name as one path segment. Spaces, slashes,
percent signs and Unicode are preserved through that encoding. The Coda read
fixes `useColumnNames=true` and `valueFormat=simpleWithArrays`; Baserow fixes
`user_field_names=true`. These are supported profile constraints, not a claim
about every ID accepted by the upstream services. Extra input/config fields
are refused, and no dropdown or auth-validation callback runs.

NocoDB `version` is explicitly 3 or 4, matching the donor's v2-table or v3-base
endpoint selection. `origin` must exactly equal a canonical HTTPS origin, such
as `https://noco.example.invalid` or `https://noco.example.invalid:8443`, without
userinfo, path, trailing slash, query or fragment. This first profile does not
support HTTP-only or path-prefixed NocoDB installations. Baserow uses the same
canonical HTTPS-origin rule, with no implied default. Confluence uses the same
origin rule. Okta also requires a selected canonical HTTPS origin, including
when using a custom organization domain. Pipedrive and WordPress use that same
explicit origin rule. Jotform and Intercom each select their documented regional origins. Other origins
are fixed to their reviewed upstream endpoints. Tokens are nonempty printable
ASCII without spaces/control characters, at most 4096 characters. WordPress
application passwords additionally accept spaces, preserved exactly inside
the Basic credential. An all-whitespace password is refused.

`timeoutMs` defaults to 10000 and accepts integers 1-60000. It covers the whole
invocation, including response reading and all Algolia pages. `maxResponseBytes` defaults to 1048576
and accepts integers 1-4194304. Response bytes, JSON depth/node count and the
returned JSON value are bounded. Byte limits are cumulative across the invocation.
Inputs are validated before donor entry.

## Airtable, Toggl Track, Stripe, Pipedrive, WordPress, Google Calendar, Notion and Intercom

These source profiles each execute one selected read. Their readiness for a
host remains tied to the separately accepted artifact and support record.
No account acquisition or live provider acceptance is implied by fixture tests.

Airtable requires manual base, table and record IDs starting with `app`, `tbl`
and `rec`, followed by ASCII letters/digits, at most 128 characters total.
The donor's table-name interpolation is not URL-encoded, so this profile
intentionally accepts table IDs only. It returns the actual record and field
values for HTTP 200; another successful status preserves the donor's
`{status,headers,body}` wrapper, with response headers omitted by the shared
seam. In either case the response body must be a record whose `id` equals the
requested ID. An empty 204 response is invalid.

Toggl Track lists one project page within a positive safe-integer workspace ID.
The API token is the Basic username with password `api_token`; a colon in the
token is refused. The upstream action expects the raw auth string, which this
profile provides. `name` may be absent, null or empty, all of which omit that
filter. Nonempty names, including whitespace, are sent unchanged and are
bounded to 256 Unicode code points. `active` may be `true`, `false` or `both`
as strings; omission leaves the provider's default unchanged. Optional
`billable`, `only_me` and `only_templates` preserve explicit boolean false.
Those options do not accept null. Toolkit supplies `page=1` and `per_page=100`
when omitted and permits pages 1-10000 and page sizes 1-200. No pagination
loop runs. Results must be an array within the chosen page size, with positive
safe-integer project IDs and the selected `workspace_id`. An empty array is a
valid empty result.

Stripe reads one invoice by an `in_` identifier followed by ASCII
letters/digits, at most 128 characters total. This operation uses the actual
Activepieces common-HTTP action; it does not create, finalize, pay or send an
invoice. Pipedrive reads one activity by positive safe-integer `activityId`,
including its attendees through the donor's `include_fields=attendees` query.
It requires the chosen connection's API origin and preexisting OAuth access
token. The returned envelope must have `success:true` and a record-valued
`data` with the selected ID. Neither profile asks for mutation-only fields.

WordPress uses an explicitly selected canonical HTTPS origin, username and
application password, and reads one positive safe-integer `page_id` under
`/wp-json/wp/v2/pages`. Subdirectory installations are outside this profile.
It preserves the donor's default REST view; the donor description's broad
draft-access claim is not part of this supported contract. Returned page IDs
must match. Rendered content is returned as service data without interpretation.

Google Calendar uses an already selected OAuth access token; service-account
JWT signing, account discovery and refresh are not invoked. `calendar_id` is a
nonblank, well-formed Unicode string of at most 256 code points, trimmed and
encoded as one path segment; `primary` is accepted. Dot segments are refused.
`event_id` accepts 5-1024 ASCII letters/digits, `_` and `-`. Optional
`max_attendees` accepts 1-200. `time_zone` may be omitted, null, empty or
whitespace to use the provider default; otherwise it is trimmed and must be
a valid zone recognized by the Node runtime. The JSON schema expresses its
lexical bound, and runtime validation additionally checks the zone name.
Event date/time fields are not converted. Returned event IDs must match.

Notion executes the pinned real `@notionhq/client` 2.2.14 `pages.retrieve`
method with the donor's `Notion-Version: 2022-02-22`. Its internal integration
token is passed through the donor's custom-auth path. `page_id` is a lowercase,
hyphenated UUID. This returns page metadata and properties, including database
rows represented as pages; it does not read body blocks. Intercom executes the
real `intercom-client` 6.2.0 `conversations.find` method with API version 2.11.
Its configuration requires `region` equal to `us`, `eu` or `au`; the origins
are `api.intercom.io`, `api.eu.intercom.io` and `api.au.intercom.io` respectively.
`conversationId` is a string of 1-64 decimal digits, preserving large IDs.
Both SDK profiles require the returned ID to equal the selected ID and retain
other bounded JSON fields without claiming full provider-schema validation.

Example nonsecret configurations:

```json
{"profile":"pipedrive.get-activity","token_env":"PIPEDRIVE_TOKEN","origin":"https://selected-company.pipedrive.com"}
```

```json
{"profile":"wordpress.get-page","token_env":"WORDPRESS_APP_PASSWORD","origin":"https://site.example.invalid","username":"reader"}
```

```json
{"profile":"intercom.get-conversation","token_env":"INTERCOM_TOKEN","region":"eu"}
```

The other five use only `profile`, `token_env` and optional limits. Notion and
Intercom use their SDKs' supported fetch/fetcher injection points. Toolkit
owns the exact GET, selected account, deadline, redirect refusal, bounded JSON
decoding and response cleanup. Intercom's built-in retries and 20-second fetch
timeout are replaced by these common whole-call limits. Notion's 60-second
SDK timer remains; an early SDK rejection cannot abandon the request. The
reader joins any still-running HTTP promise and body cancellation, and its
MCP concurrency slot remains occupied until that cleanup retires. The Notion
SDK logger is suppressed; fixed Toolkit error metadata is the diagnostic
boundary. Successful private data is still returned normally.

The retained vendor source includes inactive mutation/trigger files imported
by upstream indexes. The build projects only exact auth/helper statements from
four named files, excluding the unselected piece registries. Selected action
run bodies and framework constructors are unchanged. Real bundled SDKs also
contain inactive fallback transports, auth methods and other operations; those
are not advertised or selected. This is a bounded adapter, not a JavaScript
sandbox. Tests verify the selected routes make no ambient fetch, HTTP or process
calls and that unselected piece action/trigger modules are absent from the
compiled import closure. `licenses/BUNDLED-PACKAGES.json` lists the measured
dependency contributions and retained notices.

## Google Tasks, Campaign Monitor, Okta and Help Scout profiles

These four source profiles make one read request with a preexisting selected
credential. Installed artifact acceptance remains separate from this source
expansion. Google Tasks and Help Scout use OAuth access tokens; consent and
refresh remain owned by the host. Campaign Monitor uses its API key as the
Basic username with password `x`, so a key containing `:` is refused. Okta uses
its selected API token in the `SSWS` header and requires an explicit site:

```json
{"profile":"okta.find-user-by-email","token_env":"OKTA_API_TOKEN","origin":"https://my-org.okta.com"}
```

The other three configurations contain only their `profile`, `token_env` and
optional limits. Google Tasks `tasks_list` and `task_id` each accept 1-1024 ASCII
letters, digits, `_`, `-` and `=`. Supply actual IDs; list discovery and the
special `@default` alias are outside this profile. The unchanged action returns
one task record, including title, notes, status, due date and links when present.
Toolkit requires the returned `id` to equal the requested task ID; optional
fields are preserved without a second full provider schema.

Campaign Monitor takes a selected `listId` under the 1-128 ASCII ID rule and an
email address. The client ID is setup-dropdown context only and is omitted from
the invocation schema. The actual helper reads the subscriber and the donor
returns `{found:true,result:...}`, converting repeated custom-field keys into
arrays. Its original truthiness behavior is retained: an empty field value
followed by another value is replaced by that later value. A success response
must match the requested email ignoring ASCII case and contain at most 100
string-valued custom fields. Keys inherited from `Object.prototype` are refused
before the donor's plain-object transform; they cannot be silently lost or
converted into a false miss. Only an HTTP 404 returns
`{found:false,result:null}`, after body cleanup. Authentication, rate-limit,
transport, cancellation, malformed-result and cleanup failures remain errors
even though the unchanged action catches all exceptions.

Okta queries `profile.email eq "<email>"`. Email inputs in these profiles accept
ASCII dot-atom local parts and DNS domains, at most 254 characters. Quoted,
escaped and internationalized addresses are outside this bounded profile.
Quotes and backslashes therefore cannot become SCIM search syntax. The transport
checks the exact URL assembled by the unchanged donor, then serializes its
selected query value with URLSearchParams; literal `+`, `&`, `#` and other
permitted email characters remain data. Campaign Monitor uses the same checked
inline-query path. Neither route accepts a caller-supplied URL or extra query.

Okta returns its donor's first matching user, or its explicit no-user object for
an empty array. A non-array response, malformed user, email mismatch or more than
200 returned users is an error. Multiple valid matches preserve the first-match
behavior; all returned email values must match the selected address ignoring
ASCII case. This is not a uniqueness guarantee or a full directory enumeration.

Help Scout accepts optional `subject`, `email`, `mailboxId`, `status`, `assignTo`,
`tags` and `query`, with at least one real filter. `subject` is a nonblank literal
of at most 256 Unicode code points, without controls, unpaired surrogates,
quotes or backslashes. `email` follows the same scoped email syntax above.
`mailboxId` and `assignTo` are positive decimal strings of at most 64 digits,
without leading zeroes. `status` is one of `active`, `open`, `closed`, `pending`,
`spam` or `all`. `tags` contains 1-20 unique ASCII IDs under the 1-128 character
rule, so a tag cannot contain a comma that changes the donor's list syntax.

The separately named `query` is an explicit Help Scout search expression, not a
literal subject. It accepts nonblank well-formed text of at most 256 code points
without controls; quotes and operators are intentionally allowed here. The real
action combines that expression and structured subject/email filters with AND.
It returns `{found:true,data:<first conversation>}` or `{found:false,data:{}}`.
The response must contain the conversations array with at most 200 records.
This checks the envelope and record shape, not every conversation field.
Only the first response page is read; no next-page URL is followed. Okta and
Help Scout row ceilings reject oversized responses after the byte-bounded body
read, and do not claim provider-side pagination or total-result limits.

## Kit, MailerLite and Calendly profiles

All three profiles use a preexisting selected credential and make one GET.
Their configuration needs only `profile` and `token_env` (plus optional limits).
Their upstream origins are fixed; there is no alternate account or endpoint
fallback.

```json
{"profile":"kit.get-subscriber","token_env":"KIT_V3_API_SECRET"}
```

Kit reuses the upstream ConvertKit piece's V3 action. Supply the selected V3
API secret, not a V4 API key. `subscriber_id` is a string of 1-64 digits. The
action reads `api.convertkit.com/v3/subscribers/<id>` and returns the
`subscriber` record, including email, name, state and custom fields. The V3
protocol sends the secret as `api_secret` in the query. The trusted host
transport therefore receives a credential-bearing URL and must treat it as
secret data. Toolkit error results and MCP diagnostics omit that URL, provider
error text and underlying exceptions. Tool arguments cannot supply or replace
the query secret. Redirects and automatic retries remain disabled.

```json
{"profile":"mailer-lite.get-campaign","token_env":"MAILERLITE_API_KEY"}
```

MailerLite uses the selected API key as a Bearer token at
`connect.mailerlite.com/api/campaigns/<id>`. `campaign_id` accepts 1-128 ASCII
letters, digits, `_` or `-`. The unchanged action extracts the campaign's `data`
record. Toolkit requires that envelope before donor extraction, so a missing
or malformed `data` field cannot become an empty successful fallback. This
profile does not send campaigns or enumerate subscribers.

```json
{"profile":"calendly.get-scheduled-event","token_env":"CALENDLY_PERSONAL_TOKEN"}
```

Calendly uses a selected personal token with permission to read scheduled
events. `scheduledEvent` accepts either a 1-128 character event ID using ASCII
letters, digits, `_` or `-`, or exactly
`https://api.calendly.com/scheduled_events/<id>`. A supplied URI must use that
exact host and path, without userinfo, a port, query, fragment, percent encoding
or trailing slash. Toolkit validates the URI before the unchanged helper
extracts its last segment. The action returns the `resource` record from one
scheduled event. It does not look up invitees or create scheduling links.

All three profiles require record-valued provider envelopes and preserve their
contents, including false values, zeroes and nulls. They use the same bounded
response, cancellation and cleanup ownership as every other reader. These
source profiles still require their own reviewed installed-artifact evidence
before adoption promotion.

## Confluence, Brevo and Jotform profiles

These selected reads reuse unchanged donor actions with preexisting credentials.
For Confluence Cloud, supply the selected site origin, account username/email
and API token with permission to read the page. The profile returns one page's
storage-format content. Descendant traversal is disabled and cannot be enabled
by tool arguments. `pageId` accepts 1-64 digits as a string. The username accepts
1-254 printable ASCII characters without spaces or colon.

```json
{"profile":"confluence.get-page-content","token_env":"CONFLUENCE_TOKEN","origin":"https://my-site.atlassian.net","username":"reader@example.invalid"}
```

Brevo uses a selected API key with contact-read permission. `identifier` accepts
1-256 Unicode code points, without control characters, unpaired surrogates, `.`
or `..`, and is encoded as one path segment. `identifier_type` defaults to
`email_id`; the schema also permits `phone_id`, `contact_id`, `ext_id`,
`whatsapp_id` and `landline_number_id`. A successful lookup returns the donor's
`{found:true,data:...}` wrapper. HTTP 404 alone returns `{found:false,data:{}}`
after the response body has closed. Authentication, rate-limit and service
failures remain errors. Cancellation during that cleanup remains cancellation.

```json
{"profile":"brevo.find-contact","token_env":"BREVO_TOKEN"}
```

Jotform returns one submission's answers and metadata. `submissionId` accepts
1-64 digits as a string, preserving long identifiers without numeric rounding.
Supply a Jotform API key that can read that submission, and explicitly choose
`us`, `eu` or `hipaa` to match the account's region. These select
`api.jotform.com`, `eu-api.jotform.com` or `hipaa-api.jotform.com`; there is no
regional fallback. The adapter checks the response-code envelope before the
donor extracts `content`. A region label does not establish regulatory compliance.

```json
{"profile":"jotform.get-submission","token_env":"JOTFORM_TOKEN","region":"eu"}
```

## Google Contacts, Figma and Acuity profiles

These profiles accept an already selected OAuth access token through the same
host-owned `token_env` reference. They do not perform login, refresh or account
selection. The token must permit the selected contact search, file-comment read
or scheduling-account read, and still be valid when called. Use `profile` values
`google-contacts.search-contact`, `figma.get-comments` or
`acuity.find-appointments`; no `origin` or `version` is accepted.

Google Contacts requires `query` and a nonempty unique `readMask` list. The
schema lists the supported People API field names, such as `names` and
`emailAddresses`. `pageSize` defaults to 30 and accepts 1-30. The query accepts 1-256
Unicode code points without control characters or unpaired surrogates. This
operation performs one search request and returns the donor's response; it
does not implement cache warming or follow-up enumeration.

Figma accepts an ASCII `file_key` under the same 1-128 character ID rule. Its real
helper returns `{success:true,response_body:...}` plus an undefined
`request_body` placeholder for GET. Toolkit omits only that non-JSON placeholder,
preserving the response body and success wrapper. File comments remain service
data, not a frontend route to an agent.

Acuity accepts optional `firstName`, `lastName`, `email` and `phone` strings under
the same 1-256 code-point rule; positive safe-integer `calendarID` and
`appointmentTypeID`; and `minDate`/`maxDate` as valid `YYYY-MM-DD` calendar dates.
Date-times and reversed ranges are refused. Dates are sent unchanged, without a
timezone conversion. Supply plain phone/filter values; the shared URL encoder
encodes them once. `status` is `scheduled` (default), `canceled` or `all`;
`direction` is `DESC` (default) or `ASC`; `maxResults` is 1-100, default 100. Omitting
filters requests the first bounded appointment list. There is no implicit
complete traversal. The result is `{found:boolean,data:[...]}`. Malformed
appointment arrays or a response exceeding the selected row limit are refused.

The actual unchanged donor actions construct their own request and result.
Toolkit validates their exact selected URL, credentials and query against the
already validated call, then reuses the existing transport and cleanup owner.
No dynamic dropdown or OAuth metadata callback is invoked. These three profiles
add no npm dependency. New source behavior requires its own installed artifact
acceptance before it replaces an earlier supported archive.

## Transport and cleanup contract

The borrowed `transport(url, init)` must obey the exact supplied URL, method,
headers, abort signal and `redirect: "error"`. It must not silently retry,
replay credentials, follow redirects or retarget a request. Use normal TLS
certificate verification. The adapter does not modify environment variables,
TLS settings or global fetch behavior. The host remains responsible for proxy,
CA, DNS/egress policy and the selected account's authority.

The transport transfers an unlocked, unread standard `Response` body to the
invocation. The adapter consumes it or awaits cancellation, then releases its
reader. It also does this for HTTP errors, refused redirects, malformed or
oversized bodies and cancellation after headers. Redirect responses and an
unexpected final response URL are refused. No raw error response body is
forwarded to the caller.

Successful responses with Content-Length must carry one decimal length and no
simultaneous Transfer-Encoding. Duplicate/list-valued, malformed or ambiguous
lengths are refused. When Content-Encoding is absent or contains only identity,
the streamed byte count must equal that length: an overshoot fails immediately,
and an EOF shortfall fails before result conversion. The same existing owner
awaits body cancellation and releases the reader on a framing failure.

A borrowed transport may return decoded compressed content while preserving its
original Content-Encoding and wire Content-Length. That wire length is not
compared with decoded bytes or used as the decoded-body budget. The provided
body's actual bytes remain cumulatively bounded. The host must supply the
selected JSON/XML representation; this does not add a decompressor. Responses
without Content-Length, including already-dechunked chunked transfers, remain
supported. Contradictory framing after a write request retains uncertain effect.

Cancellation before invocation refuses donor entry. After dispatch, the adapter
signals the host, joins its actual completion and cleans up a late response.
It does not abandon an in-flight transport or body-cancellation promise. Thus a
host transport or stream that ignores cancellation can delay settlement past
the configured deadline. The deadline requests cancellation; it cannot forcibly
stop arbitrary host code. A locked response body is a host contract violation
and is rejected without taking ownership away from its existing reader.

Independent calls use independent factory closures and abort controllers. The
adapter adds no process-global current account or mutable transport slot. The
borrowed host transport must support whichever concurrency the host permits.

## Results and errors

The actions retain their operation-specific donor values: Asana and MailerLite
extract `data`, Kit extracts `subscriber`, Calendly extracts `resource`, and
PagerDuty returns `body.incident ?? body`. Other projections and their bounded
checks are described with the relevant profiles above. No generic result
projection replaces those semantics.
Raw-object profiles preserve unknown fields, false values and nulls. Donor
projections and SDK model deserialization can select fields; HubSpot also
normalizes Date values to ISO strings as documented below. This
is an operation result, not an account-binding, task-completion or adoption
receipt.

`ActivepiecesError` exposes fixed `kind`, `entered`, `dispatched`, optional
HTTP `status`, `effect` and `writeOutcome`. Readers use `effect: "read"` and
`writeOutcome: "not_applicable"`. Explicit writers use the before/after-dispatch
outcomes described below. Errors retain no raw provider body, credential,
upstream exception or cause. HTTP 401/403/429 and other error statuses remain
failures without replay. A result lost to cancellation is not proof that no
request reached the provider.

## Evidence and limits

Upstream revision: `e3e1cbebb6dafbf2bb0da35effc6dfd58b8c13e3`.
`vendor/SOURCE-MANIFEST.json` records exact files and immutable URLs.
`dist/BUILD-MANIFEST.json` records the generated artifact and input contributions.
The build preserves vendor files, selects the six documented auth/helper
statement projections, injects owned transports into the real SDKs, applies
the explicit Supabase admin constructor and Webflow v2 route ports, substitutes
the common HTTP seam, and wraps the output in a
per-invocation factory. Selected action run bodies and framework constructors
are unchanged. The projected executable input is not a byte-identical copy of
every retained upstream module.

Tests call the actual donor action bodies using host-injected protocol fixtures.
They exercise request/auth mapping, input refusal, overlapping accounts,
redirects, HTTP errors, redaction, malformed/oversized bodies, cancellation before
and after headers, delayed/rejected cleanup, donor request mismatches and
whole-call deadline signaling. MCP tests additionally exercise both protocol
eras for the selected actual donor bodies through the owned HTTP transport,
explicit credential isolation, concurrency caps, startup/EOF failure paths and
raw subprocess stdio cleanup. They make
no provider calls. A successful source test is not installed-artifact evidence,
OAuth setup, native host readiness or proof of an actual customer task.

An earlier three-action factory-only measurement on Node 25.9.0, macOS arm64, used 10 warmup
calls followed by 100 measured calls. The first construction took 36.2 ms;
the measured median was 7.1 ms, p95 18.2 ms and mean 8.6 ms. This excludes module
import, donor execution and transport. It measured that earlier 389489-byte
bundle, not this expanded bundle, a latency guarantee or Node 22 execution proof. Framework and Zod
construction repeat per invocation to retain independent transport closures.


## Twelve content and CRM source profiles

These profiles require existing selected credentials. The source tests do not
establish provider login, refreshed OAuth access, or installed-artifact adoption.
Each action performs one bounded GET through the same owned transport. The
following table gives additional nonsecret configuration and tool arguments.
`token_env` always names the selected credential environment variable.

| Profile | Credential and extra configuration | Required tool arguments | Supported result |
| --- | --- | --- | --- |
| `freshdesk.get-contact` | API key; explicit HTTPS `origin` | positive safe integer `contactid` | One contact; returned numeric ID must match |
| `google-forms.get-form` | OAuth access token | `form_id` | Form definition; returned `formId` must match |
| `google-slides.get-presentation` | OAuth access token | `presentation_id` | Presentation, or requested field mask |
| `onedrive.get-item` | OAuth access token; `cloud`: `commercial` or `government` | exactly one `itemId` or `path` | Donor's file/folder metadata projection, without file contents |
| `google-docs.get-document` | OAuth access token | `documentId` | Document including available content; returned ID must match |
| `google-drive.get-file` | OAuth access token | `file_id` | Drive's default metadata response with `supportsAllDrives=true`, not downloaded contents |
| `contentful.get-entry` | Management access token; `space` and `environment` | `entityId` | Entry fields in the selected space/environment, with matching `sys.id` |
| `outlook.get-message` | OAuth access token; `cloud`; optional explicit `mailbox` | `messageId` | Message with text-body preference and matching ID |
| `hubspot.get-contact` | OAuth access token | numeric string `contactId` | Real SDK contact model with donor default properties |
| `supabase.get-user` | Service role key; explicit HTTPS `origin` | lowercase UUID `userId` | Selected user's Auth record, using actual admin SDK |
| `sendgrid.find-list-by-name` | API key; `region`: `us` or `eu` | nonblank `name` | All exact-name matches only when the catalog fits one complete page |
| `webflow.get-collection-item` | OAuth access token | lowercase 24-hex `collection_id`, `collection_item_id` | Webflow v2 item, including `fieldData`, with matching ID |

For example, a Contentful connection file is:

```json
{"profile":"contentful.get-entry","token_env":"CONTENTFUL_MANAGEMENT_TOKEN","space":"selected_space","environment":"master"}
```

Configuration is distinct from tool input. No tool accepts a token, credential
origin, cloud, mailbox, Contentful space or environment override. Selecting an
Outlook mailbox determines the addressed mailbox; it does not prove a token's
application/delegated permissions. Freshdesk converts the selected API key to
Basic `apiKey:X` for the unchanged donor's Authorization-header slot.

Forms, Docs, Drive and Contentful IDs are 1-128 ASCII letters, digits, hyphens or
underscores. Slides accepts a 10-128 character ID or canonical
`https://docs.google.com/presentation/d/ID/edit` URL. Optional `fields` may be
absent, null, empty or whitespace, all meaning the full response. A nonempty
field mask is trimmed and sent unchanged from the donor; a masked response need
not contain `presentationId`, but one that does must match. The bounded mask
alphabet is ASCII letters, digits, underscores, comma, star, parentheses, slash
and space, up to 1024 characters. It is not a semantic validator of Google's
mask language.

OneDrive trims the selected ID/path. `itemId: "root"` reads the drive root.
Paths are relative to root, can have outer slashes, and must contain no empty,
dot or parent segments. Each component is encoded literally. Outlook trims
and encodes its message ID and optional selected mailbox. These text values
allow well-formed Unicode without ASCII control characters, up to 256 Unicode
code points. Absent/null/empty IDs are invalid. The OneDrive donor converts
missing optional metadata to null and preserves zero/false values.

HubSpot `contactId` is a 1-64 digit string, preserving large decimal identities.
Optional `additionalPropertiesToRetrieve` accepts absent, null or an empty
array as no extra properties. A nonempty array has at most 100 unique property
names (lowercase letter first, then lowercase letters, digits or underscores,
up to 128 characters). The unchanged helper supplies its 26 default contact
properties. Its real SDK selects known model fields, discards unknown model
fields, and converts dates; Toolkit converts valid Date fields to ISO strings
for JSON output. This is a declared result projection, not raw JSON fidelity.

Supabase's selected constructor is explicitly ported to return only
`auth.admin`, backed by the real public `GoTrueAdminApi` from auth-js 2.69.1.
The original action still calls `getUserById` and retains its user projection
and null defaults. The full Supabase client, session initialization, persistence,
refresh, realtime and auth listeners are not constructed. Bare-user and
`{user: ...}` response forms follow the real SDK transform. Missing or
mismatched ID/creation-time shape fails. On transport failure, auth-js may write
a fixed sanitized error and stack to stderr; it receives no original exception,
URL, provider body or credential in that diagnostic.

SendGrid trims and case-folds names as its actual donor does, and returns all
matches including duplicate names. The read requires a valid `result` array of
at most 1000 lists with string IDs/names, and no nonempty `_metadata.next`.
A paginated response fails with `response_too_large` before following any next
URL. It never returns a partial `found:false` or describes the first page as
complete. Empty complete catalogs correctly return `found:false` and `lists:[]`.

The pinned Webflow donor originally uses deprecated v1 routing. A small explicit
subclass port changes only its selected item read to
`/v2/collections/{collection_id}/items/{collection_item_id}`; the real action and
inherited authenticated request helper remain. Output is raw bounded v2 item
JSON, not a claim of unchanged v1 output compatibility. The donor's unused
`site_id` dropdown is not required in this read-only tool schema.

Google Docs/Drive, Contentful, Outlook and HubSpot execute their real pinned SDK
resource methods and response handling through supported per-instance transport
injection. Retry, redirect, refresh and background-throttle paths are bypassed
or disabled at those injection points. The adapter still owns the actual host
request, body, deadline and cancellation join. A slow or cancellation-ignoring
host can delay settlement; an expired awaiter is not retired work. Remaining
SDK fallback code in the bundle is inactive and is not an exposed capability.

## Jira Cloud, Dropbox, Sheets, Excel, Teams, QuickBooks, Xero and Algolia

These eight source profiles add selected business reads. Installed adoption is
artifact-specific; adding source does not promote an unreviewed successor.
Only `@googleapis/sheets` 13.0.2 is a new SDK pin. It uses the existing actual
Google OAuth client with an owned Gaxios adapter. Excel and Teams use the
existing actual Graph client with per-client authentication and terminal
middleware. No token refresh, auth validation callback or ambient credential
lookup runs.

Each MCP configuration includes `profile` and `token_env`, plus only the fields
listed below. Reader `auth` uses the corresponding camelCase names.

| Profile | Additional MCP configuration / reader auth | Tool arguments |
| --- | --- | --- |
| `jira-cloud.get-issue` | `origin`, `username` / `{origin, username, apiToken}` | `issueId`, `expand?` |
| `dropbox.get-file-metadata` | None / `{accessToken}` | `path`, `include_deleted?`, `include_media_info?`, `include_has_explicit_shared_members?` |
| `google-sheets.get-values` | None / `{accessToken}` | `spreadsheet_id`, `worksheet_title`, `range`, `major_dimension?`, `value_render_option?` |
| `microsoft-excel-365.get-table-columns` | `cloud` / `{accessToken, cloud}` | `storageSource`, `workbookId`, `worksheetId`, `tableId`, `siteId?`, `documentId?`, `limit?` |
| `microsoft-teams.get-channel-message` | `cloud` / `{accessToken, cloud}` | `teamId`, `channelId`, `messageId`, `replyId?` |
| `quickbooks.find-invoice` | `company_id` / `{accessToken, companyId}` | `invoice_number` |
| `xero.find-invoice` | `tenant_id` / `{accessToken, tenantId}` | `value`, `type_filter?`, `summary_only?`, `page?` |
| `algolia.browse-records` | `application_id`, `index_name` / `{apiKey, applicationId, indexName}` | `limit?` |

All credentials are already acquired, selected account credentials. Jira uses a
canonical HTTPS instance origin and Basic authentication with the selected
username/API token. This profile targets Jira Cloud's v3 issue API, not Jira
Data Center. `issueId` is a positive numeric ID or uppercase issue key. `expand`
is absent/null or a unique array of up to six values: `renderedFields`, `names`,
`schema`, `transitions`, `editmeta`, `changelog`. Empty array sends an empty
expansion; absent/null omits it. The donor's lossy name/transition maps are
fixed false. The returned issue ID or key must match the selected input and
`fields` must be an object.

Dropbox metadata is an exact read-only POST to `/2/files/get_metadata`.
`path` accepts a literal absolute provider path, an `id:` identifier or a `rev:`
hexadecimal revision. It is JSON data, including Unicode and URL punctuation.
The three flags accept boolean/null; absent/null means false. File/folder
responses require identity and name; an ID/revision selector must match.
Deleted metadata is accepted only with `include_deleted=true`. This operation
returns metadata, not file bytes or recursive directory contents.

Sheets requires an explicit finite uppercase A1 cell or rectangle, such as
`B2:D4`. Ordered endpoints, at most 1,000 rows, 100 columns and 10,000 cells,
rows up to 1,000,000 and columns through ZZZ are enforced before donor entry.
The public schema checks lexical A1 syntax; rectangle geometry is a documented
runtime semantic check. Empty, absent and cross-sheet ranges are refused.
`worksheet_title` is a literal tab title, at most 100 Unicode code points;
Toolkit quotes it and doubles embedded apostrophes before the unchanged donor
concatenates the A1 range. It therefore remains a tab title when it contains
spaces, apostrophes, `!`, or looks like a cell reference. `major_dimension`
defaults to `ROWS` and also accepts `COLUMNS`; `value_render_option` defaults to
`FORMATTED_VALUE` and also accepts `UNFORMATTED_VALUE` or `FORMULA`. Explicit
null/empty enum values are refused. Returned range/title/dimension and geometry
are bound to the selection. Ragged arrays and scalar zero/false/empty-string
values are retained; absent values become `[]` through the real donor action.

Excel and Teams require `cloud` to be `commercial` or `government`, selecting
`graph.microsoft.com` or `graph.microsoft.us`. Manual resource identifiers are
encoded as individual literal path segments before unchanged donor helpers
interpolate them. They accept nonblank well-formed Unicode, at most 256 code
points, without control characters or the standalone `.`/`..` segments.
Excel `storageSource` is `onedrive` or `sharepoint`; only SharePoint requires
and accepts `siteId` and `documentId`. `limit` is an integer 1-100, default 100;
null is refused. The action returns column names. A nonempty or malformed
`@odata.nextLink` is refused before the donor can discard it, so an incomplete
page is not presented as a complete list. Teams reads one selected message or
reply and verifies its returned ID. `replyId` absent/null/empty means the
parent message. The real Teams retry helper is composed with `maxRetries=0`;
429 and other errors return without replay or helper backoff. This is not a
Teams frontend, router or message-send implementation.

QuickBooks selects a numeric company ID in configuration and one literal
invoice number in tool arguments. The real helper performs its quoted equality
query with `MAXRESULTS 1` and minor version 75. Its existing apostrophe escape
is retained; backslashes in invoice numbers are refused because the unchanged
helper cannot safely compose every such literal. It returns the donor's
`{found, result}` shape. Faults, malformed envelopes, multiple rows and a
mismatched returned invoice number are errors, not misses. A valid empty
`QueryResponse` or empty invoice array produces `{found:false,result:{}}`.
Optional `maxResults` must equal the returned zero/one row count; optional
`totalCount` must be zero for an empty result and positive for a returned row.
A larger positive total does not imply that this first-match query is complete.
This read is neither a financial mutation nor a complete accounting export.

Xero selects a tenant UUID outside tool input. Only its `SEARCH_TERM` branch is
exposed: `value` is a nonblank literal provider SearchTerm, not a raw `where`
expression. Optional `type_filter` is `ACCREC`/`ACCPAY`; absent/null/empty omits
it. `summary_only` absent defaults true; explicit null/false omits the provider
summary flag. `page` is 1-10,000, absent/null defaults 1. The unchanged action
returns the provider envelope with up to 100 invoices from that selected page;
this is not a complete search/export. Invoice IDs must be UUIDs and any selected
type filter must match returned records. Provider error envelopes are refused.

Algolia selects the application ID and index in configuration. Its only POST
route is that exact index's `/browse`; the first body is `{}`, and later bodies
may contain only a cursor returned by the previous accepted page. The real
donor loop and final slice are retained. `limit` is 1-1,000, absent/null defaults
to 100 (a deliberately smaller host profile default than the donor's 10,000).
Each page has at most 1,000 object-ID-bearing hits. Cursors are bounded to 4,096
Unicode code points; cycles and more than ten required requests are errors.
Byte and time limits cover the whole loop, including empty pages. Hitting a
resource limit returns an error, not partial success. Reaching the selected
record limit legitimately returns the donor's first `limit` records. Browsing
is neither relevance-ranked search nor a guarantee of a complete index export.

Read effects are declared by selected operation, not inferred from HTTP verbs.
These POST readers admit exact URL, credentials, query and body contracts;
other routes, extra fields and mutation bodies are refused before dispatch.
The same invocation retains each pending request and borrowed response through
cancellation/cleanup, and never leaves a timed-out SDK waiter owning unjoined
HTTP work. As before, a host transport that ignores abort can delay retirement;
the wrapper waits for actual completion rather than claiming an early close.

## Four SDK-backed source profiles

These profiles keep the real pinned action bodies and SDK/helper operation
methods. Each makes one bounded read request. Their source status is separate
from installed-artifact acceptance and inventory adoption.

| Profile | Selected configuration, besides `token_env` | Tool arguments | Useful result |
| --- | --- | --- | --- |
| `ghost.get-post` | HTTPS `origin`; token is an Admin API key | `post_id`, exactly 24 hexadecimal characters | Selected post with HTML, tags and authors; donor omits lexical/mobiledoc fields |
| `monday.get-items` | Token is the selected Monday API token | `item_ids`, 1-100 unique positive decimal strings | Selected items and column values, plus count |
| `mailchimp.get-campaign-report` | `data_center`, such as `us1`; token is an existing OAuth access token | `campaign_id` | Campaign report, summary and performance metrics |
| `amazon-s3.list-files` | `aws_region`, `bucket`, `access_key_id_env`; optional `session_token_env`; `token_env` names the secret access key | optional `prefix` and `maxKeys` | One page of object metadata, truncation flag and continuation metadata |

Example S3 MCP configuration contains names, not secret values:

```json
{"profile":"amazon-s3.list-files","token_env":"S3_SECRET_ACCESS_KEY","access_key_id_env":"S3_ACCESS_KEY_ID","session_token_env":"S3_SESSION_TOKEN","aws_region":"eu-north-1","bucket":"selected-bucket"}
```

The host must supply every named variable. Omitting `session_token_env` selects
static credentials without a session token; naming it requires a nonempty value.
All credential values are snapshotted at server construction. No profile reads
an unnamed environment credential or performs OAuth, OIDC, STS, refresh or account
discovery. The S3 host must select a region and a general-purpose bucket permitted
by its existing credentials. Custom endpoints, access point ARNs, directory
buckets and object downloads are outside this profile.

Ghost requires `24-hex-key-id:64-hex-secret`. Its unchanged helper signs the real
five-minute HS256 Admin JWT with jsonwebtoken 9.0.1. The transport verifies the
selected key ID, audience, signature and exact post route. The response must
contain exactly one post with the requested ID. The token is generated locally;
this is not an authentication acquisition flow.

Monday uses monday-sdk-js 0.5.2's Node entry and real `.api` method. The only POST
is the exact retained read query at `/v2`, with the selected literal IDs as
variables and API version `2026-07`. Callers cannot supply GraphQL, mutation text,
another API version or numeric IDs. Missing requested items can produce an empty
or smaller result, reflecting provider visibility. Returned IDs must be a unique
subset of the selection. Non-2xx responses fail before SDK JSON handling, even
if they contain `data`. GraphQL errors remain errors. The SDK warning callback is
replaced within this invocation's bundle so provider warning text is not logged;
no global console function is modified by product code.

Mailchimp uses @mailchimp/mailchimp_marketing 3.0.80's real ApiClient and ReportsApi.
The donor's `setConfig` targets a fresh instance in the invocation-local bundle.
The data center is host-selected, replacing its metadata discovery call. The
upstream `fields`/`exclude_fields` action props are strings, but this SDK expects
arrays; neither option is exposed by this profile. Absent, null, empty or other
values for those unexposed inputs are refused. The selected campaign ID and
nonnegative integer `emails_sent` are required in the response. Supplied known
metric fields must be finite nonnegative numbers. Missing/null metric sources
become explicit `null` in derived metrics, and zero-denominator rates are `null`.
A supplied numeric zero stays zero. Other report fields remain the donor's
bounded projection rather than a complete provider schema. HTTP 404 remains a
typed HTTP failure here; it is not converted into the donor's catch-all report.

S3 uses @aws-sdk/client-s3 3.974.0's real ListObjectsV2 serializer, SigV4 signer
and XML model parser through its public requestHandler. The only endpoint is
`https://s3.<selected-region>.amazonaws.com` (or `.amazonaws.com.cn` for a selected
`cn-` region), with path-style bucket addressing. The shared owner verifies the
signed headers, selected credentials, route and query. SDK retry count is one;
region redirects, Express session auth, ambient auth-scheme/configuration lookups
and clock-skew correction are disabled through explicit client options. A fixed
SDK user-agent provider avoids copying ambient execution-environment labels.
No default credential chain or config-file reader executes for these calls.

S3 `prefix` is literal: absent, null and empty all select the bucket root. It
allows Unicode scalar text without control characters and at most 1,024 UTF-8
bytes. JSON Schema also bounds character length; the byte bound is an additional
runtime constraint. `maxKeys` absent/null defaults to 100; values must be integers
1-1000. This default is deliberately smaller than the original action's 1000.
The returned Name, KeyCount and object identities are validated after real XML
parsing. Objects missing Key, Size, LastModified or ETag are refused instead of
silently skipped; zero-byte objects remain valid. Duplicate keys are refused.
Dates become JSON ISO strings and absent StorageClass is omitted.

The donor sorts only the returned page by modification date. This does not mean
these are the newest objects in the entire bucket. `isTruncated` and
`nextContinuationToken` preserve page incompleteness: the token is a nonempty
bounded string when truncated, otherwise `null`. The profile does not consume
continuation tokens or claim a complete listing. It rejects XML entity/DOCTYPE
payloads before the SDK parser and joins all HTTP/body work through the same
Invocation owner used by every other reader.

The four additional direct build pins are bundled, with no new unbundled runtime
dependency. Exact import/composition-port inputs and replacement sources are
retained in `dist/SDK-PORTS/` and bound by `dist/SDK-PORTS.json`. These documented
transport, configuration and result ports are not claims of byte-identical
executable I/O behavior. See the current notice inventory for the full transitive
closure, including inactive fallback SDK modules.


## Eight source profiles with explicit operation effects

These profiles require an already selected local credential. Source tests use
synthetic fixtures only; they establish no live account access, agent acquisition,
registration, business write or provider completion. The host owns when to offer
an operation and any approval policy. This package adds no durable intent,
permission, credential, registration or agent-runtime authority.

| Profile | Host API auth | Additional MCP configuration | Selected input/result |
| --- | --- | --- | --- |
| `activecampaign.create-contact` | `{origin, apiKey}` | `origin` | `email`, required `contactCustomFields` map, optional name/phone; created contact |
| `freshservice.create-ticket` | `{domain, apiKey}` | `domain` | `subject`, `description`, positive `requester_id`, optional classified fields; created ticket |
| `gitea.create-issue` | `{origin, accessToken, owner, repository}` | `origin`, `owner`, `repository` | `title`, optional `body`; created issue in the fixed repository |
| `datadog.submit-log` | `{site, apiKey}` | `site` | `message`, optional log fields and scalar `additionalProperties`; intake acknowledgement |
| `gmail.get-thread` | `{accessToken}` | None | `thread_id`, optional `format`; one thread with message/MIME metadata |
| `slack.get-thread-replies` | `{accessToken}` | None | `channel`, `threadTs`, optional `cursor`; bounded thread replies and continuation |
| `linear.get-issue` | `{apiKey}` | None | `issue_id`; selected issue and bounded labels |
| `google-cloud-storage.search-objects` | `{accessToken, bucket}` | `bucket` | optional listing filters/page token; one object-metadata page |

Every MCP configuration also names `profile` and `token_env`. For example:

```json
{"profile":"gitea.create-issue","token_env":"SELECTED_GITEA_TOKEN","origin":"https://gitea.example.invalid","owner":"selected-owner","repository":"selected-repository"}
```

`createReader` remains read-only at both the type and runtime boundaries.
The separately named `createWriter` API accepts only the four writer profiles:

```typescript
import { createWriter } from "@rkat/toolkit-activepieces";

export function selectedIssueWriter(accessToken: string, selectedFetch: typeof fetch) {
  return createWriter({
    profile: "gitea.create-issue",
    auth: { origin: "https://gitea.example.invalid", accessToken,
      owner: "selected-owner", repository: "selected-repository" },
    transport: selectedFetch,
  });
}
```

A writer invokes one unchanged donor action and permits one exact write request.
There is no retry, token refresh, replay, compensation or implicit deduplication.
The selected-operation contract defines its effect; Slack and Linear POSTs are
reads. The existing Invocation owns the request, response body, cancellation,
deadline and actual cleanup join for both APIs. Host transports must obey the
same no-retry/no-redirect contract as reader transports.

Write failures before transport entry have `effect: "write"` and
`writeOutcome: "not_attempted"`. After entry, every failure conservatively has
`writeOutcome: "uncertain"`, including HTTP errors, lost/malformed replies,
cancellation, timeout, cleanup and result-conversion failure. No HTTP refusal is
currently treated as proof that the effect did not happen. Repeating an uncertain
call can create a duplicate. Metadata contains fixed error text, stage flags and
optional HTTP status; it never contains original exception or provider error text.

A successful write returns `{writeOutcome: "acknowledged", acknowledgement,
data}`. `data` is the real donor result. The three create operations use
`acknowledgement: "created"` after validating the returned resource identity.
Datadog uses `"accepted_for_processing"`; its donor success text does not prove
indexing, delivery to a downstream pipeline or later query visibility.

ActiveCampaign fixes the selected canonical HTTPS origin and uses Api-Token.
Custom-field IDs are positive decimal strings and values are bounded scalars;
false, zero, null and empty strings remain data. Freshservice selects a tenant
subdomain and Basic API-key authentication. Status, priority, source, urgency and
impact use the exposed enum values. Its unchanged action omits empty optional
strings and empty tags, while nested custom fields retain false/zero/null.
Every supplied tag position must be an own string; sparse arrays and null or
undefined elements are rejected before donor entry.
Gitea fixes the canonical HTTPS origin, owner and repository outside tool input;
callers cannot override the repository. Datadog fixes one of seven reviewed sites
and uses only the selected API key. Additional properties cannot overwrite the
standard message/source/tags/host/service fields.

These input strings count Unicode code points and refuse unpaired surrogates
and control characters other than tab, line feed and carriage return where free
text is allowed. Identifier/cursor fields use narrower schemas. Scalar maps have
at most 100 entries and bounded keys/values. In addition to per-field schema
limits, serialized inputs and request bodies have a 131072-byte UTF-8 runtime
bound for these explicitly selected effect plans. Prior read/SDK plans retain
their 32768-byte request-body ceiling, including when an injected transport is
used. The generic HTTP mechanics accept the wider size only after Invocation
validates the selected plan. The default MCP stdio framing bound is still 65536 bytes, so a host using
that transport must send smaller calls or select a different supported framing
limit when embedding. No input can grant a different URL, credential or method.

Gmail uses the real common-HTTP donor, without a file service. Its Thread API
supports `full` (default), `metadata` and `minimal`; the donor's unsupported `raw`
option is excluded. Thread/message identities, duplicate message IDs and bounded
recursive MIME shape are checked. Payload data and attachment IDs remain inline
metadata/references; attachment bytes are not fetched. Full thread response size
still shares the operation's byte/deadline limits.

Slack executes the real WebClient 7.9.0 serializer with a per-client Axios adapter,
zero retries and immediate rate-limit rejection. The actual donor requests up to
200 messages per page and at most 50 pages, deduplicates by timestamp and preserves
a continuation when the bound is reached. Cursor cycles and contradictory
pagination fail. A returned continuation means the thread is partial, even if
some messages were deduplicated. Credentials and the channel/thread binding are
checked on every page. The same byte/time limits cover the whole operation.

Linear executes SDK 7.0.1 with one exact embedded fetch-binding projection inside
the invocation factory. It sends only the retained selected-issue and label-page
queries. The first issue request is followed by at most 20 label pages of 250
labels. The donor's bounded partial-result flag is preserved at that successful
page limit. GraphQL errors, malformed responses, cancellation and resource-limit
failures remain errors even though its label helper catches exceptions. The public
input accepts a UUID or issue identifier, not arbitrary GraphQL.

Cloud Storage fixes one selected bucket. `prefix`, `matchGlob`, `delimiter`,
`includeFoldersAsPrefixes`, `versions`, `pageToken` and `maxResults` are the only
listing options. Page size is 1-1000, default 100. Delimiter, when present, is `/`;
`includeFoldersAsPrefixes: true` requires it. The donor omits false query flags,
matching the provider default. The result preserves its items, prefixes and any
nextPageToken, adds `pageOnly: true`, and defines `totalItems` as the number of
objects in that single page. It does not count the bucket or fetch object bytes.
