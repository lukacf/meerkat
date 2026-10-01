# ADR-001 implementation addendum: process confinement and human consent

## Status

Proposed implementation contract, 2026-10-01. This extends the accepted
[r8 local authorization decision](adr-001-local-governed-default.md) with actual
process confinement and a concrete human-consent workflow. The accepted r8
snapshot and its review evidence remain unchanged. This addendum does not claim
implementation acceptance or measured performance.

Luka authorized bringing the sandbox implementation and the sandbox, human
approval, on-behalf-of, audit and traceability clarifications into this work.
Detailed workflow choices below are engineering proposals implementing that
direction. They are not additional requirements attributed verbatim to him.

## Context

Tool authorization determines whether a tool may be invoked. It does not limit
the OS resources available to an arbitrary command after invocation. The current
shell working-directory restriction and durable process custody do not establish
filesystem or network confinement. Likewise, an approval audit record alone does
not establish authenticated, expiring, single-use consent for a physical action.

Neither extension changes the disclosure limit. Once information enters an LLM
context, there is no mechanical guarantee that it will not influence later
output. We enforce concrete operations and boundaries. We do not add semantic
labels, context taint, proof of meaning, or a semantic provenance subsystem.

## Process confinement

The existing authorization path supplies immutable mechanical requirements to a
platform adapter. The existing launch owner retains process identity, custody,
cancellation and outcomes. The adapter prepares the exact executable, arguments,
working directory, environment, standard streams and explicit inherited
resources. It enforces all required filesystem, network, IPC and process
restrictions or refuses the individual launch. It is not a second policy engine,
permission store, identity service or process supervisor.

Requirements distinguish literal files from directory subtrees, reads from
writes, denied paths from allowed paths, IP connections from local sockets, and
outbound connections from listening. Exclusions take precedence. Any platform
compatibility baseline is named, versioned and enumerated; it cannot silently
grant the home directory, shared scratch, network access, or runtime secrets.
Paths are resolved on the execution host. Unsupported path forms and encodings
are refused rather than lowered to a different resource.

The child receives an explicit environment and descriptor/handle set. Ambient
credentials, inherited privileged sockets, parent file descriptors, shell
startup variables and loader overrides cannot bypass the boundary. Trusted
bootstrap code must be protected before it enters the sandbox. Custody gate
resources disappear before the untrusted target executes.

Use a bounded adaptation of the Codex implementation where appropriate:
Seatbelt on macOS, namespace/bubblewrap and seccomp confinement on Linux, and
restricted identities/tokens plus the required ACL, firewall and job machinery
on Windows. Preserve applicable source attribution and licenses. Do not import
the Codex application, permission authority or child supervisor into Meerkat.
The inspected reference is `d6c3b448a41311ece3255c52ec3dbfd9ff36f154` in
`/Users/luka/src/cc/codex`; adapted files record their exact source and changes.

A backend reports its actual guarantees. Seatbelt alone does not prove resource
quotas or termination of descendants that escape an owned process group.
Required unavailable guarantees refuse the operation; there is no automatic
unrestricted fallback. Proxy environment variables are configuration, not
network enforcement. A restricted network mode must prevent direct connections
that bypass the trusted proxy or allowed destinations.

Probe backend capabilities once at host/profile setup and publish a typed
capability report. An unmet mandatory profile requirement is a typed setup
failure; optional unavailable tools report unsupported through ordinary model
feedback. Genuine per-launch failures remain local. Do not admit a profile
known to produce an undiagnosable storm of identical unsupported refusals.

Linux support includes common containers and CI hosts without unprivileged user
namespaces. A Landlock/filesystem and seccomp/socket backend provides the subset
the installed kernel can enforce without user namespaces; bubblewrap/namespaces
provide stronger isolation where available. Neither backend may claim unsupported
endpoint, IPC or filesystem semantics. A deployment can explicitly select an
existing container/pod boundary as its backend, declaring its image/volume,
network and process guarantees. A shared pod is not automatically a boundary
between differently authorized agents or between tools and privileged brokers.
Only the isolation actually configured and enforced counts toward requirements.

Compile immutable platform policy once for the existing policy owner's
configuration generation. Each launch binds exact process inputs to that
compiled policy and performs the required current-owner check. Do not add a
parallel generation registry or repeatedly compile rules on the launch path.

