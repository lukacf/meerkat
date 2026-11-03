# Third-party source and notices

This package embeds selected Activepieces action/helper/framework code from
revision `e3e1cbebb6dafbf2bb0da35effc6dfd58b8c13e3`. Exact unmodified source,
upstream URLs, Git blob IDs and SHA256 values are retained under `vendor/`.
Activepieces' copyright and MIT Expat text are in `licenses/Activepieces-LICENSE`.
The selected paths are outside its listed enterprise-license exceptions.

The initial generated bundle retained code from these packages; the expanded
bundle's complete measured inventory is `licenses/BUNDLED-PACKAGES.json`:

- Zod 4.3.6: MIT, `licenses/zod-LICENSE`.
- ipaddr.js 2.3.0: MIT, `licenses/ipaddr.js-LICENSE`.
- nanoid 3.3.18: MIT, `licenses/nanoid-LICENSE`.

Deepmerge-ts 8.0.2 is a resolved build input, with no contribution in the initial
measured bundle. Its BSD-3-Clause text is retained in
`licenses/deepmerge-ts-LICENSE` as well. Build tools are pinned in package.json
and package-lock.json. The emitted build manifest records the exact generated
artifact and contributing source modules.

Toolkit changes are the public reader, generated invocation factory, common
transport seam, input constraints and error handling. The original HTTP client
and its TLS environment mutation are excluded. The donor action bodies and
framework constructors are retained unchanged. No Activepieces workflow engine
is included. These observations do not grant permission for unrelated upstream
enterprise paths or remove third-party notice obligations.

The expanded source manifest additionally retains the selected Baserow, Bitly,
Clockify, Coda, PagerDuty, Postmark and Typeform read actions, their piece-local
import closures and package manifests at that same revision. Their donor files
are unmodified. Separate write action files are not included by this expansion;
shared helpers can contain other methods, but only the selected read actions
are exported and the transport permits one exact GET per invocation. No new
npm dependency was introduced for these seven profiles.

The next source expansion retains 15 additional exact files for Google Contacts,
Figma and Acuity Scheduling, including the selected action import closures and
package metadata. Only the three selected reads are exported. Their OAuth auth
objects and dropdown descriptors remain metadata; acquisition and setup
callbacks are not executed. Toolkit's Figma result adaptation omits the donor's
undefined GET request-body placeholder, while preserving its success/response
wrapper. Acuity's returned appointment array is checked against the selected row
limit. No donor file or npm dependency is changed by this expansion.

The content expansion retains 14 additional exact files for Confluence,
Brevo (the upstream sendinblue piece) and Jotform. Only page content without
descendants, a contact lookup and one submission read are exported. The common
HTTP seam supplies the actual donor HttpError for Brevo's defined 404 outcome;
it does not turn other profile errors into successful results. Jotform response
codes are validated before its helper extracts submission content. Source files
and npm dependencies are unchanged; named configuration and bounded input/result
checks are Toolkit-owned adaptations.

The service-reader expansion retains 20 additional exact files for Kit (the
upstream ConvertKit piece), MailerLite and Calendly, including action/helper
import closures and package manifests. Only subscriber, campaign and scheduled
event reads are exported. Kit's V3 query-secret binding, MailerLite's Bearer
header and Calendly's exact event path are validated by the shared transport.
Toolkit also requires their record-valued provider envelopes before donor
extraction. Auth/setup descriptors stay inactive. No donor bytes, existing
lifecycle owner or npm dependency are changed by this expansion.

The business-reader expansion retains 18 additional exact files for Google
Tasks, Campaign Monitor, Okta and Help Scout: four selected action import
closures and four package manifests. The original action/helper files remain
unchanged. The existing seam verifies inline donor URL queries before safely
serializing their selected values, validates bounded provider shapes before
helper extraction, and allows Campaign Monitor's explicit HTTP 404 miss only.
The common cancellation/body owner, framework implementation, npm dependencies
and all 189 prior donor files remain unchanged.

The platform-service expansion retains 212 additional exact files for Airtable,
Toggl Track, Stripe, Pipedrive, WordPress, Google Calendar, Notion and Intercom,
including their eight package manifests and one common token helper. All 207
previous donor files remain byte-identical. The larger retained closure includes
inactive write/trigger modules referenced by piece indexes; retaining a source
file does not expose its operation.

