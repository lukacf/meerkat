# TypeScript quick start: one long-lived agent

The smallest Meta application in TypeScript, as a project you can copy: a
runtime on a state directory, one agent that the program names in a roster,
a question, and the answer of that turn. Run it twice: the second run finds
the same agent, and the agent knows what the first run told it.

The page it goes with: [TypeScript SDK](../../docs/sdks/typescript.mdx).
[006](../006-long-term-agents/) goes on from here to agents that are created
and changed while the program runs.

## Files

- `package.json`: the project; `npm start` runs the program
- `config/mob.toml`: one profile, with the three settings a first program
  trips over (each has its reason in a comment)
- `main.ts`: the program

No console configuration is needed: the program opens no console.

## What it shows

| Step | Call | What to notice |
| --- | --- | --- |
| 1 | `Meta.builder().mob(...).gateway(...).persistentState(...).rosterProvider(...).build()` | The roster names the agents the application keeps. The gateway asks for it at every start, so the agent is there again after a restart, on the same session. |
| 2 | `runtime.sendAndWait(identity, text)` | One call sends, waits for the turn that send started, and resolves the text of that turn. The wait is by a ticket: nothing is polled, and another answer of the agent cannot be mistaken for this one. |
| 3 | `runtime.sendAndWaitOutcome(identity, text)` | The typed form: the text, how the turn ended (`outputStatus`), and whether the answer is attributed to this send. |

Two things decide whether step 2 works, and both are in this project:

- The agent is named by the **roster**. A member made with
  `mobHandle().ensureMember(...)` can be sent to, but `sendAndWait` on it is
  refused: `identity-first runtime not configured`.
- The profile is **`turn_driven`**. An agent in the default mode,
  `autonomous_host`, gives no ticket, and the wait is then for any completion
  of that agent. With the mode line taken out of `config/mob.toml`, a run of
  this program printed the remark the agent made at its start as the answer
  of step 2, and step 3 came back with no text and `attributed` false, under
  a `TurnTrackingUnavailableWarning`.

## Run

The SDK and the gateway must be the same version. Download
`meta-rpc-gateway-<version>-<target>.tar.gz` from the release page of that
version and extract `rpc_gateway`.

In a directory of your own, with the released SDK:

```bash
cp -r /path/to/meerkat/meta/examples/008-typescript-quickstart my-app && cd my-app
npm install
export META_GATEWAY_BIN=/path/to/rpc_gateway
npm start                          # the start and the agent, no provider key
OPENAI_API_KEY=sk-... npm start    # the question and the answer
OPENAI_API_KEY=sk-... npm start    # again: the agent remembers
```

`package.json` names the `@rkat/meta-sdk` version of this repository tree,
and the gateway must be of that version. On a checkout between two releases
that version may not be published yet: `npm install @rkat/meta-sdk@latest`
then takes the newest released one, with the gateway of the same release.

The program prints, on the first run with a key:

```text
[1] runtime running: true
[1] assistant: active, generation 0, session 19890034-a36b-50b9-a0e9-209caf190273
[2] the word from an earlier run: none
[3] answer: noted (output text, attributed to this send: true)
```

and on the second run the same session and `[2] the word from an earlier
run: heron`. The agent lives in `.meta/state` beside the program (or in
`EXAMPLE_STATE_DIR`); remove that directory to start over.

The gateway writes its own log lines to standard error. One of them warns
that the console requires sign-in and trusts no key: this program opens no
console, so nothing is refused. `npm start 2> gateway.log` keeps the lines
of the program apart.

## Check it in this repository

`./examples.sh` runs the program twice on one state directory with the SDK
of this checkout and checks what it prints: without a key, and once more
with `OPENAI_API_KEY` when it is set. `META_GATEWAY_BIN` names the gateway;
without it the script builds one, which takes a while the first time.

```bash
META_GATEWAY_BIN=/path/to/rpc_gateway ./examples.sh
```

`npm run check` type-checks `main.ts` in strict mode.
