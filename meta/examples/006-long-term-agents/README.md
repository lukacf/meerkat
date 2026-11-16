# Long-term agents created and changed at run time

A host program that changes its agents while they run: it adopts an agent its
configuration declares, creates another, updates it, archives it, brings it
back, removes it, and reads what a refusal says. Python and TypeScript, the
same five steps, the same output.

**Needs Meta 0.9.15 or later, gateway and SDKs.** `runtime.identities` and
the builder's `identity_creation(policy)` are new in the 0.9.15 SDKs. A
0.9.14 gateway reads an `identity_creation` policy at launch and carries out
no change under it; given this example's policy it refuses to start, because
it does not know the change `adopt`. `examples.sh` builds a gateway from this
repository when `META_GATEWAY_BIN` names none.

The guide it follows: [Long-term agents](../../docs/guides/long-term-agents.mdx).

## What it shows

| Step | Call | What to notice |
| --- | --- | --- |
| 1 | `Meta.builder()...identity_creation(policy)` | The gateway changes agents only under your policy. A change no rule matches is refused (`change_denied`), so the policy names every change the program makes, and allows the peers the new agent lists. |
| 2 | `identities.get`, `identities.adopt` | `scout` is declared by the roster. Adopting it releases it from configuration; from then on it is changed at run time. |
| 3 | `identities.operation` | Any change can be read back later by its request id. |
| 4 | `create`, `update`, `archive`, `unarchive`, `remove` | Each change names the spec digest it expects and answers the next one. The program checks that each answer is `done` before it reads an agent from it: under a rule that asks a person first, the answer is a pending decision with no agent in it. |
| 5 | a refused `create` | A refusal carries a kind, the field that was wrong, a sentence, and whether the same request can succeed later. |

Every change carries a request id. Run the program twice on the same state
directory: the second run sends the same ids, each change is answered from
the record of the first run, and nothing is changed twice. By then the
first run has removed `editor`, and the answers from the record state less
than the first ones: no seat and no instruction outcome.

No provider key is needed. The program starts no agent run: the agent it
creates has no first message and its profile is `turn_driven`.

## Files

- `mob.toml`: the two profiles the agents run on
- `main.py`: the Python program
- `main.ts`: the TypeScript program, step for step the same
- `examples.sh`: runs both, twice each, and checks what they print

## Run

From the repository root, build the gateway once and name it:

```bash
./scripts/repo-cargo build --locked -p meerkat-meta --bin rpc_gateway
export META_GATEWAY_BIN="$(./scripts/repo-cargo --print-env | sed -n 's/^CARGO_TARGET_DIR=//p')/debug/rpc_gateway"
```

Python:

```bash
python3 -m venv .venv && . .venv/bin/activate
python -m pip install ./meta/sdk/python
python meta/examples/006-long-term-agents/main.py
```

TypeScript:

```bash
npm --prefix meta/sdk/typescript ci && npm --prefix meta/sdk/typescript run build
npm --prefix meta/examples install
(cd meta/examples && npx tsx 006-long-term-agents/main.ts)
```

Or both, with the checks:

```bash
./meta/examples/006-long-term-agents/examples.sh
```

The programs keep their state under `.state/` beside them, each language in
its own directory (`.state/python`, `.state/typescript`), so each prints its
first-run lines the first time you run it. Set `EXAMPLE_STATE_DIR` to put
the state elsewhere. Delete the directory to start over.

## What it prints

First run:

```text
[1] runtime started with a policy that allows the host to adopt, create, describe, archive and remove
[2] scout: spec digest sha256:2a7d2d311c90...
[2] adopt answered done
[3] the adoption is committed
[4] created editor: state committed, seat seated
[4] updated editor's instructions: applies_at_next_run
[4] archived editor: state archiving
[4] unarchived editor: state restored
[4] removed editor: state retiring
[5] refused as expected: name_taken at agents[0].name: The name is taken: choose another. A removed agent's name is never available again.
[5] retryable: False
[end] agents now: scout (dormant)
```

(TypeScript prints `false` where Python prints `False`.) On a second run
the first two `[4]` lines read `seat None` (TypeScript: `seat null`) and
`not stated`: those two answers come from the record.

## Notes

- `applies_at_next_run` is what the update answers here, where the agent has
  no conversation yet: its first run is built from the new instructions.
  For an agent that already has a conversation the answer depends on its
  model. On a model that takes a system instruction in the middle of a
  conversation (`claude-haiku-5-5`, which the profiles of this example name,
  is one) the answer is `applies_at_next_run` again, and the next run of the
  agent follows the change. On a model that takes one only ahead of the
  conversation (`claude-haiku-4-5`, for instance) the update answers
  `applies_after_compaction`: the agent follows the change only once that
  conversation is compacted or reset. Read the instruction status of the
  agent (`runtime.instruction_status(identity=...)`) to see whether a change
  is in effect; the guide lists the models and says what each outcome means.
- If you archive or remove an agent within moments of changing its
  instructions while it has a conversation, the gateway log may show one or
  two `operator_instruction_delivery` warnings. This program does not cause
  them; the guide's create-and-archive sample can.
- The policy allows the host on the `rpc` surface only. Nothing in it lets
  an agent change other agents; for that see
  [Agents that create agents](../../docs/guides/agents-create-agents.mdx).
