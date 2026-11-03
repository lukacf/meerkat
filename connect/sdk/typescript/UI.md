# Connection and frontend UI

`@rkat/toolkit/ui` provides framework-free DOM components for setup progress,
service connections, frontend exposure and package management. It is a separate
browser entrypoint from the Node service operations. The optional stylesheet is
`@rkat/toolkit/ui/style.css`.

## Native setup conversation (source embedding)

`NativeSetupConversation` from `@rkat/toolkit/setup-session` borrows an actual
host-selected `Session` from the released `@rkat/sdk@0.8.51`. It uses that SDK's
`Session.history()`, `Session.stream(message)` and `Session.stopRun(runId, reason)`.
`mountSetupConversation` from `@rkat/toolkit/ui` renders its native saved history,
pages earlier/later messages, sends complete user messages unchanged and shows
streamed assistant text. It has no question schema, intent classifier or saved
workflow of its own. Only user and assistant display text is projected; tool
results, reasoning and raw event/error payloads are not rendered. Model text is
not a secret detector: the host must keep privileged material out of the setup
agent's conversation in the first place.

The [typed embedding example](examples/setup-conversation.ts) composes this chat
with the existing management view. The host supplies the real Session, captured
viewer/realm/session admission readback and mandatory permission-invalidation
`AbortSignal`. There is no user-supplied session ID. The host owns exclusive
streaming access to that Session: the released SDK has one stream queue per
session, and the adapter's local reservation does not arbitrate other handles or
clients. The host must preserve that reservation until the native turn settles.
This interactive binding requires admission to view, send and exact-run control,
not just session-read access. Audiences lacking one of these permissions need
the host's separate read-only presentation. An authenticated browser bridge can
implement the exported `SetupConversationPort` without transferring a Session.

That port requires `onStateChange(listener)`, a payload-free presentation
notification after `canSend`, `canStop`, `outcomeUncertain` or the optional
`privateNoticePending` changes. Publish coherent getter values before notifying.
This subscription creates no stream and transfers no native events, run IDs or
secrets. Disabled Send is not evidence of an active run: a completed send may
be waiting for a private notice in the host's controls.

`privateNoticePending` is display-only. A v2 stdio bridge maps the owner's
`private_notice_pending` directly; while a completed send awaits dismissal,
`active_send` is null and `can_send` and `can_stop` are false. V1 ports may omit
this property. The protected host owns the notice body and dismissal, neither
of which is exposed through this conversation port. Dismissal does not repair
a failed history read or resolve an uncertain send. Admission loss retires
availability. A state notification or enabled Send button does not establish
task, installation or frontend completion.
Closing and reopening a view preserves the same owned conversation. The reopened
view can enable Stop when that original run becomes observable, and refreshes
canonical history when the original send settles, including uncertain failure.
It does not replay the send or recover streamed text missed while unmounted.
Disposal detaches presentation subscriptions without stopping the session.

Admission is checked before history, send and stop, and again before late history
or final turn/stop results are exposed. Relevant scope or permission changes must
abort the host signal immediately. That retires the binding, clears current
presentations and drains later events without displaying them. It does not undo
already delivered text or close/stop the native Session. There is no new remote
per-chunk approval check. The actual SDK may still finalize its privileged Session
state while the retired presentation drains. A throwing presentation callback is
detached; it cannot stop or replay the native turn.
Repeated native `turn_started` events reset only their identified live message,
so a provider restart does not append replayed text to the previous draft.

Stop is enabled only after this send's primary `run_started` names the bound
session and a nonempty native `identity.run_id`. It targets that exact run once,
retaining the native `stopped`, `not_current` or `not_stoppable` distinction. It
never falls back to session interrupt or stopping a new current run. A transport
error after send entry makes the outcome unconfirmed. Refreshing history does
not settle that uncertainty, and remounting the same binding cannot resend.
The host must reconcile native input/run state before creating another admitted
binding. Disposing a view only detaches DOM/listeners; it never cancels the run,
archives the session, or installs anything.

