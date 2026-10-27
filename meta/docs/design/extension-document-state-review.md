# Independent extension-state and native adapter review

Date: 2026-10-05. Reviewed the current uncommitted Meta worktree, the
upstream Meerkat creation/activation changes, and a downstream adapter
(`src/native.rs` and `tests/native.rs` in its separate repository). This reviewer authored the design; different agents wrote
the implementation. This pass changed documentation only and ran no builds.

## Findings

### P1: Initial identity kickoff can acquire the temporary Worker principal

Primary location: `crates/meerkat-meta/src/extension_state.rs`,
`NativeAuthorityRegistry::principal` (lines 252-280 before formatting).

An absent retained identity binding plus covered member birth is classified
as a Worker. That absence is not yet final while an application identity is
being materialized. `identity_first/runtime.rs:7609-7619` first persists a
provisional session ID. `identity_first/bridge.rs:5710-5739` receives that ID
but does not put it into the spawn spec; it learns the actual session ID
after spawning. The spec forwards `initial_message` at `bridge.rs:5331-5332`.
Upstream `crates/meerkat-mob/src/runtime/actor/spawn_activation.rs:2186-2224`
admits that initial turn before the spawn commit and reply. Only after the
bridge returns does `runtime.rs:7771-7774` persist the actual session binding.

Therefore an extension call in the initial turn can resolve as Worker, while a
later call from the exact same session resolves as Agent. A created private
document becomes inaccessible to its later logical owner. ABAC also evaluates
the wrong subject during this window, so an Agent-specific denial does not
necessarily apply. The creation coverage anchor proves historical coverage;
it does not prove that the identity binding transaction has completed.

Required fix: establish the actual session's authoritative identity binding
before it can execute, or hold initial execution until that binding commits.
Do not derive the stable identity from member names or labels. A regression
must issue the first document call from an identity's kickoff turn and assert
the stable principal, its Agent-specific ABAC policy, and ownership continuity
on the subsequent turn and after restart.

### P2: Cold-resume registry retains a lifecycle watch that never updates

Primary location: `crates/meerkat-meta/src/extension_state.rs:212-219`,
the existing-handle replacement check in `before_activation`.

Upstream `crates/meerkat-mob/src/runtime/builder.rs:7780` creates an isolated
preview phase channel. Its receiver is installed on `preview_handle` at
line 7830, and the extension callback registers this handle at lines
7842-7843. Final runtime startup passes no callback, so this remains the
registered handle. `MobHandle::status_observation_snapshot` reads only that
phase receiver (`runtime/handle.rs:7486-7487`), not the shared machine state.

A mob restored in Running state and then stopped still appears Running to
the registry, and reopening it in the same host is refused as already bound.
A mob restored Stopped and later started has the inverse stale observation,
so this check can permit replacement while it is live.

Required fix: have preview and final handles observe the same live lifecycle
publication, or use an equally authoritative current lifecycle check. Test
cold resume, state transition, and a second same-host opening in both starting
states. A generic new-runtime restart test does not exercise this sequence.

Source re-review: root changed `RuntimeWiring` to carry one `phase_watch_tx`
through cold resume and final actor construction. The preview receiver stays
subscribed to that sender, and final construction reseeds it with
`send_replace`. This addresses the stale-watch finding in source. The added
`before_activation_retained_resume_handle_observes_live_lifecycle` regression
covers both initial phases, transitions and destroy. Compilation and execution
are still pending; this reviewer did not run them. Root also seeds the preview
roster from committed replay and applies the exact newly appended recovery
event to both local and shared roster projections before member provisioning.
Those bounded changes preserve the journal as the authority.

### P2: Edit retry misses a committed receipt if deletion wins before its read

Location: reference downstream adapter `src/native.rs:447`, the fallible
document read before the second receipt lookup at lines 448-468.

The adapter retries receipt lookup when it sees a different revision, but
propagates a failed read immediately. A valid ordering is: duplicate edit
finds no receipt; the original identical edit commits; the owner deletes the
document; the duplicate reads the tombstone and gets NotFound. The backend
still retains and authorizes the original edit receipt against the tombstone
ACL (`crates/meta-extension-state/src/sqlite.rs:126-135`), but the adapter
returns unavailable instead of that receipt. The same request succeeds if
retried once more, making the result depend on this narrow race.