Coverage includes foreground shell, background jobs, initial and recovered
monitors, local MCP processes, command hooks, skill-source processes, and
filesystem helpers. Package installation is also a launch family: downloads,
extraction, lifecycle scripts, build tools and bootstrap effects use the same
declared boundaries. A package cannot declare itself trusted host code.
In-process arbitrary native callbacks remain trusted code;
untrusted extensions run out of process. Runtime and credential brokers remain
outside the tool sandbox, with their own explicit access boundaries. Remote,
browser and WASM execution must describe the boundary actually enforced there
and refuse requirements that it cannot provide.

Host-launched MCP servers use the same adapter or are explicitly declared
trusted host code. Long-lived device, browser and daemon helpers may run as
declared host-managed brokers; sandboxed tools reach them only over specifically
authorized endpoints. A browser adapter reports both its outer boundary and the
state of its own inner sandbox. Disabling an inner sandbox is an explicit
deployment choice, never an automatic fallback. These compatibility claims need
actual browser/device tests, not assumptions about nested sandbox behavior.

The host supplies exclusions for its real secret files, credential directories,
live stores and privileged control endpoints, including those under otherwise
allowed roots. Legitimate credentials use explicit per-tool injection or a
broker. Package caches, selected home directories and device access are named
requirements rather than implicit access to every host resource. Capability
preparation fails before starting a device operation whose required mechanical
guarantees are unavailable.

An already-running process can retain open OS resources. Permission narrowing
must stop or replace affected long-lived workers through their existing owners
before claiming that new restrictions govern their operations. Logical
invalidation suffices only when the existing owner mechanically prevents every
further affected operation. It cannot retroactively revoke bytes already read
or undo completed effects.
If stopping a worker fails, report that it still holds its previous rights and
preserve the actual owner's cleanup responsibility. Do not block unrelated work
or silently claim that narrowing succeeded.
Narrowing is scoped to affected agents and operations. A shared worker requires
per-client enforcement or replacement of the affected client's access; an
unrelated member's work is not cancelled by a global worker restart.

Preparation failure produces ordinary tool feedback. An OS-denied command keeps
its real exit status and output. Do not infer a trusted security verdict by
matching arbitrary child stderr. Neither failure terminates the agent run, and
neither triggers an unrestricted retry.

## Human consent

The existing approval owner and `ApprovalLifecycleMachine` own consent. A trusted
application policy declares which otherwise-permitted actions require approval,
who may approve them, and the authenticated presentation and decision channel.
Consent is a conjunction with agent permission, the original requester's
authority, delegation, account and resource policy, confinement, and required
gate routing. It cannot override a nonoverridable ceiling or activate every
permission held by a connected human account.

The operation owner retains the exact candidate. The presentation identifies its
concrete action, target and recipient, relevant arguments or effect summary,
selected account, requester, executing agent and expiry. An audience unable to
read the required details cannot supply informed approval through a redacted
placeholder. The agent receives typed action-required feedback and may continue
other permitted work. No run is parked merely to await consent.

The host authenticates the decision event and checks current approver
eligibility. A caller-supplied actor string, model statement, gate reply or
replayed callback is not authentication. Consent binds the retained candidate,
actual parties, qualified resource and recipient, selected account and route,
relevant owner policy generation and finite expiry. Changed binding requires
fresh consent. Diagnostic counters are not durable policy generations.
For executable actions, the binding includes the actual executable selection
and prepared artifact or manifest. Unchanged arguments do not preserve consent
when the selected script, package or executable artifact changes. The launch
owner must retain and enforce that selection, not merely compare a pathname
before later executing replaceable content.

Approval records a decision; it does not execute or replay the action. A later
explicit attempt resolves the same retained candidate through its owner and
rechecks all current requirements. Equal-looking JSON or a copied approval ID
does not transfer consent. Lost or unrecoverable live candidate/work bindings
require fresh consent, not a fabricated recovery association.
Neither a historical approval nor a missing audit entry authorizes another
physical attempt after restart. If the owner cannot recover the binding and
disposition, it preserves the uncertain outcome and returns action-required
feedback. Fresh consent alone does not establish that repeating a possibly
completed effect is safe; the existing operation owner must resolve that
uncertainty or use its own applicable idempotency guarantee before another
physical attempt. This adds no journal or per-action fsync requirement.