A returned turn is conversation progress, not approval, authentication,
installation, a reusable grant, original-task completion or a running frontend.
Those remain separately observed actions in the management view. Both service
and frontend setup can be discussed in the same conversation.

This is source-level embedding support, not stock MobKit console integration or
packaged Connect delivery. The TypeScript native Session client is Node-oriented;
it must remain in the privileged host, not be serialized to a browser. A web
console still needs its authenticated application bridge and the optional MobKit
application slot. This package introduces neither a user-operated Connect server
nor invented MobKit RPC methods. The SDK is an exact optional peer, with the same
release installed for component type/event tests. No native/provider execution,
sign-in or host adoption is implied by those tests.
The browser UI declaration closure uses only the safe view model and remains
usable without that optional native peer; only the native adapter entry needs
the released Session types.

```ts
import { mountToolkitView } from "@rkat/toolkit/ui";
import type { ControlRequest, ToolkitSnapshot } from "@rkat/toolkit/ui";
import "@rkat/toolkit/ui/style.css";

// Application composition supplies these authenticated host operations.
declare const currentViewerScope: string;
declare const readOwnerProjection: () => Promise<ToolkitSnapshot>;
declare const submitToOwner: (request: ControlRequest) => Promise<void>;

const panel = mountToolkitView(document.querySelector<HTMLElement>("#connections")!, {
  scope: currentViewerScope,
  dispatch: submitToOwner,
});
panel.update(await readOwnerProjection());

// Subsequent authenticated owner events or bounded ticket reads call update.
// Route removal disposes only the view. It does not cancel any owned work.
// panel.dispose();
```

The sample host functions are application-supplied ports, not existing MobKit RPC
method names. This package does not install a management endpoint, establish a
user session, open a browser or implement an authorization queue. Its production
integration remains dependent on those owners.

## Projection contract

Each `ToolkitSnapshot` is a complete projection for the mounted scope. The host
authenticates and filters the person, tenant and audience before returning it.
Use a strictly increasing safe-integer presentation `revision`; an old or equal
revision is ignored. A different scope is refused. Dispose and mount a new panel
when the authenticated viewer changes. These checks prevent accidental mixing;
they do not authenticate the viewer or supply authority.

The snapshot is cloned once before checking scope or revision; that same clone
supplies validation, rendering and control dispatch. Its typed presentation fields,
discriminants and row identities are checked before rendering. Unknown DTO fields are refused with value-free errors. Conflicting payloads or displayed sign-in
context for the same owner and offer within one snapshot are refused; identical
offers in different rows remain supported. These checks do not authenticate data.
Rendering stages detached DOM, navigation and control occurrences. Only a complete
render advances the accepted revision. A malformed projection or render exception
throws while retaining the previous view, focus, controls and pending dispatch
observations. A corrected snapshot with that rejected revision can still be
accepted. Controls from a rejected detached render remain inert after correction.
Focus callbacks may synchronously supply a newer snapshot; its accepted view and
announcement remain current. Projection failure never dispatches, retries or labels
owned work failed.

The snapshot is cloned before rendering. Changing an input object after update
cannot replace a displayed account, action reference or destination. All content
is rendered as text. Host IDs may contain separators; internal navigation uses
tuple keys rather than concatenating their meanings.

The host supplies the meaningful status text and tone. `ready` is a presentation
tone, not a readiness computation. Task, service grant, connection, frontend,
package and supporting-component status remain separate observations. A connected
account does not grant its future use. A green process does not activate a
frontend. Expiry and ceremony state are owner observations, never local timers.

Service cards require account, effective operations/resources, grantee/task,
access lifetime and any standing authority. Connection management additionally
shows provider credential scope, separately from effective grants. Frontend cards
require endpoint, admitted audience, target, conversation policy, output
visibility, service authority and exposure lifetime. Combined tasks display their
role outcomes independently. Diagnostics live in keyboard-accessible disclosures.

