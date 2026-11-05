# Changelog

All notable changes to Meerkat Connect are documented in this file.
The format is based on [Keep a Changelog](https://keepachangelog.com/).

## [Unreleased]

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
