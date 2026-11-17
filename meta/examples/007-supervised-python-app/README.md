# A long-lived Meta application in Python

A program that keeps three agents for as long as it lives, and survives what
happens to such a program: an agent that needs recovering, a planned restart,
a gateway process that dies, and two mistakes a supervisor can make. It is
the [Python quick start](../../../docs/meta/sdks/python.mdx) grown into the
application you would actually run.

**Written for and run on Meta 0.9.15, gateway and Python SDK.** On 0.9.9 to
0.9.14 a respawn on a gateway that keeps its state on disk has a known issue
(see the Meta 0.9.14 release page), so step 3 needs 0.9.15.

## What it shows

| Step | Call | What to notice |
| --- | --- | --- |
| 1 | `Meta.builder()...persistent_state(dir).roster(...).init_deadline(s)` | The state directory is what makes the agents outlive the process. The SDK has no start deadline of its own; the host sets one. |
| 2 | `send_and_wait`, then `send(track_turn=True)` and `wait_for_turn` | Two ways to know that the work you sent has finished, and to get the answer of that turn and no other. |
| 3 | `respawn()` | Recovery, not reset: the agent comes back one generation later with the same session and still knows what it was told. |
| 4 | `shutdown()`, then `build()` on the same directory | The same agents, the same generations, the same memory. |
| 5 | the gateway process is killed | The next call raises `TransportReaderFailedError`; `runtime.is_running` is false. Nothing is retried for you: the program starts a new runtime, and the state is unharmed. |
| 6 | a second `build()` on a directory in use; a start that runs out of time | The first is refused. The second raises `InitOutcomeUnknownError`, which says how far the start got and does not say that it failed. |

Steps 1 and 4 to 6 need no provider key. Steps 2 and 3 do: giving an agent
work calls the model, and a respawn builds the agent again, which needs the
key of its provider. Without `OPENAI_API_KEY` the program says so at those
steps and runs the rest.

## Files

- `mob.toml`: the one profile the agents run on
- `main.py`: the program
- `examples.sh`: runs it without a key, and again with one when
  `OPENAI_API_KEY` is set, and checks what it prints

## Run

From the repository root, build the gateway once and name it:

```bash
./scripts/repo-cargo build --locked -p meerkat-meta --bin rpc_gateway
export META_GATEWAY_BIN="$(./scripts/repo-cargo --print-env | sed -n "s/^CARGO_TARGET_DIR=//p")/debug/rpc_gateway"
```

Then:

```bash
python3 -m venv .venv && . .venv/bin/activate
python -m pip install ./meta/sdk/python
export OPENAI_API_KEY=...        # optional: without it steps 2 and 3 are skipped
python meta/examples/007-supervised-python-app/main.py
```

Or with the checks:

```bash
./meta/examples/007-supervised-python-app/examples.sh
```

The program keeps its state under `.state/meta` beside it. Set
`EXAMPLE_STATE_DIR` to put it elsewhere, and delete the directory to start
over: on a second run over the same state the agents are already there, so
`desk:intake` starts at the generation the first run left it at.

Step 5 kills the gateway with `pkill`, so the program runs on Linux and
macOS.

## What it prints

With a key, on a fresh state directory (the lines that list the three agents
again after steps 3, 4 and 5 and at the end are left out here):

```text
[1] desk:intake: active, generation 0
[1] desk:billing: active, generation 0
[1] desk:notes: active, generation 0
[2] send_and_wait answered: noted
[2] the tracked turn ended completed, with text: yes
[3] respawned desk:intake: generation 0 to 1, same session: yes
[3] after the respawn, intake still knows the word: yes
[4] same agents after the restart: yes
[4] after the restart, intake still knows the word: yes
[5] the gateway is gone and the runtime says so: True
[5] same agents after the crash: yes
[5] after the crash, intake still knows the word: yes
[6] a second runtime on the same state was refused, state in use: yes
[6] a start that ran out of time: outcome unknown, last phase seen: True
[end] same agents at the end: yes
```

Without a key, steps 2 and 3 print one `skipped` line each and the lines
about what the agent remembers are absent.

The gateway writes its own log lines to standard error. Step 6 makes it
write one that reads `gateway initialization failed ... state directory ...
is in use by another Meta runtime`: that is the refusal the step asks for.

## Notes

- **Knowing that a turn finished.** `send_and_wait` and the ticket of
  `send(track_turn=True)` follow one turn, so another delivery to the agent
  cannot be mistaken for the answer. This works because the profile is
  `turn_driven`. On an `autonomous_host` agent the turn cannot be tracked and
  the SDK says so with a warning.
- **A refused start arrives wrapped.** In the 0.9.15 Python SDK a start that
  the gateway refuses is raised as `GatewayStartupCleanupError`; the sentence
  that says why is in its `startup_error`. The program reads it there.
- **After the gateway is gone, `shutdown()` raises.** The program still
  calls it, to release what the runtime holds on its side. It lets the error
  pass only when `runtime.last_gateway_termination` says the process has
  exited. In every other case the error stands: the runtime still holds a
  gateway, and a later `shutdown()` has to stop it.
- **An unknown outcome is not a refusal.** After `InitOutcomeUnknownError`
  the gateway may have changed state before it stopped. The program does not
  loop on it: it starts once more and looks at what it finds. A state
  directory that a gateway still holds refuses that start, and a start that
  settles ready shows the agents as they are.
- **What a start materializes.** By default a start builds only the agents
  that must be live, and every other agent when work first reaches it; see
  `member_restore` in the
  [configuration reference](../../../docs/meta/reference/configuration.mdx).
  This program leaves the default.
