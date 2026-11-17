Original accepted Meta candidate; see the attached selections and provenance. Linux x86_64 and aarch64 and macOS arm64 archives are listed in index.json and checksums.sha256. macOS x86_64 and Windows archives may arrive later, listed in index-trailing.json and checksums-trailing.sha256.


## Known limitations

The known limitations of Meta 0.9.15 are in the release's own notes: section 0.9.15, Known limitations, of meta/CHANGELOG.md at this tag (https://github.com/lukacf/meerkat/blob/meta-v0.9.15/meta/CHANGELOG.md). Two were added late in the release and are worth reading before an upgrade: `ask_user` in `tools.deny` for a member whose host decides its tool surface, and voice (GPT Live) through the Meta SDKs over the stdio gateway.

- Added 2026-11-16, after the release. Approvals: a change that a rule holds
  for approval can be decided only by a person whom the approver map of the
  gateway's access file binds to the rule's approver id as the stdio
  operator or as a person a declared ingress attests. A binding that lists
  only Console subjects or groups lets nobody decide: the Console serves no
  `meta/decisions/decide`. The map is read whether or not the access file
  has `enabled = true`. When the gateway was started with no access file, or
  with a map whose binding for the rule's approver id lists neither the
  stdio operator nor a person a declared ingress attests, the change is
  held, a decide is refused (`-32602`, `decision_refused`, `not_eligible`),
  and nobody can decide it. Until its deadline nothing is lost: start the
  gateway again before the deadline with an access file whose approver map
  binds the id to the stdio operator or to a person a declared ingress
  attests, and the held change is presented and can be decided. Left
  undecided, it ends at its deadline: the operation is refused
  (`decision_closed`), nothing is changed, and the change has to be asked
  again. Run on the released 0.9.15 gateway for a host's create under a rule
  that requires approval, decided by the stdio operator, with no access file
  and with a map that binds another id only, and for the deadline with that
  rule's decision timeout set to five seconds; a binding that lists only
  Console subjects is read from the gateway's tests at the tag, not run on
  the released gateway; not run: a decide attested by another declared
  ingress, a held change of another kind, and an edit of the map without a
  restart. See
  [Approval rules](https://docs.rkat.ai/meta/reference/identity-policy#approval-rules).

- Added 2026-11-16, after the release. Agents created at run time: where a
  rule requires approval for a create, a create under a name that is already
  taken (an agent of the roster, an agent created earlier, the name of a
  removed agent) is not refused at once. It is held for approval,
  `meta/identity/plan` answers `require_approval` for it, and it is refused
  only when the approved change is carried out: the operation then answers
  `refused` with the kind `decision_closed`, and its text names `name_taken`
  and says "ask again" although the refusal is not retryable
  (`retryable: false`). Nothing is created and no agent is changed. The
  pages said such a create is refused `name_taken` without this condition.
  Before approving a create, check its name against `meta/identity/list`
  with `include_archived`: an archived agent holds its name too. The list
  does not show a removed agent's name, so a create under one is found only
  when the approved change is carried out. Run on the released 0.9.15
  gateway for the three kinds of name taken before the create, with the
  approver bound to the stdio operator; a name taken between the hold and
  the approval ends the same way, by a run on a build of this release's
  source. A fix is planned for the next release.

- Added 2026-11-16, after the release. Python and TypeScript SDKs
  (`meerkat-meta`, `@rkat/meta-sdk`), with a gateway the SDK starts itself:
  when the gateway refuses a start after it has accepted the init request (a
  mob definition that differs from the stored one, or a declared update
  whose expected revision is stale, is such a refusal), `builder.build()`
  does not raise the refusal's own exception, although the SDK pages show a
  handler for `CompositionProvenanceError` around it. It raises
  `GatewayStartupCleanupError` (a `TransportError` in both SDKs), whose
  message says that the stop of the gateway transport failed; the refusal is
  its `startup_error` and `__cause__` in Python, and its `startupError` and
  `cause` in TypeScript. So a handler for `CompositionProvenanceError` or
  for `RpcError` does not match, and a host that retries transport errors
  retries a start that can never pass. Read the startup error of a
  `GatewayStartupCleanupError`, and do not retry when it is a typed refusal.
  A host's own gateway transport whose stop succeeds raises the refusal
  unchanged; that path was not run. Nothing is left behind: the refused
  gateway has exited, and a next start with the matching definition works.
  Run on the released 0.9.15 gateway with the `meerkat-meta` 0.9.15 and
  `@rkat/meta-sdk` 0.9.15 packages for a mob definition that differs from
  the stored one (the next start with the Python SDK); that no file under
  the state directory stays held, and that the next start needs no cleanup
  call, by a run on a build of this release's source with the Python SDK. A
  stale declared revision is masked the same way, by a run on a build of
  this release's source; no other refusal of a start was run. The Python
  path is the same in 0.9.11 (read by lines), and by a search in 0.9.9,
  0.9.10 and 0.9.12 to 0.9.14; earlier TypeScript releases were not read.

- Adopted agents on a host with an agent customizer or agent memory (added
  2026-11-16). On a host that registers an agent customizer or configures
  agent memory, an agent that is configured in the roster and adopted into
  the identity store (`meta/identity/adopt`) is refused the next time it is
  built: after a restart, or when a message, a peer send or a scheduled
  prompt reaches it while it is not live. An agent that is live at the
  adoption goes on serving until then. The agent is refused for good as it
  stands and reads `broken`; the reason names label keys (`agent_identity`
  and `profile_name` among those dropped), and the agent then serves
  nothing. Its conversation and its record are not changed. Not affected: a
  host that registers no agent customizer and configures no agent memory,
  and an agent created with `meta/identity/create`. One committed update of
  the agent (`meta/identity/update`, as a new request; it may set a value
  the agent already has, its display name for example) lets it build again
  on the same conversation, and it stays so across restarts. The update must
  commit: where the host's identity policy requires approval for it, the
  agent builds again once the update is decided; where the policy denies the
  host a `descriptive` change, allow it first. Until the fix, follow each
  adoption on such a host with such an update. A fix is planned for 0.9.17.
  (The cause by reading the 0.9.15 source; the remedy for a refused agent by
  a host's run on the published 0.9.15 gateway; the update right after an
  adoption by reading.)

- Side chats and the memory tool after a restart (added 2026-11-16). With
  agent memory on and the `memory` tool in the agent's profile, a side chat
  is not offered the `memory` tool after a gateway restart until its source
  agent has taken a turn in the new process. A call to the tool meanwhile
  answers that the tool was not found, and the gateway logs nothing about
  it. A side chat that existed before the restart is offered the tool once
  the agent has taken its turn. A side chat that is forked after the restart
  and before the agent's turn stays without the tool in that process, also
  after the agent's turn; a further restart and the agent's turn give it the
  tool. After a restart, let the source agent take one turn before its side
  chats are used. (By a run on the published 0.9.15 `meta-rpc-gateway`
  archive for Linux x86_64 with a scripted model; the other platforms and
  `meta_gateway` were not run.) A fix is planned for 0.9.17.

- Added 2026-11-17, after the release. TypeScript SDK: when a gateway that the SDK started refuses the start after it has accepted the init request, and exits, the Node program can end with an unhandled `write EPIPE` error instead of `builder.build()` rejecting. In the maintainers' runs on Linux with Node 22 that happened in 7 of 8 starts; in the others `build()` rejected with `GatewayStartupCleanupError` as documented. A refusal that the gateway gives as its answer to the init request is not affected. Until it is fixed, install a `process.on("uncaughtException", ...)` handler before calling `build()` and remove it with `process.off` once `build()` has settled. In the handler, ignore only this error (`error.code === "EPIPE"` and `error.syscall === "write"`); for any other error, print it and exit with a non-zero code, as Node does without a handler; do not throw from the handler. The `EPIPE` then arrives in the handler and `build()` rejects with `GatewayStartupCleanupError` (in 10 of 10 starts on the published 0.9.15 on Linux with Node 22, where the refusal was a missing console configuration file; the `EPIPE` arrived in the handler in 9 of them; not run on macOS or on Windows). While it is installed the handler also swallows a `write EPIPE` on the program's own output. A fix is planned.

- Added 2026-11-17, after the release. Peer requests between mob members: on a host whose realm is on disk, a member that has sent a peer request (`send_request`) stops running when a response to that request arrives without a result (`send_response`, or a host's or SDK caller's `peer_response`, with no `result`); after the next restart of the host the member can be refused for good. This release has it too (shown by runs of the published 0.9.15 `rpc_gateway` with a real model: stopped in 3 of 3 with `accepted` without a result, not stopped in 2 of 2 with `completed` with a result). The entry of the same date on the Meta 0.9.16 release page (https://github.com/lukacf/meerkat/releases/tag/meta-v0.9.16) says what an operator sees, which hosts are affected and what not to do; it applies to this release as written.
