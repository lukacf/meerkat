# 039 — Drive a Mob from the TypeScript SDK

An application creates a small mob over `rkat-rpc`, gives it work, follows
what its members do and stops work it no longer wants. Two members: an editor,
which is the one member the application talks to, and a researcher the editor
asks for facts. [038](../038-mob-control-py/) is the same program in
Python.

Use the 0.9.15 SDK and runtime or later (see
[Stop and cancel](#stop-and-cancel) for why).

## Concepts
- `client.createMob({ definition })` — profiles are the roles; only a profile
  with `external_addressable: true` takes work from outside the mob, and every
  profile needs `tools.comms: true`
- `mob.spawn({ profile, agentIdentity, initialMessage })` and
  `mob.wire(a, b)` — a member runs one first turn on its initial message, and
  `mob.waitForKickoffComplete()` waits for those turns
- `mob.member(identity).send(text)` — work goes to a member through the mob,
  not to its session
- `mob.subscribeEvents()` — one stream for all members; each item names the
  member (`source`) and carries the event (`envelope.payload`). Narrow the
  event with the guards the SDK exports (`isRunCompleted`, `isTextDelta`, ...)
- `mob.memberStatus(identity)` — status, tokens used, the last answer
  (`outputPreview`) and whether a run is open (`progress.run_state`)
- `mob.memberHistory(identity, { fromIndex: 0, limit: 1 })` — the first
  message of a member is its system prompt, with the skills the runtime gave it
- `mob.stopMemberRun(identity, runId, reason)` and
  `client.mobCancelAllWork({ memberRef })` — the two ways to end work

## What it does

```
create the mob, spawn editor-1 and researcher-1, wire them
  ↓
send a request to editor-1   → it messages researcher-1, then answers
  ↓
send a long job, stop that run            → the stop button
  ↓
send a long job and two more, cancel all  → the run ends, the queue is dropped
  ↓
send one more request        → the member is still there
  ↓
destroy the mob
```

The members decide who messages whom. The application sends work to the
editor and reads what happens; it does not relay between members.

A request to a mob is not one request and one response: the editor ends a
run when it has asked the researcher, and a new run starts when the reply
arrives. The script prints each run as it ends and moves on when every member
is idle. An application with a window keeps the stream open instead.

## Stop and cancel

| Call | What it ends | When |
|------|--------------|------|
| `mob.stopMemberRun(identity, runId, reason)` | The one run named by `runId` (from the `run_started` event, `identity.run_id`) and the inputs that run was working on. Queued input stays. | At once. A run that is no longer current is left alone and the receipt says `not_current`. |
| `client.mobCancelAllWork({ memberRef })` | The run of that member and every input it had queued. Input sent after the cancel is untouched. | At the next boundary of the run, which can be the end of the model response it is in. |

Use the stop for a stop button. Use the cancel to clear a member.

Three things to know before copying this:

- There is no cancel for one submitted unit: `mob/cancel_work` is always
  refused.
- On 0.9.5 through 0.9.14, `mob/cancel_all_work` could destroy the mobs owned
  by the session of the member when a turn sent straight to that session was
  waiting. 0.9.15 fixed it; see the
  [0.9.14 release page](https://github.com/lukacf/meerkat/releases/tag/v0.9.14)
  and [Cancel work](https://docs.rkat.ai/guides/mobs#cancel-work) in the mobs guide.
- From 0.9.15 one limit remains, listed under Known limitations in the
  [changelog](../../CHANGELOG.md) of 0.9.15: after a cancel, the first turn a
  client starts on the session of the member (`turn/start`) is answered an
  internal error and is not run; the turn after it runs. This example starts
  no turn on the session of a member: it sends through the mob
  (`member.send`), and that send runs.

## Run

With the released SDK in a directory of its own (see
[003](../003-hello-meerkat-ts/) for what it downloads and verifies on first
use):

```bash
mkdir mob-control && cd mob-control
npm init -y
npm install @rkat/sdk tsx
cp /path/to/meerkat/examples/039-mob-control-ts/main.ts .
ANTHROPIC_API_KEY=sk-... npx tsx main.ts
```

To run against this checkout instead, build the SDK and the runtime from the
repository root; the SDK built from source refuses to download a runtime, so
point it at the one you build:

```bash
npm --prefix sdks/typescript install
npm --prefix sdks/typescript run build
(cd examples && npm install)
./scripts/repo-cargo build -p meerkat-rpc --bin rkat-rpc
export MEERKAT_BIN_PATH="$(./scripts/repo-cargo --print-env | sed -n 's/^CARGO_TARGET_DIR=//p')/debug/rkat-rpc"
ANTHROPIC_API_KEY=sk-... npx tsx examples/039-mob-control-ts/main.ts
```

A run takes about half a minute. Each `new MeerkatClient()` starts its runtime in a realm
of its own, so the mob of one run is not there in the next.

## Offline regression checks

`npm --prefix examples run check:sdk` type-checks this example in strict
mode. `npm --prefix examples run test:sdk` runs it against a local
synthetic mob (`examples/tests/rpc_fixture.py`): it checks the order of the
requests, that the stop names the run the long job opened, that the cancel
finds two inputs waiting, and that a refused `mob/create` ends the program
nonzero with no runtime left behind. It makes no provider calls and does not
replace a run against a provider.
