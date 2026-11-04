# Changelog

All notable changes to Meerkat Connect are documented in this file.
The format is based on [Keep a Changelog](https://keepachangelog.com/).

## [Unreleased]

## [0.9.9] - 2026-11-03

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
  `rust-version` (1.97.0; Connect's own workspace declared 1.94) and take
  Core and Meta at the workspace's exact pins, and the standalone
  `connect/Cargo.toml` and `native-candidate/` workspaces are gone. Build and
  test from the repository root, for example
  `./scripts/repo-cargo build -p meerkat-connect`.

- The bundled discovery catalog (revision `discovery-2026-10-28.1`) no longer
  carries one paired host's device extension recipe, and its Telegram recipe
  is a generic bot frontend that points at the Python SDK's public frontend
  modules instead of one host's extension package. It holds 9 discovery
  recipes; with the adopted library the combined catalog holds 110 records.

### Fixed

- A setup check that answers for an already retained validation id with
  something that is not a valid proposal (for example, a file list naming both
  a declaration and an activation document) now invalidates the retained
  proposal, as a changed answer already did, so a selection made from that
  proposal is no longer current. Before, the check was refused but the earlier
  selection stayed current.

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

### Upgrade notes

- Build Connect with Rust 1.97.0 or newer, from the repository root. An
  application that embeds Connect's crates pins Core and Meta at the same
  exact versions as the repository state it builds Connect from.

- Move paired hosts, devices and scripts to the new paired-host names: pair
  each device again (its stored credential sits under the earlier keyring
  service id, and the earlier pairing cookies are not read), prepare again
  any change prepared before the upgrade (its digest prefix differs), install
  the `paired-host-live` extra in place of the earlier one, and update host
  and agent configuration that names the earlier options, tools or fields.

- Going back to a Connect built on Core 0.9.8 or older can be refused. A
  Connect built on Core 0.9.9 can write sessions at session envelope 5 and
  mob journals at mob event schema 10, which Core 0.9.8 and older refuse (see
  Core's and Meta's Upgrade notes). Before such a rollback, run
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