Four explicitly named modules have a build-time statement projection. The
Toggl, Stripe and WordPress indexes contribute only their original auth
declarations and necessary imports. The common helpers module contributes its
exact `getAccessTokenOrThrow` declaration. `dist/SOURCE-PROJECTIONS.json` records
the source and projected SHA256 values, retained statement slices and line
positions; their generated source text is retained alongside it. The selected
action bodies, selected helper behavior and real framework constructors are
unchanged. No broad purity annotation or simulated action/SDK replaces them.

Notion 2.2.14 and Intercom 6.2.0 execute their original SDK operation methods.
Small subclasses inject a fetch/fetcher through their documented options.
The common owner enforces the selected GET, token, URL, bounds and cleanup.
Notion logging is suppressed at the injection point. Intercom's default fetch
retry/timeout implementation is replaced by Toolkit's existing whole-call
limits. Notion's timer still exists; Toolkit now joins the pending transport
when an SDK waiter rejects early. These I/O/lifecycle adaptations are not
claims of unchanged executable transport behavior.

Direct additional build pins are Airtable 0.11.6 (MIT), Stripe 18.2.1 (MIT),
dayjs 1.11.9 (MIT), form-data 4.0.6 (MIT), google-auth-library 10.5.0
(Apache-2.0), @notionhq/client 2.2.14 (MIT) and intercom-client 6.2.0
(Apache-2.0), from the selected donor package manifests. Encoding 0.1.13 and
supports-color 7.2.0, both MIT, satisfy optional imports in the bundled closure;
those exact versions also occur in the pinned upstream lock. The actual
resolved npm graph is frozen in this package's lock, not asserted identical
to the upstream Bun resolution. No new unbundled production dependency is
introduced. Other SDK operations, auth paths and fallback HTTP implementations
may remain as inactive bundled code; this package does not advertise them.

`licenses/BUNDLED-PACKAGES.json` binds all 99 contributing package locations
(96 unique name/version pairs), retained package metadata, resolved integrity
and notice hashes. `licenses/dependencies/` preserves their full texts. The
data-uri-to-buffer 4.0.1 license is embedded in its README, which is retained
verbatim. The tr46 0.0.3 package and source commit declare MIT but omit its
license file. Its exact first upstream MIT license addition is retained as a
clearly attributed supplement with source URL, commit and history under
`licenses/dependencies/tr46/0.0.3/`. It is not mislabelled as a tarball file.
Build verification refuses a changed bundled-package inventory, package
metadata or notice digest. Direct dependencies with no measured contribution
are build inputs rather than bundled-code claims.


The twelve-service content expansion retains 78 additional exact donor files,
including the twelve piece manifests and their selected import closures. All
419 previous donor files remain byte-identical (497 total). Freshdesk and
Webflow add two exact auth-statement projections, for six projections overall.
Their full indexes remain retained unchanged; unused registrations and trigger
closures are not evaluated. Selected action run bodies are unchanged.

Additional direct build pins are @googleapis/docs 9.3.0 and @googleapis/drive
20.2.0 (Apache-2.0), contentful-management 11.48.1 (MIT),
@microsoft/microsoft-graph-client 3.0.7 (MIT), @hubspot/api-client 12.0.1
(Apache-2.0) and @supabase/supabase-js 2.49.9 (MIT). Their exact lock closure
includes auth-js 2.69.1, googleapis-common 8.0.3 and contentful-sdk-core 9.4.5.
The Supabase top-level package is a retained build/provenance input; the actual
executed constructor comes from auth-js. The previous lock's package entries
are unchanged; forty package paths are added. Production MCP/Undici pins are
unchanged.

Google Docs/Drive inject a supported Gaxios adapter into the actual donor-created
OAuth client. Contentful injects an Axios adapter into its real plain client;
Outlook supplies a per-client authentication/terminal middleware chain; HubSpot
uses a real Client with its instance contact API composed from the actual
generated BasicApi and request/response classes. The invocation-local bundle
scope also isolates HubSpot's module-local decorator singleton. Supabase is an
explicit constructor port to the real public GoTrueAdminApi, exposing only the
shape needed by the original action. Webflow is an explicit selected-method
port from its deprecated v1 path to v2. HubSpot's Date-valued model result is
projected to JSON ISO date strings. These are disclosed I/O, composition and
result deltas, not claims of byte-identical executable SDK or transport reuse.

The two Google API subpackages declare Apache-2.0 but omit a LICENSE file from
their npm archives. Each retained notice includes its exact README declaration,
original generated copyright header, and the standard Apache 2.0 license text
from the locked googleapis-common package. It is labelled as supplemental text,
not an omitted tarball file. Agent-base 6.0.2 and https-proxy-agent 5.0.1 embed
their full MIT notices in README files; those are retained verbatim. The measured
inventory distinguishes every contributing package path, including nested
versions and inactive SDK fallback code. The normal build checks all retained
notice hashes and package metadata before succeeding.