IDs are unique within their corresponding snapshot collection and within each
task's same-kind role/action collection. Package export IDs identify exports,
not connection or frontend instances. Each export can supply `instanceIds`:
connection IDs for a `service` export, or frontend IDs for a `frontend` export.
This explicit host-projected relationship supports several accounts or endpoints
for one export. Every referenced instance must be present in the same snapshot,
belong to that package, and be assigned to only one export of its role. Missing,
wrong-role, cross-package and repeated targets reject the whole snapshot under
the existing atomic validation contract. A host that filters an instance out
must also omit its reference; removal updates both collections together.

**Migration:** `instanceIds` is optional, so older typed projections remain valid
and keep their cards and owner controls. Omitted or empty references render no
package-to-instance links. The previous equality-based ID matching is removed;
hosts must provide explicit references to retain those links. For example:

```ts
const mailExport = {
  kind: "service" as const,
  id: "mail-access", // Canonical package export ID.
  label: "Mail access",
  status: { tone: "ready" as const, text: "Host reports active" },
  instanceIds: ["work-mail-connection", "personal-mail-connection"],
};
```

Package navigation displays each target's account or endpoint/audience and uses
that visible context in its accessible name. Its focus identity includes the
export row as well as the target, so moving a target to another export does not
silently transfer focus. Exact in-view buttons link package instances and pending
setup tasks without constructing URLs or dispatching owner operations. These
relationships grant no access. The host must supply a separate management route before
removing the frontend that currently carries setup, or obtain the required
explicit authorization for loss of that route.

`announcement` is the host's concise changed-status message for the polite live
region. Include material changes to every affected task, not just the first one.
The renderer retains keyboard focus on the same control occurrence across updates.
When it disappears, focus goes to its contextual heading or the panel heading,
never a neighboring destructive control. Headings reached by navigation or fallback
also retain focus across later updates. An update does not move focus from outside
the panel. Control names are associated with their visible card heading and, for
service controls, the displayed account; frontend controls include the displayed
endpoint and audience. These associations remain when the separate outcome
description changes to pending or uncertain. Opaque owner/action references are
not used as labels. All statuses also remain ordinary visible text; animation,
color and popups are unnecessary.

## Controls and uncertain outcomes

Only the supplied `OwnerControl` entries become buttons. Stop setup, cancel task,
change account, revoke access, disconnect account and remove frontend are distinct
controls. The host routes each to its actual owner, preserving other waiters and
existing resource ownership. The renderer does not infer allowed controls from
status, and does not add an approval ceremony when standing authority suffices.

Each control carries its canonical owner reference/revision and a host-issued
`offer` correlation, unique within that owner. Keep the offer and its payload stable while replaying the same offered
control. An explicit newly permitted offer can reopen the same ceremony without
inventing another native attempt or changing the underlying target revision.
The offer is a presentation correlation, not a grant, credential, durable
idempotency key or substitute for the owner's exact action identity.

The callback receives the displayed snapshot revision and the exact offered
control, captured before dispatch or focus listeners can update the view. The
renderer reserves the offer and starts that single dispatch before moving an
affected focused control to its own contextual heading and disabling it. If the
host synchronously updates, disposes or moves focus outside, the old render does
not overwrite that presentation. A programmatic click never takes external
focus. If a synchronous callback selects another occurrence of that same pending
offer, the finite disable pass explicitly blurs that occurrence while it is still
enabled and uses its contextual heading only if the host left focus on the body.
This does not depend on when the browser blurs a disabled button. Each occurrence
is processed once; no timer or repeated focus repair chases host focus changes.
Disposal removes presentation only and does not cancel dispatched work.
The host must authenticate the current person and revalidate the owner,
attempt, current permission, target and intended effect before dispatch. Browser
code is not a security boundary against a caller who invokes host endpoints
directly. Callback references must never themselves be treated as permission.

The UI submits an offer once per mount and disables all its duplicate occurrences.
Acknowledgement means only that a request was submitted. Only an owner snapshot
can change operation, role or ceremony state. A throw or rejection displays a
redacted uncertain acknowledgement, leaves that offer disabled, and never retries
or labels the effect failed. Fresh permitted controls come from the owner. The
host still provides durable replay protection across navigation and remounts.

