# Changelog

All notable changes to Meerkat Connect are documented in this file.
The format is based on [Keep a Changelog](https://keepachangelog.com/).

## [Unreleased]

## [0.9.11] - 2026-11-09

Meerkat Connect 0.9.11 is part of Meerkat 0.9.11 (Core 0.9.11, Meta 0.9.11
and Connect 0.9.11, released together): GitHub release `connect-v0.9.11`, a
prerelease that is never the repository's latest.

### Changed

- Application setup refuses a definition whose profile names
  `auth_binding` (the profile key Meerkat 0.9.11 adds), in TOML or native
  form, and refuses a retained setup whose definition names one when it is
  read again: which binding a member's credentials come from is the
  operator's choice, not the application definition's.

- A reply, message or request that a member's governed work sends to another
  member in the same process now runs under the sending work's requester.
  Connect binds the receiving work with the sending member as its ingress
  actor and every work of the sending work as a contributor, as Core 0.9.11
  requires, and each operation of the receiving work is narrowed by the
  sending work's current grants for that target. Host-triggered deliveries,
  deliveries from members that run no governed work and deliveries over a
  network are bound as before, under the receiving member's own standing
  grants.

- Connect refuses, before it records anything, a governed delivery it cannot
  bind so: one whose sending work is in another realm or ingress domain, or
  whose origin names another agent than the member that delivered it
  (`origin_unverifiable`); one whose sending work has a host-policy or
  service-mandate original (`sender_basis_unbounded`); one whose sending work
  names more than 64 works (`contributor_bound_exceeded`). The delivery ends
  rejected.

- Every Toolkit binder answers Meta's typed bind contract: definite refusals
  are typed (`denied`, `closed`, `unavailable`; the two capture doors answer
  `withheld`). The shared original binder and the helper submission and
  preparation answer "not now" at a held source collection only before their
  attempt's first durable write; past it they keep their shipped outcome. The
  helper preparation's own wait-and-bind-again loop is removed: Meta's send
  loop waits outside the identity's lock.
- The peer ingress binder answers a delivery whose binding is pending as
  pending instead of waiting for it inside its bind, and records the peer
  source record as its attempt's first durable effect. Where the shared
  original binder still waits in place inside a bind in this release (its
  checks after a first durable write, its custody recovery enrollment), a
  peer bind waits there too.
- A binder's input check answers "not now" only inside a governed bind,
  before its attempt's first durable write; called outside one it keeps its
  shipped outcome.
- A workflow launch waits out its binder's "not now" answers only within its
  dispatch's deadline (its request deadline or its remaining tool time,
  whichever ends first, within Core's owner-change bound), as every other
  wait of the launch.
- A setup acquisition's admission that reads its original's source facts in
  conflict (contention: another writer's change in flight) waits it out
  through Core's owner-change waiter, within its deadline and the dispatch's
  remaining tool time, and reads again; a conflict that outlasts the wait is
  answered as unavailable (retryable). It was answered as `access_denied`
  ("not allowed by policy"), although no policy refused it.
- A Find's work step answers contention as unavailable (retryable), never as
  `access_denied`: an owner change its helper's first binding check meets
  (another original's first binding, for example) is waited out through
  Core's owner-change waiter and the bind made again, and one that outlasts
  the wait is unavailable. The workflow call gate, plan staging and the
  setup report tool answer a conflict in a source's facts as unavailable too.
  Each answered `access_denied`.

- `@rkat/toolkit` accepts any `@rkat/sdk` and `@rkat/meta-sdk` from 0.9.10
  up to, but not including, 0.10.0 as optional peers (`>=0.9.10 <0.10.0`;
  each was one exact version), so it installs next to the 0.9.11 SDKs
  without a peer conflict. Its development dependencies and lock keep the
  published SDK versions it is built and tested against.

### Fixed

- Service finalization reads its retained documents (the receipt, the MCP
  document, the selected configuration and the selected service inputs) only
  from a regular file of at most 1 MiB. A symbolic link, a FIFO or another
  special file at that path is now refused before any read, so finalization
  no longer waits on a FIFO that has no writer.

- Setup: after a reviewed setup conversation closed, the setup that
  continued in the same process could reopen the realm with its governed
  work and configured MCP connections unavailable, or have the realm's
  grant owner refused when the setup finalized, while tasks of the closed
  conversation were still ending. The close now hands the realm's grant
  owner back, so the finalization and the continuation restore it at once.
  An installed host's close hands its own grant owner and its originals
  store back the same way, as its last step, once every task of the host
  that can still use them has finished: the host's admitted work and its
  presenters are joined first (within 30 seconds), so work admitted before
  the close completes instead of being refused, and a host reopened in the
  same process opens both at once instead of being refused while tasks of
  the closed host are still ending. After a host loss the close hands both
  back as soon as it settles, and work still running then fails.
  `SqliteStore::release_custody` (in `toolkit-store`) ends a store's
  physical custody for every clone, after any operation already running on
  it, and unlocks its owner file before closing it; `SqliteStore::custody`
  is that custody's handle. `NativeHostLifetime::bind_native_grant_custody`
  and `NativeHostLifetime::bind_native_store_custody` (in `meerkat-toolkit`)
  name the grant owner and the store a host's close releases, and
  `PrivateHostLifetimeControl::release_native_custody` is that release.

### Security

- `zip` is updated from 2.4.2 to 9.0.4 for RUSTSEC-2026-0367,
  RUSTSEC-2026-0368, RUSTSEC-2026-0369 and RUSTSEC-2026-0429. They affect
  `zip`'s archive extraction functions, which Connect never calls: it reads
  MCPB bundles with its own bounded ZIP32 reader. `zip` is linked only
  through `toolkit-package`'s `local-artifacts` feature, which
  `toolkit-mcpb-install` uses; `meerkat-connect` and `toolkit_gateway` do
  not link it.

### Upgrade notes

- Downgrade: the source row of a governed peer delivery records the sending
  work's requester, agent and works. A Connect without this change refuses to
  open a source store that holds such a row. Rows written before the upgrade
  are unchanged and read as before.

### Known limitations

- After a host loss, or a close that ran out of its 30-second join bound, an
  access decision whose native issuance was still pending stays pending: the
  next host does not settle it, and a fresh approval of the same proposal is
  refused as a conflict. Make a new proposal for that access instead.

- A member's work cannot answer a reply, message or request that another
  member's governed work sent it in the same process. Each operation of the
  work that such a delivery starts must also be allowed by the sending
  work's grants, and a member's standing grants allow only replies to that
  member's own sender and writes to its own Recorder. So the receiving work
  runs its model, but Core refuses its replies, its other peer sends and its
  Recorder writes. Work started by a message the host sends to a member is
  not limited this way.

- Only in-process deliveries carry the sending work. A delivery over a network
  is bound under the receiving member's own standing grants.

- Under sustained heavy CPU load, attaching a native service can stall
  before all of its members are attached. Each readiness check spends its
  250 ms budget on a currentness check before it waits for the member, so
  while that check alone takes longer than the budget, the member is not
  seen as ready and the members after it are not attached until a later
  check fits within the budget. This behavior is not new in 0.9.11.

## [0.9.10] - 2026-11-05

Meerkat Connect 0.9.10 is part of Meerkat 0.9.10 (Core 0.9.10, Meta 0.9.10
and Connect 0.9.10, released together) and the first Connect release from
this repository: GitHub release `connect-v0.9.10`, a prerelease that is never
the repository's latest. Connect 0.9.0 and 0.9.4 and the Linux component
prereleases 0.9.5 and 0.9.6 were released from `lukacf/meerkat-connect`;
there are no Connect releases numbered 0.9.7 to 0.9.9.

### Breaking

- Connect's paired-host contract no longer names one host product. Every
  option, tool, field and stored name that carried that product's name now
  says `paired-host` or `paired_host`: among them the `--paired-host` and
  `--paired-host-memory-only` options, the `paired_host_*` tools and pairing
  fields, the pairing cookies (`paired_host_pair`, `paired_host_device`), the
  prepared-change digest prefix (`paired-host-change-v1:sha256:`), the device
  credential's OS keyring service id
  (`ai.rkat.meerkat-connect.paired-host-device.v1`) and item label ("Meerkat
  Connect paired host device"), and the Python SDK's live-image extra and
  module (`paired-host-live`, `meerkat_toolkit.frontend.paired_host_live`). A
  host, device or script that uses the earlier names needs the steps in
  Upgrade notes.

### Changed

- Connect builds in the single Cargo workspace of the Meerkat repository.
  Its six crates are members of the root workspace: they inherit its
  `rust-version` (1.97.0; the standalone Connect workspace declared 1.94.0)
  and take Core and Meta at the workspace's exact pins, and the standalone
  `connect/Cargo.toml` and `native-candidate/` workspaces are gone. Build and
  test from the repository root, for example
  `./scripts/repo-cargo build -p meerkat-connect`.

- The bundled discovery catalog (revision `discovery-2026-10-28.1`) no longer
  carries one paired host's device extension recipe, and its Telegram recipe
  is a generic bot frontend that points at the Python SDK's public frontend
  modules instead of one host's extension package. It holds 9 discovery
  recipes; together with the integration library it ships with, the catalog
  holds 110 records.

- A Find whose setup helper submission native admission refuses because an
  authorization change landed under it now rebinds after each settled wait
  within the owner-change bound, instead of once. Every rebind keeps the same
  input and rechecks the source, the admitted helper and its selection. The
  Find answers `retry_safe` (nothing admitted) only when the changes outlast
  that bound; a fresh Find of the same run then replays the helper input.

### Fixed

- Setup: when a setup check answered again for a validation it had already
  accepted, with something that is not a valid proposal (for example a file
  list that names both a declaration and an activation document), the check
  was refused but the selection made from the earlier proposal stayed in
  force. Such an answer now withdraws the earlier proposal, as a changed
  answer already did, so a selection made from it no longer applies.

- The files Connect keeps through Meerkat's file approval store (its
  approval store and `native-decisions.json`) are created owner-only
  (`0600`) on Unix whatever the process umask, so Connect's private-file
  checks no longer refuse a store created under the common 022 or 002
  umask. A store file an earlier version left readable by others becomes
  private on its next commit (Core's `FileApprovalStore` fix in Core
  0.9.10).

- Provider setup and first-target setup read a realm's model targets as
  Meerkat resolves them, so a realm whose GitHub Copilot account is
  configured once, by a `[realm.<realm>.copilot]` section, lists the
  account's routes (`copilot_openai`, `copilot_anthropic` and
  `copilot_gemini`) as configured targets, and first-target setup is refused
  next to the section. For a model whose provider family Meerkat knows
  locally (a catalog model or a `[models.<id>]` entry), the lists of
  existing targets, in the terminal and in the private-pipe setup
  conversation's provider prompt, offer only the targets a session for that
  model runs on, and the chosen target's details show the route that
  session runs on; for a model only the Copilot account lists, see Known
  limitations. Before, such a realm looked unconfigured there, and the lists
  offered each Copilot route for a model of one provider family: choosing
  `copilot_openai` for an Anthropic model ran the session on
  `copilot_anthropic`.

- A Find whose own entry meets an authorization change that another task is
  making no longer fails as `operation_refused` with kind
  `reprepare_required`. The Find entry waits out the concurrent change within
  the attempt's owner-change bound and then enters once, spending its review
  once. A change that outlasts the bound is still refused as
  `reprepare_required`, before anything ran. At review tier R2 such a change
  settles the Find as `review_unsatisfied` (`context_changed`) instead, by
  design: nothing runs under a decision that was not reviewed.

- Setup calls no longer fail at once when they meet an authorization change
  that another task is making: a setup helper's report, a reviewed install,
  `setup_state`, `setup_stop`, a shared setup's entry reservation and a
  setup workflow's call. Each waits for the change within the owner-change
  bound and checks again. A call that holds a reserved entry or a prepared
  review releases it first and starts that step over, checking everything
  afresh. A change that outlasts the bound is still refused as
  `operation_refused` with kind `reprepare_required`. A setup workflow's
  call no longer reports that refusal as access denied.

- A setup call refused that way before any of its work ran now says so:
  the refusal carries `tool_invoked: false`, so the call can be retried
  safely. This also covers a reviewed install that meets such a change just
  before it is sent, which is still refused at once. A refusal after part
  of the call ran still says nothing about whether it ran.

- A reviewed install refused that way at its shared setup step, after its
  review but before anything was sent, can now be retried. The install is
  recorded as failed rather than uncertain, and its checked installation
  stays available, so a fresh install of it asks for its own review. A
  refusal without `tool_invoked: false` still leaves the install uncertain
  and refuses its retry.

- A selected setup task's call (for example a reviewed install's shared
  setup step, a shared setup's check, join or stop, a setup helper's
  report, an account or public-service request, or a software review) no
  longer waits for as long as another task holds an authorization change
  while its task is prepared. The preparation now ends at the owner-change
  bound, including its reads of the setup store that wait behind a write
  held up by that change. If the change is still in progress then, the
  call is refused as `operation_refused` with kind `reprepare_required`
  and `tool_invoked: false`, before any of its work ran, so it can be
  retried; otherwise it answers temporarily unavailable.

- A Find no longer fails as temporarily unavailable (`tool_unavailable`)
  when it reads its setup helper's admission while another original's
  first binding is changing the shared authorization. The read waits out
  that change within the owner-change bound and is made again. With no
  change in progress the refusal still answers at once.

- A Find no longer fails as temporarily unavailable (`tool_unavailable`)
  when its setup helper's admission, grants or source binding meet another
  owner's brief hold on the shared source state, or a change of the shared
  authorization in progress. The checks made before each of those steps,
  and the reads made after them, wait that out within the owner-change
  bound; no step is made twice. A hold that outlasts the bound still
  answers `tool_unavailable`, and nothing the Find already did is made
  again.

- A Find is no longer refused as not allowed by policy (`access_denied`)
  when another original's setup helper changes the shared authorization
  while the Find retains or recovers its own helper's source. That change
  is waited out within the owner-change bound and the helper's grant
  resolved again; a change that outlasts the bound answers
  `tool_unavailable`, and only a real refusal is a denial.

- A Find no longer fails as temporarily unavailable (`tool_unavailable`)
  when its setup helper's source binding is checked and published, or its
  bound source recovered, or its helper observed, during another owner's
  brief hold on the shared source state. The check or the publication
  waits that out within the owner-change bound; the binding is never
  written twice, and a hold that outlasts the bound still answers
  `tool_unavailable`.

- A Find no longer fails as temporarily unavailable (`tool_unavailable`)
  when another owner's brief hold on the shared source state meets its
  setup helper's admission right after its create (or the create's
  replay), its helper's observation right after its read, its grant
  issuance before or after the grant intent is written, its first source
  binding write, its helper's preparation or submission, or a
  contributor's admitted-source read (also in a selected setup task's
  result source). Each step waits that hold out within the owner-change
  bound, and the grant issuance also waits for another writer of the
  source state; nothing the Find already did is made again. A hold that
  outlasts the bound still answers `tool_unavailable`, except at the
  submission's check before it binds again after an authorization change:
  there it ends the Find as `retry_safe` (nothing admitted), as a change
  that outlasts the bound does.

