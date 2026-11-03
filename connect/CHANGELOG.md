# Changelog

All notable changes to Meerkat Connect are documented in this file.
The format is based on [Keep a Changelog](https://keepachangelog.com/).

## [Unreleased]

### Fixed

- A setup check that answers for an already retained validation id with
  something that is not a valid proposal (for example, a file list naming both
  a declaration and an activation document) now invalidates the retained
  proposal, as a changed answer already did, so a selection made from that
  proposal is no longer current. Before, the check was refused but the earlier
  selection stayed current.

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