Disposal releases the panel and ignores late callbacks. It does not abort a
host-owned dispatch, cancel a task, retire an auth attempt or revoke access. Browser
closure therefore does not itself cancel setup. Actual continuity depends on the
host's durable operation/ticket reads, not anything persisted by this library.
The renderer has no polling, member/session inspection, network, local storage or
cookie code.

## Optional catalog and manual navigation

An authenticated host can add `catalog?: CatalogView` to its snapshot. Omission
keeps the existing management view. The optional projection contains ordered
`categories` (`id`, `label`) and `entries` (`id`, `label`, `description`, `roles`,
`categoryIds`, `support`, `prerequisites`, optional `evidence`). Each entry has one
or both explicit roles, `service` and `frontend`, displayed as Service access and
Agent frontend. These are catalog descriptions, separate from connected accounts,
task grants, package activation and frontend readiness. The catalog offers no
install, authorization or owner-action controls.

`Search catalog` performs a literal case-insensitive substring match on individual
displayed data values: labels, descriptions, role labels, category labels, support,
prerequisites and evidence. Spaces and punctuation are kept. IDs, field captions,
UI prefixes and absent-value placeholders are not searched. There is no intent
resolver, ranking, fuzzy matching, automatic selection or network request. Empty
query shows all entries in the selected exact category, in host order. The native
selector displays `All categories` for no category restriction and
`Category: <host label>` for each real category, including a real label of
`All categories`. No host ID or label is reserved as a sentinel.

Category and entry IDs must be nonblank and unique in their respective arrays;
category labels must also be nonblank and unique. Entry labels, support, supplied
evidence and prerequisite items are nonblank strings; description can be empty.
Roles must be a nonempty unique subset of the two supported roles. Category
references must be unique and refer to supplied categories. Null or malformed
catalogs are refused atomically with the existing projection. Empty categories
and entries are valid and visibly distinguished from a filtered empty result.

Filters live only in the mounted view, without storage or changes to the host
snapshot. Query/category survive accepted updates and temporary catalog omission.
A later supplied catalog missing the selected category resets to All with visible
feedback and retains the query. Rejected or old updates do not alter them. Search
focus and selection, category focus and surviving entry-heading focus are retained.
A removed or filtered focused entry falls back to Catalog; removing the catalog
falls back to the panel heading. External focus stays with the host.

The optional `onManualConfiguration: () => void | Promise<void>` mount callback
is trusted host UI navigation only. It must reveal the host's manual-configuration
surface without beginning auth/install, approving work, creating an attempt or
changing domain state. It receives no URL, entry selection or authority reference.
The function is captured and validated at mount. One pending navigation is retained
across snapshots, with guarded clicks and `aria-disabled` feedback that keeps the
navigation button focusable. A successful return says only that navigation was
requested. Throw/rejection gives fixed uncertainty text without exception content
or a claim of no effect. There is no automatic retry; a later explicit click after
settlement may invoke it again. Disposal removes presentation without cancelling
host navigation, and stale detached controls cannot invoke it.

With a supplied catalog but no callback the view says that manual configuration is
not provided by the host, with no placeholder button. With a callback but no
catalog, a separate Manual configuration section keeps the route usable. With
neither, the old DOM remains unchanged. This callback does not implement a native
manual setup API or protected browser launcher, nor prove authenticated continuity.

`examples/catalog-fixture.mjs` contains synthetic descriptions only.
`examples/management-catalog.mjs` exports `mountCatalogManagement`, a public-API
composition that reveals and focuses an already supplied host manual panel. The
caller supplies the authenticated snapshot and real OwnerControl dispatcher.
The example does not implement configuration submission or fabricate setup success.

## Human authentication boundary

The shared projection never carries a native authorization URL, state, code,
callback or start-response object. `UserActionView.authorizationOrigin` is an
optional canonical HTTP(S) origin with no path, query, fragment or userinfo. The
host derives it before constructing the projection. It is a destination label,
not verified issuer identity. `requestedAccountLabel` names the requested account
until the native owner reports verification. Both appear as inert text and are
associated with the sign-in control for accessible context.