- A Find no longer fails as temporarily unavailable (`tool_unavailable`)
  while another original's Find is writing the shared source state. Its
  setup helper admission waits for that write within the owner-change bound;
  a write that outlasts the bound is still refused as temporarily
  unavailable, with nothing admitted.

- A shared setup helper is no longer refused as unavailable policy while
  another task briefly holds its original's in-memory source state. Native
  reads of that state now wait for such a short hold, within the bound
  Meerkat uses for owner changes, and still refuse at once when the state is
  poisoned, stopped or withdrawn.

- Cancelling an original's operations, a follow-up of a shared setup and a
  setup helper's recovery step no longer fail as unavailable while another
  task briefly holds the original's source state or writes it. They wait for
  that holder, within the same bound, and never while an authorization
  change is in progress; a holder that outlasts the bound is still refused as
  unavailable.

- A setup result delivered back to its original through a retained resume no
  longer stays pending when another task briefly holds the original's source
  state at that moment. The resume waits for that holder within the same
  bound.

- A shared setup check whose call meets an authorization change in progress
  right after the check ran now waits for the change and checks again,
  instead of being refused. Only the authorization check repeats; the check,
  its join and its release run once. A change that outlasts the bound is
  still refused as `operation_refused` with kind `reprepare_required`, and
  the refusal does not say whether the check ran (`tool_invoked` stays
  unknown).