The approval owner delivers approved, declined, expired and cancelled outcomes
as typed inputs to the owning session through existing admission. The input
retains the original requester/work association and candidate reference; it can
arrive after the original run has completed and wakes the session under normal
turn and admission rules. Delivery does not execute the action. The agent or
host makes a later explicit attempt, which resolves the retained candidate and
rechecks current requirements. An expired or lost candidate returns typed
fresh-consent-required feedback. Missing delivery is observable and retryable
through the existing delivery owner, not silent success or a new parked run.

At the designated physical entry boundary, after waits and preparation, the
operation owner checks current permissions and consumes the still-valid consent
through the approval owner's single serialized transition. Nested checks do not
consume it. Concurrent attempts cannot consume it twice. Consumption is distinct
from entry and from outcome; a later failure does not silently restore consent,
rewrite a completed effect, or retry an uncertain effect.

Decline, expiry before or after approval, cancellation, changed policy, missing
binding, unavailable service and previous consumption refuse the new entry and
produce typed local feedback. They never relabel a prior uncertain or completed
effect as unexecuted. Cancellation after consumption
cannot undo a physical effect. A restored historical approval record is not an
executable capability. Existing store ownership and durability limits apply;
there is no new exactly-once physical execution claim or default per-action
fsync requirement.

## Delegation, gates, audit and traceability

Keep requester, agent, represented subject, selected external account and
delegation distinct through helpers, schedules, connectors and gates. For
example, calendar-read delegation cannot authorize deletion merely because the
OAuth token can delete. A shared assistant cannot use one user's account
authority for unrelated callers. A gate executor may have publishing capability
that its upstream agent lacks while remaining subject to the original requester
and delegation ceilings.

Mandatory gate routing and human consent are independent requirements. The
runtime can enforce review of an exact action and recipient; it cannot prove
that the gate understood the content correctly. Gate review is not human
consent, and human consent does not bypass a mandatory gate.

Audit joins the existing operation and state owners. It records authorization
decisions, consent transitions, entry attempts, actual results and uncertainty
separately. Records use existing commits; an in-memory host provides
process-lifetime history, and a crash can lose recent uncommitted observations.
Exporter failure does not terminate the run. Operational correlation connects
request, delegation, attempts, retries, account, historical policy evidence and
outcome. It is not semantic provenance, a hidden-reasoning trace, or proof of
tamper resistance, rollback or exactly-once external execution.

## Acceptance

The first complete path must demonstrate a refused action reaching the model,
then a permitted action completing in the same run through real native work
admission. Each additional surface must use the same owner contracts. A narrow
fixture remains an integration milestone, not full coverage.

Confinement evidence includes real positive controls and blocked filesystem,
symlink/rename, inherited secret/descriptor/socket, direct network and descendant
operations; concurrent differently authorized agents; background and recovery
paths; unavailable-backend refusal; and ordinary development-tool compatibility.
Skipped OS probes are reported as unverified, not passed.

Consent evidence includes authenticated and forged decisions, changed candidate
or account, expiry at use, cancellation, simultaneous consumption, restart with
lost binding, unaffected sibling operations, and preservation of physical
results after settlement failure. Narrow external-account delegation is tested
independently of the broader OAuth credential.
Tests include decisions arriving after run completion and an ineligible member
clicking a shared-channel approval control. Visibility or channel membership
does not establish eligibility.
Each restriction has a permitted positive control as well as a refused case.
Tests include executable selection changing behind unchanged arguments and
package installation or bootstrap attempting effects outside its boundary.

Land reviewed slices in dependency order: existing-owner consent; Linux/macOS
confinement for foreground shell, background jobs and local MCP; remaining
launch families; then Windows. An unsupported platform reports that fact until
implemented. Earlier slices are useful integration milestones; full platform
coverage remains the completion requirement.

Measure authorization checks separately from process launch and explicit gate
or human workflow latency. Ordinary checks remain local with no additional
network request or fsync. Serialize heavy builds and benchmark only in a
confirmed quiet window. Review implementation deltas against accepted evidence
without reopening the settled semantic disclosure limit.
Representative compatibility measurements include browser, package-runner and
device-broker cold starts on the minimum supported host class.