`open_authorization` is labelled **Review sign-in** and dispatches only the same
allowlisted owner/control reference as other controls. It asks the authenticated
host to present its protected human sign-in surface; it does not launch a URL or
start, complete or cancel a native attempt. A click on an agent-observable app
surface is not evidence of human consent. Hosts without a protected presenter
omit the offered control and describe that unmet prerequisite. The control does
not require an origin label because the protected presenter supplies the full
review before any ceremony.

The host revalidates the current person, offer and selected target/account, then
uses the accepted native auth owner. Native PKCE, state, account verification,
persistence and cancellation remain there. For MCP login, honor Started versus
Joined: the original listener/custodian continues to own a shared attempt; a
later view or waiter does not acquire cancellation authority. Generic connector
login has separate target/result types and no such wire disposition. Browser launch failure is
advisory. Generic panel disposal removes presentation only. Stopping one task is
an operation-owner action and must not cancel surviving waiters' ceremony.

Authorization URLs and callbacks remain exclusively in a host channel excluded
from agent browser, shell, screenshot, MCP and computer-use observation/control.
A system-browser launch, new window, boolean option or inert text is not proof of
that exclusion. No such protected host implementation is supplied by this DOM
component. Status `authorized` is a native credential observation, not proof of a
connected server, usable grant or original-task completion. Wire start has no
authoritative attempt expiry; hosts must not invent one for `expiresAt`.

The validator rejects legacy raw-URL properties and unknown properties on every
DTO record. Dispatch constructs only declared control and target fields. Hosts
must still build a safe projection explicitly, never spread native start/callback
objects or place their values into instructions, account labels, announcements or
diagnostics. Schema validation cannot recognize a secret deliberately placed in
an allowed string. Do not treat this component as an authentication, redaction or
human-isolation boundary.

Secret-entry controls similarly delegate to the host's secure presenter. The
renderer creates no password, token, PIN, ordinary MCP form, URL link, preview,
favicon, QR service, iframe or automatic navigation. An expired action never
creates a Retry button on its own.

### Optional generic connector host binding

The Python [`ConnectorSignInBinding`](../python/CONNECTOR-AUTH.md) attaches callback
custody to an existing authenticated host step. It consumes the same
`ControlRequest` and produces the same `UserActionView`; it does not add a browser
auth API, gateway, credential store or setup lifecycle. It is disabled for opening
unless the host supplies an actual protected presenter. A supplied callback or
the example's mock dispatcher does not prove that protection.

`examples/connector-sign-in.mjs` mounts an already supplied `ToolkitSnapshot` with
the host's real dispatcher. Sign-in stays in `userActions`; verified account and
credential-scope observations stay on the separate connection. Only the host can
project grants, readiness and original-task outcomes. A native `authorized` report
is insufficient. `view.dispose()` releases the UI only, so an existing host step
can retain its callback transport across remounts. Offers must still be admitted
and replay-protected by that host.

This optional Python binding targets reviewed Meerkat PR 1732 source, not the
released 0.8.50/0.8.45 consumption baseline. It does not complete the native
provider protocol or credential-use bridge. See its guide for exact contracts,
uncertain outcomes and loopback-local profile limits.

## Verification and preview

Run `npm run validate` for TypeScript and the Node/jsdom suites. The tests exercise
safe sign-in context, reference-only dispatch, strict DTO fields, stale views and controls, duplicate submissions, uncertainty,
scope separation, role distinctions, owner control targeting, focus and navigation.
They are component evidence, not live provider or authenticated native integration
evidence. Automated DOM checks do not establish assistive-technology compatibility.

After `npm run build`, serve this directory and open `examples/management.html` for
the synthetic preview. Its CSP forbids network connections, frames and images.
Clicks acknowledge locally without opening anything or simulating successful auth.
No connected account, public frontend, persistent human-action queue or migration
is claimed by the preview.
