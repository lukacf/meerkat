# Mob announcements: agents hear about what they subscribed to

A host program whose agents learn of events without anyone sending to them
by name: the one who announces names a topic, and every agent subscribed to
that topic is delivered the announcement. Python and TypeScript, the same
six steps, the same output.

**Needs Meta 0.9.15 or later, gateway and SDKs.** `examples.sh` builds a
gateway from this repository when `META_GATEWAY_BIN` names none.

The page it follows: [Mob announcements](../../docs/concepts/mob-announcements.mdx).

## What it shows

| Step | Call | What to notice |
| --- | --- | --- |
| 1 | `Meta.builder()...mob_announcements(config)` | Announcements are off unless the host enables them, and they need a state directory and a roster. The example sets the batch delay to one second; the default collects for up to a minute. |
| 2 | `announcement_subscription_put` | The host subscribes the agent `ops` to the topic `deploys`. The operation id names the change: sent again, it is answered from the record. |
| 3 | `announce` | `captured` is the number of subscriptions an announcement was recorded for: 1 on the topic `ops` listens to, 0 on another. The same key with the same content replays the first announcement; with other content it is refused (`announcement_conflict`). |
| 4 | `announcement_delivery_list` | What a subscriber is owed comes as a batch, one agent turn for all of it. `state` is about the delivery (`sealed`, then `delivered`), `handling` about the turn it started. |
| 5 | the agent tools `mob_subscription` and `mob_announce` | An agent subscribes and announces for itself when its profile lists the `mob_announcements` bundle. `ops` announces on a topic and does not know that `support` listens. |
| 6 | `announcement_subscription_cancel` | A change names the revision it expects. After the cancel the next announcement on the topic is captured for nobody. |

Run the program twice on the same state directory. The second run sends the
same operation ids and announcement keys: every call is answered from the
record of the first run, the subscription is as the first run left it
(`cancelled, revision 2`), and nothing is delivered twice.

Steps 1 to 4 and 6 need no provider key. Without one the agents cannot run:
the batch of step 4 stays `sealed`, and the cancel of step 6 cancels it.
With `OPENAI_API_KEY` the batch is delivered, `ops` answers from what it was
told, and step 5 runs.

## Files

- `mob.toml`: the profile of the two agents, with the tool bundle for step 5
- `main.py`: the Python program
- `main.ts`: the TypeScript program, step for step the same
- `examples.sh`: runs both twice on one state directory and checks the lines

## Run

```bash
META_GATEWAY_BIN=/path/to/rpc_gateway ./examples.sh
```

or one program by hand, with `meerkat-meta` installed:

```bash
export META_GATEWAY_BIN=/path/to/rpc_gateway
python3 main.py                       # the host-driven steps, no provider key
OPENAI_API_KEY=sk-... python3 main.py # in a fresh state directory: with the agents
```

The state is kept in `.state/` beside the programs, or in
`EXAMPLE_STATE_DIR`; remove it to start over. A state directory that was
first run without a key has no delivery left to show: its one batch was
cancelled with the subscription.

With a key the first run prints:

```text
[1] runtime started, mob announcements enabled: yes
[2] put answered: active, revision 1
[2] ops-deploys now: active, revision 1, topics deploys
[3] deploy-41 on deploys: captured for 1
[3] lunch-1 on lunch: captured for 0
[3] deploy-41 again, same content: replayed true, sequence 1
[3] deploy-41 with other content: refused, announcement_conflict
[4] ops is owed 1 batch: deploy-41
[4] the batch is delivered, its turn handled
[4] ops knows the version: 4.2
[5] support subscribed itself: active, topics incidents
[5] incident-7 by the host: captured for 1
[5] support was delivered 2 batches, announced by: the host, the member ops
[5] support knows the card processor is back: yes
[6] ops-deploys: cancelled, revision 2
[6] deploy-42 on deploys: captured for 0
[end] batches for ops: 1, delivered
[end] subscriptions: support on incidents (active), ops on deploys (cancelled)
```

## Things a first program trips over

- **The batch delay.** With the default settings an announcement reaches a
  subscriber up to a minute later, because Meta collects what a member is
  owed into one batch and one turn. Step 4 would look empty for that minute.
- **A repeated put answers what the first one answered.** In the second run
  step 2 prints `put answered: active, revision 1` although the subscription
  is cancelled by then. Read the subscription to learn what it is now.
- **The tool bundle.** Without `rust_bundles = ["mob_announcements"]` in the
  profile the agents have no tool to subscribe or announce with: in a run of
  this example with that line removed, `support` had no subscription after
  its turn and the incident was captured for nobody.
- **What an announcement says to an agent.** A batch quotes each
  announcement under the name of its announcer and tells the agent to treat
  the bodies as data, not as instructions.
