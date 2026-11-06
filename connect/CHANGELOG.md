# Changelog

All notable changes to Meerkat Connect are documented in this file.
The format is based on [Keep a Changelog](https://keepachangelog.com/).

## [Unreleased]

### Breaking

- Console setup: only a signed-in person answers a setup prompt. The answer
  routes (`POST /host/toolkit/setup/start`, `/answer`, `/credentials`,
  `/cancel-sign-in` and `/callback`) refuse a caller that is not a verified,
  current person (no token on a console without sign-in, a service token, a
  token that names no person, a shared-account token, or a login that ended
  before the answer was applied) with `403` and
  `{"error": "approver_unauthenticated"}`, a new refusal kind (the one the
  Meta console uses for approval decisions); nothing is answered.
- Console setup: an answer from a login that has no open setup prompt is
  refused with `409`, whether or not another login's prompt is open. Before,
  it was refused with `403` while another login's prompt was open.

### Fixed

- Console setup approvals record the person who answered. The decision's
  actor is the person's qualified principal key
  (`principal:["human","oidc-issuer:<issuer>","<sub>"]`), and its provenance
  keeps the assurance (`person_authenticated`) and a digest of the login,
  never the token or the login itself. Before, every Console answer was
  recorded as the Console presenter (`connect-console-presenter`).
- Console setup: several people answer setup prompts at the same time. Each
  login has its own prompt, and one login's prompts come one at a time, in
  the order they arrived. Before, one person's open prompt held back every
  other person's.
- Setup: when a setup check answered again for a validation it had already
  accepted, with something that is not a valid proposal (for example a file
  list that names both a declaration and an activation document), the check
  was refused but the selection made from the earlier proposal stayed in
  force. Such an answer now withdraws the earlier proposal, as a changed
  answer already did, so a selection made from it no longer applies.

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
