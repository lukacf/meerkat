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
  An installed host's close hands its own grant owner back the same way,
  once the host has settled.

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