Required fix: repeat authorized receipt lookup before propagating an Edit
read failure, preserving the failure when no authorized receipt exists.
A deterministic barrier test should establish the ordering above and assert
the original receipt with `replayed: true`, without fresh changes or selected
data. Also retain the revoked-authority failure case. This finding is copied
to the downstream `REVIEW-engine.md` as a separate native-adapter follow-up.

## Acceptance evidence still required

The existing downstream native tests explicitly use a `TestAuthority`; they
do not prove `NativeCallerResolver`, the factory bootstrap, or the provider binding.
The implementer reports that real-runtime tests are being added. Acceptance
must include the kickoff and lifecycle sequences above, real same-mob and
cross-mob fork/spawn resolution, descendant revocation after restart, and
ABAC evaluation using the current principal and real resource attributes.
Provider tests must exercise typed unsupported capability and demonstrate that
no extension store is opened when no factory is registered or the feature is
omitted. No native build or runtime test result is claimed by this review.

The parent clarified that an application `AgentIdentity` is an immutable
authority identifier, not a display name. Deliberately recreating that same
stable identifier reactivates the same document owner. Unrelated workers that
reuse a member/display name must receive fresh creation tokens. The design's
same-name warning should be read with that distinction; no new application
incarnation mechanism is requested.

## Inspected protections

The SQLite backend scopes records and receipts by realm and namespace, checks
the current ACL inside the mutation transaction, advances one revision for
data and administration, retains tombstones and retry receipts, and filters
listing before returning records. The native adapter fingerprints the original
typed action and removes recalculated output on a commit-time replay. The
feature and provider capability are optional. These source observations are
not a claim that the unfinished acceptance suite passed.

## Focused follow-up: identity publication and retry repair

This source-only follow-up reviewed `extension_state/identity_publication.rs`,
its bridge/runtime call sites, and the native retry repair. No builds or tests
were run by this reviewer.

### P2: Publication admission changes unrelated member capabilities

Location: `extension_state/identity_publication.rs:56-59` and the unconditional
bridge binding at `identity_first/bridge.rs:5769-5770`, with equivalent reset
and resume call sites.

Registering one extension enables this admission for every identity, including
profiles that never select the extension bundle. Upstream remote placement
rejects any process-local `tool_dispatch_admission`
(`crates/meerkat-mob/src/runtime/actor.rs:31844-31848`), so those remote members
now fail materialization. Local members also lose all provider-native server
tools because `crates/meerkat/src/factory.rs:7381-7392` sets `DisableAll`
whenever this admission exists. Publishing the readiness flag never removes
that capability change.

Required fix: enforce extension identity readiness without installing a
general tool admission on unrelated members or permanently changing their
provider capability policy. Include regressions with an extension registered
but an unrelated remote profile and a profile using provider-native tools.
The implementation owner agreed and is replacing the admission with a
registry-local publication fence; that replacement is not yet reviewed here.

The current wait itself preserves the prior admission and outcome forwarding;
dropping its sender makes waiting calls fail rather than wait forever. Create,
resume and explicit identity-reset successor specs are bound before launch,
and publication follows actual continuity persistence and registration.
Initial-turn spawn completion awaits admission, not tool completion, so this
inspection found no direct spawn/publication wait cycle. The current tests
exercise the fence in isolation; the real-runtime harness still constructs a
plain mob rather than an identity-first kickoff. A first-tool stable-principal
test plus actual materialization-failure and reset/reload cases remain needed.

The replacement must preserve historical scope: a pending or failed launch of
member X cannot block a surviving fork's already proven exact-session ancestor
X. Check existing authoritative bindings before waiting and re-read them after
publication. A closed fence must also not indefinitely cover later unrelated
worker births that reuse X's name. These conditions were sent directly to the
implementation owner and root before implementation of the replacement.

### Native edit retry: addressed in source

The downstream adapter `src/native.rs:447-470` now repeats authorized receipt
lookup when the Edit read fails and preserves the original failure if no authorized receipt
is returned. `tests/native.rs:510-567` deterministically pauses the missing
receipt, commits the original edit, optionally revokes the Editor, deletes,
and resumes the duplicate. It asserts the original receipt without selected
data or changes when permitted, and unavailable after revocation. The code
and test address the reported P2; no execution result is claimed here.

