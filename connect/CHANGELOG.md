# Changelog

All notable changes to Meerkat Connect are documented in this file.
The format is based on [Keep a Changelog](https://keepachangelog.com/).

## [Unreleased]

### Fixed

- Setup: when a setup check answered again for a validation it had already
  accepted, with something that is not a valid proposal (for example a file
  list that names both a declaration and an activation document), the check
  was refused but the selection made from the earlier proposal stayed in
  force. Such an answer now withdraws the earlier proposal, as a changed
  answer already did, so a selection made from it no longer applies.
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

### Known limitations

- Provider setup and first-target setup read a realm's explicit tables only,
  so they do not yet list the routes a `[realm.<realm>.copilot]` section
  declares (Meerkat's GitHub Copilot account configured once), and a realm
  whose Copilot account is configured only by that section can look
  unconfigured there. Use the explicit Copilot tables with Connect until a
  later release. With the explicit tables, Connect can offer each of the three
  Copilot routes as a target for a model of one provider family, and the
  session runs on the account's route of the model's family whichever is
  chosen, on the same credential: choosing `copilot_openai` for an Anthropic
  model runs on `copilot_anthropic`, so the chosen target's name is not the
  route the session runs on.
- After a host loss, or a close that ran out of its 30-second join bound, an
  access decision whose native issuance was still pending stays pending: the
  next host does not settle it, and a fresh approval of the same proposal is
  refused as a conflict. Make a new proposal for that access instead.