- A service task check that meets an authorization change still in progress
  now waits for the change and checks again, instead of refusing the call as
  access denied. Only the check repeats; the task is prepared once. A change
  that outlasts the bound is refused as `operation_refused` with kind
  `reprepare_required`: the call needs a fresh, current authorization. The
  refusal does not say whether the call's work already ran (`tool_invoked`
  stays unknown), so a retry follows the caller's own operation, idempotency
  and settlement rules, never a blind replay of the whole call. A real denial
  is still refused at once.

### Upgrade notes

- Build Connect with Rust 1.97.0 or newer, from the repository root. An
  application that embeds Connect's crates pins Core and Meta at `=0.9.10`,
  the versions Connect 0.9.10 is built with.

- Connect's SDKs are release assets, not registry packages: the Python SDK
  `meerkat-toolkit` 0.9.10, whose `native-host` extra requires
  `meerkat-meta==0.9.10`, and the TypeScript package `@rkat/toolkit` 0.9.10,
  whose development and optional peer dependencies are `@rkat/sdk` 0.9.10
  and `@rkat/meta-sdk` 0.9.10. Install them from the `connect-v0.9.10`
  release.

- Move paired hosts, devices and scripts to the new paired-host names: pair
  each device again (its stored credential sits under the earlier keyring
  service id, and the earlier pairing cookies are not read), prepare again
  any change prepared before the upgrade (its digest prefix differs), install
  the `paired-host-live` extra in place of the earlier one, and update host
  and agent configuration that names the earlier options, tools or fields.