The eight-service read expansion retains 88 additional exact donor files,
including package metadata, the selected action/helper closures, Xero's full
inactive index-referenced source and common props-validation implementation.
All 497 previous donor files remain byte-identical (585 total). The Xero index
contributes only its exact auth declaration and required import, bringing the
statement projections to seven. Inactive source retention is not an executed
operation or lifecycle claim; build proof records the actual contributing files.

The only additional direct dependency is @googleapis/sheets 13.0.2
(Apache-2.0). Its real generated values.get method uses the existing per-client
Google-auth/Gaxios transport seam. Like the earlier Docs/Drive subpackages,
its npm archive omits a LICENSE file, so retained supplemental notices include
the exact README declaration, generated copyright header and standard Apache
2.0 text from locked googleapis-common 8.0.3. The supplemental provenance is
explicit in the bundled notice inventory. Graph 3.0.7 and all earlier lockfile
package entries remain unchanged.

Selected action bodies remain unchanged. Toolkit input projections quote and
escape literal Sheets tab titles and URI-encode Excel/Teams path identifiers.
Teams retains the real helper with an explicit maxRetries=0 composition; Xero
fixes the selected tenant and SEARCH_TERM branch; QuickBooks refuses backslash
literals its real query helper cannot safely compose. Algolia retains the real
browse loop but receives a lower selected default/maximum and an owned cursor,
request and cumulative-response-byte budget. Dropbox and Algolia introduce
exact semantic read-only POST contracts, without authorizing mutation routes.
These request/result/limit deltas are Toolkit adaptations, not claims of
byte-identical executable I/O semantics. No auth callback, refresh flow, write
action, trigger or unrelated SDK operation is exposed.

The four-service SDK expansion retains 58 additional exact donor files for
Ghost, monday.com, Mailchimp and Amazon S3, including piece manifests and selected
import closures. All 585 prior files remain byte-identical (643 total). The
Mailchimp index adds one exact auth-statement projection, for eight statement
projections overall. Selected run bodies and framework constructors remain
unchanged. Unselected registrations, triggers and auth callbacks do not execute.

The new exact direct build pins are jsonwebtoken 9.0.1 (MIT), monday-sdk-js 0.5.2
(MIT), @mailchimp/mailchimp_marketing 3.0.80 (Apache 2.0) and @aws-sdk/client-s3
3.974.0 (Apache-2.0). All 203 prior non-root lock package entries remain unchanged;
115 package locations are added. The generated bundle now contributes from 172
package locations (168 distinct name/version pairs). Every contributing location
has retained package metadata and notice hashes in licenses/BUNDLED-PACKAGES.json.
MCP and Undici runtime pins are unchanged. These counts describe this successor
build, not the earlier measured bundles above.

The three locked AWS packages @aws-sdk/credential-provider-http 3.972.74,
@aws-sdk/credential-provider-login 3.972.79 and @aws-sdk/nested-clients 3.997.46
omit LICENSE files from their npm payloads while declaring Apache-2.0. Their
retained package metadata is accompanied by the standard Apache 2.0 text from
locked @aws-sdk/client-s3/LICENSE, from the same upstream repository. Each
supplement is labelled explicitly in the notice inventory, not presented as an
original file from the smaller npm tarball. These fallback modules can contribute
inactive bundled bytes without their credential or auth flows executing.

The original Monday Node SDK entry, fetch-binding/warning modules, Mailchimp
entry/ApiClient/data-center helper and S3 client helper are bound to their
explicit replacements in dist/SDK-PORTS.json. Exact inputs and replacement
source are retained in dist/SDK-PORTS/. Monday keeps its real query method;
Mailchimp keeps ApiClient/ReportsApi with a per-invocation instance and selected
Superagent-compatible I/O port. Mailchimp data-center discovery is replaced by
host-selected configuration and its incompatible string field-selector props
are omitted from the public profile. Missing metric inputs and undefined rates
are explicitly projected to null after the unchanged action runs.

S3 replaces the original credential/OIDC client-composition helper with a real
selected S3 instance, static credentials and its public requestHandler. The
original serializer, SigV4 signer, XML parser and action sorting remain in use.
The port validates the SDK model before the donor's omission loop, adds parsed
continuation metadata the donor drops, and converts Date values to ISO strings.
Ghost executes its actual JWT helper. All four use the existing HTTP and body
lifecycle owner. These are declared import, configuration, result and transport
deltas; unrelated SDK operations and acquisition paths are not exposed.