## Registry-fence delta review

This follow-up is source-only. The registry fence no longer writes
`ToolDispatchAdmission` or modifies the session's existing admission. The
remote-placement and provider-native capability regression above is therefore
addressed in source. `MobReadHandle` exposes only direct observations, so the
pre-activation callback does not gain actor mutation commands.

`NativeAuthorityRegistry::principal` now queries retained exact-session
identity history before waiting and again after readiness
(`extension_state.rs:266-293`). An already proven historical stable ancestor
can therefore remain usable while another incarnation of the same member is
being prepared. The parent-policy ceiling now loads the exact source session's
persisted metadata on each resolution (`extension_state.rs:393-414,520-526`);
missing metadata fails with `AuthorityUnavailable`. No public copied policy
snapshot is used for this decision.

### P1: Unpublished stable sessions must remain excluded from Worker fallback

The first replacement used a dropped guard's asynchronous journal-head read
as the upper end of its failed-spawn interval. That read was not ordered after
spawn settlement. A submitted spawn can commit its member after the caller is
canceled and after the sampled cursor; its birth then falls outside the closed
fence and is classified as Worker without an actual stable continuity binding.
A later unrelated birth could instead fall inside a delayed sample. The
implementation owner removed asynchronous sealing during this review.

The agreed conservative contract reserves an identity materialization's target
instead of guessing that its in-flight spawn is finished. Two boundaries still
need to be closed in the reviewed snapshot:

1. `IdentityPublications::bind` replaces a closed fence with the current
   `after_cursor` (`identity_publication.rs:130-145`). An already committed,
   unpublished session covered by the old fence can fall below the new floor
   and escape before `bind_session`. A retry must preserve the exclusion of
   unresolved prior incarnations, including if the retry itself fails.
2. A new registry starts with no reservations (`extension_state.rs:193`), but
   `establish_identity_history_coverage` preserves its first durable cursor
   using `INSERT OR IGNORE` (`identity_first/local_store.rs:2241-2270`). After a
   process restart, an unpublished stable session born above that old cursor
   therefore has neither an exact stable binding nor a fence, and reaches
   Worker fallback at `extension_state.rs:283-301`.

The implementation owner and root are addressing these as the same negative
classification boundary. Their proposed restart protection consults retained
identity-intent history for the root host mob's canonical stable member target,
and refuses unbound sessions there rather than inferring an Agent principal
from a member name. Existing `member_id_for_spawn_spec` uses the same reversible
durable identity codec for every binding (`identity_first/bridge.rs:5147-5171`),
including external bindings. Coincidental names in child mobs must not be
reserved by a root-mob identity. This proposal had not landed when this review
section was written and is not yet marked resolved.

Required regressions are a dropped publication followed by a late committed
birth, a retry after such a birth, and a fresh registry/restarted process after
failure before actual continuity publication. Each must refuse Worker fallback.
Also retain coverage of proven historical stable sessions, ordinary unreserved
Worker name reuse, and child-mob name collisions. The contract should state the
availability consequence of reserving stable targets explicitly.

### Release evidence pending

The identity-first initial-tool, reset and successful-restart test has been
written (`tests/extension_state.rs:802`) but was not executed by this reviewer.
Its happy path does not establish the failed-publication restart invariant.
The focused failure regressions above, the parent-policy-change regression,
the repaired native Edit race regression, and the upstream restored-binding
and lifecycle tests still need their build and execution results recorded.
No additional broad review or build was run in this delta pass.

## Focused re-review of durable exclusion and retained policy

Source re-review now closes the prior unpublished-stable-session P1.
`NativeAuthorityRegistry::principal` grants Agent only from exact session
history, before and after the readiness wait. Before Worker fallback it checks
the exact identity bridge mob, a canonical member-codec round trip, and
retained identity-intent history (`extension_state.rs:274-318`). A provisional
identity record is retained by the existing history trigger even if the live
record is rolled back or deleted. Its existence now excludes an unbound
stable target after restart; the decoded member name grants no identity or
document permission. Coincidental child-mob names do not meet the root-mob
check. The local retry fence also preserves its earliest cursor and refuses a
different pending logical identity (`identity_publication.rs:132-153`).