- Going back to a Connect built on Core 0.9.8 or older (Connect 0.9.0 to
  0.9.6) can be refused. Connect 0.9.10, on Core 0.9.10, can write sessions
  at session envelope 5 and mob journals at mob event schema 10, which Core
  0.9.8 and older refuse (see Core's and Meta's Upgrade notes). Before such a
  rollback, run
  `rkat storage doctor` and handle the sessions and mobs it lists under
  `operator-instruction-envelope` and `operator-instruction-mob-events` as
  Core's CHANGELOG describes.

### Known limitations

- For a GitHub Copilot model that only the account lists (absent from
  Meerkat's model catalog and from `[models.<id>]` configuration), the lists
  of existing targets in the terminal and in the private-pipe setup
  conversation still offer every Copilot route of the account: setup makes
  no provider or account-model request, so it cannot tell the model's
  family. When the session starts, it runs on the account's route for the
  protocol the account's model list gives the model, on the same account,
  which can differ from the route chosen in setup.

- Under contention, a first submission, a setup helper's or an original's,
  can be refused while its input is bound for native admission: when it
  meets another submission still in flight in the same namespace, or when a
  setup helper's wait budget runs out. The refusal is retryable and nothing
  runs; a retry after the other submission settles succeeds.

- A Find whose contributing original has an input that is neither admitted
  nor awaiting admission (for example one whose admission failed or that
  was withdrawn) can be refused as not allowed by policy (`access_denied`)
  at its work step, although no policy refused it. This is not fixed in
  this release.

- Under contention, binding an original's or a setup helper's input for
  native admission can wait longer than 2 s for the source's shared state,
  for another write to the same source, or for a recovery round.

- A send refused at its entry checks can leave an internal pending attempt
  until the next send of the same input or a restart; nothing is admitted
  without passing the checks again.
