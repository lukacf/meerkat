# 038 — Drive a Mob from the Python SDK

An application creates a small mob over `rkat-rpc`, gives it work, follows
what its members do and stops work it no longer wants. Two members: an editor,
which is the one member the application talks to, and a researcher the editor
asks for facts. [039](../039-mob-control-ts/) is the same program in
TypeScript.

Use the 0.9.15 SDK and runtime or later (see
[Stop and cancel](#stop-and-cancel) for why).

## Concepts
- `client.create_mob(definition=...)` — profiles are the roles; only a profile
  with `external_addressable: true` takes work from outside the mob, and every
  profile needs `tools.comms: true`
- `mob.spawn(profile=..., agent_identity=..., initial_message=...)` and
  `mob.wire(a, b)` — a member runs one first turn on its initial message, and
  `mob.wait_for_kickoff_complete()` waits for those turns
- `mob.member(identity).send(text)` — work goes to a member through the mob,
  not to its session
- `mob.subscribe_events()` — one stream for all members; each item names the
  member (`source`) and carries the event (`envelope.payload`)
- `mob.member_status(identity)` — status, tokens used, the last answer
  (`output_preview`) and whether a run is open (`progress.run_state`)
- `mob.member_history(identity, from_index=0, limit=1)` — the first message
  of a member is its system prompt, with the skills the runtime gave it
- `mob.stop_member_run(identity, run_id, reason=...)` and
  `client.mob_cancel_all_work(member_ref)` — the two ways to end work

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
| `mob.stop_member_run(identity, run_id, reason=...)` | The one run named by `run_id` (from the `run_started` event, `identity["run_id"]`) and the inputs that run was working on. Queued input stays. | At once. A run that is no longer current is left alone and the receipt says `not_current`. |
| `client.mob_cancel_all_work(member_ref)` | The run of that member and every input it had queued. Input sent after the cancel is untouched. | At the next boundary of the run, which can be the end of the model response it is in. |

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

With the released SDK, from the repository root (see
[002](../002-hello-meerkat-py/) for what it downloads and verifies on first
use):

```bash
python3 -m venv .venv
. .venv/bin/activate
python -m pip install meerkat-sdk
ANTHROPIC_API_KEY=sk-... python examples/038-mob-control-py/main.py
```

To run against this checkout instead, build the SDK and the runtime first; a
source install of the SDK refuses to download a runtime, so point it at the
one you build:

```bash
python3 -m venv .venv && . .venv/bin/activate
python -m pip install --upgrade pip
python -m pip install -e sdks/python
./scripts/repo-cargo build -p meerkat-rpc --bin rkat-rpc
export MEERKAT_BIN_PATH="$(./scripts/repo-cargo --print-env | sed -n 's/^CARGO_TARGET_DIR=//p')/debug/rkat-rpc"
ANTHROPIC_API_KEY=sk-... python examples/038-mob-control-py/main.py
```

A run takes about half a minute. Each `MeerkatClient()` starts its runtime in a realm
of its own, so the mob of one run is not there in the next.

## Offline regression checks

`npm --prefix examples run test:sdk` runs this example against a local
synthetic mob (`examples/tests/rpc_fixture.py`): it checks the order of the
requests, that the stop names the run the long job opened, that the cancel
finds two inputs waiting, and that a refused `mob/create` ends the program
nonzero with no runtime left behind. It makes no provider calls and does not
replace a run against a provider.