The eight-profile effects expansion retains another 96 exact Activepieces donor
files (739 total), including the selected actions, helper/import source and piece
manifests. All 643 AP55 donor files remain byte-identical. Freshservice and Linear
add exact index auth-statement projections, for ten statement projections total.
Unselected action and trigger source is retained only as source closure; no
unrelated operation is registered or exposed. ActiveCampaign, Freshservice and
Gitea use their unchanged real common-HTTP actions. Gmail and Cloud Storage also
use the unchanged common-HTTP donors. The selected Linear query constants in
src/effect-queries.ts are exact source copies used to bind allowed SDK requests.

The five additional direct pins are @slack/web-api 7.9.0 (MIT),
@datadog/datadog-api-client 1.43.0 (Apache-2.0), @linear/sdk 7.0.1 (MIT),
@googleapis/oauth2 5.0.1 (Apache-2.0) and mailparser 3.9.20 (MIT). The latter two
remain exact donor import dependencies; no file service is provided. This bundle
contributes from 219 npm package locations, 214 distinct name/version pairs,
bound in licenses/BUNDLED-PACKAGES.json. These counts supersede the earlier
measured bundle counts above. Existing MCP/Undici runtime pins are unchanged.

Slack's actual WebClient, API methods and Axios serializer execute through a
per-client adapter with retries disabled and rejectRateLimitedCalls enabled.
Datadog's actual LogsApi, model serialization and response processor execute
through an explicitly constructed own Configuration and httpApi. The donor's
configuration helper is ported so createConfiguration's ambient credentials and
shared server-selection helper do not run. Exact original helper/WebClient source
and replacement ports are retained in dist/EFFECT-SDK-PORTS/ and hash bound by
EFFECT-SDK-PORTS.json. The source action bodies themselves remain unchanged.

Linear 7.0.1's published ESM SDK embeds its own fetch binding. Exactly one matching
statement is replaced with the invocation factory's owned fetch binding. The
complete original SDK, exact replacement SDK and published source map are retained
under dist/EFFECT-SDK-PORTS/, bound by dist/LINEAR-SDK-PROJECTION.json. The installed
npm SDK omits a LICENSE file; the exact upstream MIT license at release tag
@linear/sdk@7.0.1, commit 3452ce144ee6dae8e8c6dccbdcc312e6dde95b72, is retained with
its provenance. This is a declared SDK source projection, not an unchanged
transport claim. The per-factory Node HTTP fallback refuses unowned requests.

The source map exposes 20 embedded dependency source files from nine packages:
graphql 15.8.0, isomorphic-unfetch 3.1.0, node-fetch 2.6.12, punycode 2.3.0,
tr46 0.0.3, tslib 2.6.1, unfetch 4.2.0, webidl-conversions 3.0.1 and whatwg-url
5.0.0. All 20 source contents exactly match the corresponding public npm tarball
files, and all nine tarball SHA512 integrity values match the exact release-tag
yarn.lock. Their separate metadata and notices are retained in
licenses/linear-embedded/, with exact hashes and source identities in its
MANIFEST.json. Tagged build/lock source is also retained. tr46's tarball omits its
license text, so its explicitly labelled first upstream MIT license addition is
retained using the existing provenance record. Embedded sources are not silently
covered by the top-level esbuild notice inventory; both inventories are verified.

Four profiles expose explicit write effects through createWriter and MCP write
annotations. There is one exact request and no retry/refresh/replay/compensation.
Safe error/outcome metadata and Datadog intake-acknowledgement result wrapping are
Toolkit projections. Gmail excludes the donor's unsupported raw Thread format.
Slack preserves bounded continuation. Linear's label helper cannot suppress
owned transport/GraphQL/cancellation failures. Cloud Storage returns one page
with a pageOnly marker; its totalItems is a page count. These are declared
configuration, validation and result constraints, not expanded upstream features
or agent-led acquisition and runtime-authority implementations.

The source-closure correction retains Pako 2.2.0's Zlib notice separately from
its root MIT LICENSE. Eleven contributing `lib/zlib/` modules share that exact
notice; `licenses/dependencies/pako/2.2.0/ZLIB-NOTICE` preserves it verbatim,
and `ZLIB-SOURCE-PROVENANCE.json` binds each original file, hash and line range
to the locked npm package. Both are checked by the existing bundled-notice
inventory. This adds missing attribution without changing the donor bundle,
selected operations, dependency resolution or execution behavior.