The subtree-denial evaluator remains correct in the inspected source. It
distinguishes no audience route from a matching route with no data ceiling,
then applies denies before owner or explicit child grants
(`crates/meta-extension-state/src/policy.rs:26-46,72-90`). A zero source
ceiling therefore cannot hide an ancestor from a deny. A direct child grant
can independently authorize data when permitted, but cannot override a
matching subtree deny. The current caller's native policy and ABAC still
intersect every result. Conformance source covers a subtree denial overriding
an explicit child Editor grant; an additional zero-ceiling variant would
directly lock down the nested-option distinction.

### P1: Retained metadata seam omitted by service wrappers - repaired in source

When `source_ceiling` moved to `load_retained_session_metadata`, Meta's
`delegate_mob_session_service!` and `AfterCreateMobSessionService` still
forwarded only ordinary metadata reads. They therefore inherited the new
upstream method's `Unsupported` default. The normal persistent constructor
installs `PreBuildMobSessionService`, so every inherited fork/spawn document
route through that constructor failed with `AuthorityUnavailable`.

This was reported during the re-review and the implementation owner added
exact pass-through methods to both wrappers. The additions are visible at
`mob_handle_runtime.rs:6840-6844,7949-7953`. This finding is source-closed;
the real fork and retired-parent acceptance paths still need to pass against
the new upstream dependency.

### P2 acceptance fixture: Policy transition writes the compatibility store

At review time, `extension_native_source_policy_is_current_and_only_narrows_data_access`
changed `SqliteSessionStore` metadata at `tests/extension_state.rs:939-945` and
`969-971`. The resolver now uses the retained metadata seam, which reads the
RuntimeStore's committed authority. Upstream
`meerkat-session/src/persistent.rs:15641-15667` explicitly excludes a
SessionStore-only row. The fixture therefore does not perform the policy
transition that its assertions claim to test.

The implementation owner agreed to change the fixture to commit through the
authoritative runtime store. A wrapper returning test metadata can separately
test resolver composition, but does not prove production policy changes are
observed. Preserve the exact shared runtime-store facade when mutating this
fixture; directly reopening its database bypasses the facade's write-epoch
assumption. No execution result is claimed here.

The `late-create`/`late-reopen` subprocess fixture now establishes retained
provisional identity intent, commits an actual session without publishing its
exact continuity binding, deletes the live provisional record, and requires
`AuthorityUnavailable` both before and after a fresh process. It also checks
an ordinary unreserved Worker. Together with the dropped-waiter unit fixture,
this exercises the relevant failure-state predicates. Compilation and runtime
results remain pending. This re-review made no production changes and ran no
builds or tests.

## Final focused source pass at Meta c4e60f1e936a934608de02e167226bbfa706b947

No new production defect was found in this bounded pass. The prior retained
metadata forwarding repair is present in both wrapper implementations
(`mob_handle_runtime.rs:6855-6859,7975-7979`). The source-policy fixture now
retains the exact runtime-store facade and commits through it
(`extension_state.rs:659-661,718-731`), closing the earlier fixture defect in
source. Stable Agent identity still requires exact-session history; a reused
Worker name gets its creation-token principal, and the root stable-target
exclusion prevents failed publication from silently becoming a Worker.

The reference downstream adapter's authority failure is an infrastructure
result:
`Failure::AuthorityUnavailable` becomes the fixed
`ToolError::ExecutionFailed` message, and the caller resolves before any
document access (`src/native.rs:204-219,281-325`). Missing documents and denied
document access keep the same generic unavailable response. Empty create
skips the nonempty edit-batch requirement, but still constructs and validates
the document through serialization before committing
(`src/native.rs:414-423,571-579`; `src/engine.rs:314-320`). Receipt lookup still
precedes state-dependent work, commit-time replay drops fresh selections, and
the read-failure recovery only returns a provider-authorized receipt.

### Required acceptance fixture: native delegation across a mob boundary

Current native tests fork with `fork_member` and spawn with a host-supplied
creation witness on the same mob handle
(`tests/extension_state.rs:342-364`). Those tests do not establish that an
actual agent's `delegate` call delivers the registered bundle into its child
mob, captures the parent from the executing session, and registers the child
handle before its first native extension-probe dispatch.

