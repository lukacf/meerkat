# Ask the user: an agent stops to ask a person, and goes on with the answer

A host program whose agent does not guess what its person wants: it asks,
with the `ask_user` tool that agents of a Meta gateway are offered, and takes
up the work again when the answer comes. The question waits in the state directory,
so the answer may come much later, and after a restart. Python and
TypeScript, the same six steps.

**Needs Meta 0.9.15 or later, gateway and SDKs, and a provider key**
(`OPENAI_API_KEY`): only an agent can ask a question. `examples.sh` builds a
gateway from this repository when `META_GATEWAY_BIN` names none.

The page it follows: [Question sets](../../docs/concepts/gating.mdx#question-sets).

## What it shows

| Step | Call | What to notice |
| --- | --- | --- |
| 1 | `Meta.builder()...persistent_state(dir)` | A question can only wait where there is a state directory. Nothing else is switched on: an agent of a Meta gateway is offered `ask_user` unless its profile denies it (the page above names the few other exceptions). |
| 2 | a message to the agent | The planner is given two options and told never to choose for the user. It calls `ask_user` and its turn ends: the call does not wait for the person. |
| 3 | `gating_questions` | The host reads the open question sets: who asked, each question with its choices, and the deadline (one hour unless the agent names another). |
| 4 | `shutdown`, then a new launch | The same set is open after a restart of the runtime. |
| 5 | `gating_answer` | The host sends the answer for the person: the id of the set, the digest it was shown with, who answered, one choice per answered question. The answer reaches the agent as the result of its question and starts a turn of its own. |
| 6 | `gating_dismiss` | A set can be ended without an answer. The agent is told that none is coming and goes on without one. |

This program plays the person: it answers with the choice named by
`EXAMPLE_CHOICE` (`boat` unless set). In an application the same three calls
sit behind your own screen: list the open sets, show one, send the answers.
Where the gateway serves a Console that people sign in to, the Console shows
the same sets and a signed-in person answers them there (see the page above;
this example has no Console).

Run it again on the same state directory and the planner asks a new
question: each run is one whole round. A run that was stopped between
steps 3 and 5 leaves its question open, and the next run takes that one up
instead of asking again.

## Files

- `mob.toml`: the profile of the one agent
- `main.py`: the Python program
- `main.ts`: the TypeScript program, step for step the same
- `examples.sh`: runs both twice on one state directory and checks the lines

## Run

```bash
OPENAI_API_KEY=sk-... META_GATEWAY_BIN=/path/to/rpc_gateway ./examples.sh
```

or one program by hand, with `meerkat-meta` installed:

```bash
export META_GATEWAY_BIN=/path/to/rpc_gateway
OPENAI_API_KEY=sk-... python3 main.py
OPENAI_API_KEY=sk-... EXAMPLE_CHOICE=bowling python3 main.py
```

The state is kept in `.state/` beside the programs, or in
`EXAMPLE_STATE_DIR`; remove it to start over.

A run prints (the sentences of the planner are its own and differ from run
to run):

```text
[1] runtime started, open question sets: 0
[2] the planner's turn ended with: I asked the user.
[3] asked by planner: Which option would you like for the team outing on Friday? Choices: bowling (Bowling), boat (Boat trip). Open for about 60 more minutes.
[4] after a restart the same question is open: yes
[5] the host answered boat for the person: the set is answered
[5] the planner went on: You chose a boat trip for the team outing on Friday.
[6] the host dismissed the question lunch: the set is dismissed
[6] the planner was told: No answer came about whether to include lunch.
[end] open question sets: 0
```

Without a provider key the program starts the runtime, prints that no
question set is open, says that step 2 is skipped, and stops.

## Things a first program trips over

- **The call does not wait.** `ask_user` answers `asked` at once and the
  turn of the agent ends. The answer arrives later as a new input and starts
  a new turn, so wait for the completion after the answer, not for the turn
  in which the agent asked. The programs read a completion cursor before
  they answer and wait for the first completion after it.
- **No state directory, no question.** On a launch without
  `persistent_state` the same call answered `no_human_surface` in a run of
  this example (the tool told the agent that nothing was asked and no answer
  will arrive), and `gating_questions` listed nothing.
- **A question nobody answers.** The set stays open until its deadline. In
  a run with a deadline of one minute the set left the list at the deadline
  and the planner was given a turn in which it said that no answer came.
  A gateway started with the default settings of the SDK logs that its
  Console requires sign-in and trusts no signing key, so nobody can answer
  there: the host program is the one who can. List the sets and answer or
  dismiss them, as this example does.
- **Refused answers.** Run on Meta 0.9.16: an answer with a digest other
  than the one the set was shown with is refused `subject_changed`; a choice
  the question does not have is refused as invalid params, naming the
  question and the choice; a second answer to a set that was answered is
  refused `not_open`. Nothing is decided by a refused answer.
- **Keeping the tool from an agent.** With `deny = ["ask_user"]` under
  `[profiles.planner.tools]` the planner of this example had no such tool
  and asked nothing.
- **The profile needs comms.** With a profile that lacks `comms = true`
  under its tools, the first message to the planner was refused with a
  wiring error that names the profile.
