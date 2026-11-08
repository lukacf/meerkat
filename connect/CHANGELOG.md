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

- Service finalization reads its retained documents (the receipt, the MCP
  document, the selected configuration and the selected service inputs) only
  from a regular file of at most 1 MiB. A symbolic link, a FIFO or another
  special file at that path is now refused before any read, so finalization
  no longer waits on a FIFO that has no writer.

### Security

- `zip` is updated from 2.4.2 to 9.0.4 for RUSTSEC-2026-0367,
  RUSTSEC-2026-0368, RUSTSEC-2026-0369 and RUSTSEC-2026-0429. They affect
  `zip`'s archive extraction functions, which Connect never calls: it reads
  MCPB bundles with its own bounded ZIP32 reader. `zip` is linked only
  through `toolkit-package`'s `local-artifacts` feature, which
  `toolkit-mcpb-install` uses; `meerkat-connect` and `toolkit_gateway` do
  not link it.

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