Source wiring exists: Meta supplies the shared dispatcher and pre-activation
callback to `MobMcpState` (`unified_runtime/builder.rs:1953-1973`). Upstream
`configure_builder` applies them for child create and restore
(`meerkat-mob-mcp/src/lib.rs:1463-1478,1671-1675`); child definitions receive
the available bundles at lines 1888-1893. Native delegation captures creation
source from the bound parent session (`agent_tools.rs:1073-1079`), and the
resolver follows each source's recorded mob/session/creation identity using
the registry (`extension_state.rs:254-270,519-565`). This inspection supports
the intended route but is not runtime evidence that it works.

The smallest useful additional fixture is one scripted native flow:

1. A parent owns a document and invokes the actual `delegate` tool. Assert the
   helper's native binding has a different mob id and its first dispatched
   extension-probe call resolves its own principal. Default owner reach denies the
   spawned child until the owner explicitly shares.
2. Share Reader access with that child using its resolved principal and Forks
   reach. Have the child invoke actual `fork_off`; its fork must read the same
   document, and neither child nor fork may edit or administer it. This checks
   both the cross-mob Spawn edge and the subsequent native Fork edge without
   manufacturing `HostAccessContext` or lineage in the test.
3. Remove that grant and repeat reads through the same shared dispatcher;
   both child and fork must lose access. With a persistent helper/fork lifetime,
   reopen the runtime once and recheck a granted read followed by revocation,
   so child handle restoration and retained source policy are exercised.

Use the existing fake LLM and deterministic barriers to control helper
lifetime and interleave owner sharing; no live provider is needed. One fixture
can share the existing warm runtime suite. This is a missing acceptance path,
not a claim that the inspected cross-mob implementation is defective.

The parent reports that native metadata compilation and all external targets
compiled; real runtime execution is queued on GCP. This reviewer ran no builds
or tests and does not treat queued tests as passed.

## Implementation response to delegation acceptance scope

The actual native delegate lifetime narrows the requested restart case:
Meta currently composes implicit child mobs with in-memory journals, and
delegate completion retires the helper together with its fork descendants.
The new probe fixture exercises actual cross-mob delegation, native fork_off,
private denial, explicit Reader sharing and revocation while those members
are alive. It then asserts their native retirement and reopens the persistent
host to verify the original document and revoked ACL. It does not claim the
ephemeral child mob or its retired fork can resume. Persistent root-mob
fork/restart acceptance is a separate route. This clarification preserves
native storage and lifecycle semantics instead of altering them for a test.

The integration owner added explicit SDK-hosted history refusal before
extension activation, documented the one-way local history triggers and
unconfigured ABAC behavior, and retained exact released version requirements
with a workspace-only pinned upstream patch. Runtime execution of the new
acceptance fixtures remains pending; these source changes are not an
independent test verdict.

## Runtime findings and owning-domain repair

The `f8424fafb` GCP run compiled the host tests, then passed 9 of 11 selected
unit tests and 4 of 7 integration tests (one subprocess helper was ignored
as intended). Identity-history activation failed because the independent
history schema oracle could not create triggers on another domain's
`continuity_records` table. Two unit and two integration failures shared
that defect. The delegation fixture also lacked the tool producer's native
Rust-bundle provenance. The earlier source reviews did not catch these
runtime failures.

The implementation response moves history into the owning continuity domain
as explicit version 3 opt-in. Its initializer owns every trigger target;
backfill and the version stamp commit together, or roll back together on
conflicting identity bindings. Frozen version 1/2 verifiers and head-only
migrations preserve the earlier storage contracts. Older binaries refuse
an opted-in database, while current feature-off binaries verify its complete
schema and preserve retention triggers. This prevents a downgrade interval
from invalidating the native-birth coverage proof.

Five new unconditional schema tests cover these boundaries. The native test
producer now declares its actual Rust bundle. The native fixtures also inspect
model-visible denial, continued permitted sibling calls and a real restricted
source's fork policy. Root and a separate implementation agent found no
concrete issue in bounded source reviews of the schema repair; neither ran
builds or tests during those reviews. Successor execution and lint remain
pending. This implementation response records the repair and failed run,
not an independent runtime acceptance verdict.
