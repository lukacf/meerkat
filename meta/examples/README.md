# Examples

This repo now has a repo-root `examples/` surface for multi-language packs that prove the shipped Meta surfaces end to end.

Numbered packs are intentionally browser-first and can include:

- a stock Rust example server
- a browser smoke proof
- TypeScript and Python helpers
- a shared scenario file
- operator drill prompts

## Packs

- `001-incident-command-center-pack`
- `002-foresight-studio-pack`
- `003-swarm-stress-pack`
- `004-mdm-console-pack`
- `005-access-control-pack`

Host programs:

- `006-long-term-agents` (Python and TypeScript; no console, no provider key)
- `007-supervised-python-app` (Python; a long-lived application: work, recovery, restart, a lost gateway; a provider key is optional)
- `008-typescript-quickstart` (a TypeScript project to copy: one agent, a question, its answer; no console; a provider key for the question)
- `009-mob-announcements` (Python and TypeScript; no console; a provider key for the steps in which agents run)
- `010-ask-the-user` (Python and TypeScript; an agent asks a person and goes on with the answer; no console; needs a provider key)
- [TypeScript host runnable](../sdk/typescript/examples/README.md)
  (a durable source-runner schedule and its callback; no provider key)

## Prerequisites

From the repository root, install the separate console and example JavaScript
dependencies before running packs that build the console:

```bash
npm --prefix console ci
npm --prefix examples ci
```

Installing only `examples/` does not install the console's build tools.
The structural `--smoke` modes in packs 002 and 003 exit before the console
build; they only need the example JavaScript dependencies.

For browser smoke lanes, also install Playwright's Chromium:

```bash
(cd examples && npx playwright install chromium)
```

Install any browser system dependencies required by your platform as well.
Structural smokes and the ABAC pack's HTTP-only smoke do not need Chromium.
Pack 001's live launcher requires `OPENAI_API_KEY` and runs browser,
TypeScript, and Python smoke helpers (so Python 3 is also required). Its
separate deterministic topology browser lane needs Chromium but no provider
key. Pack 003's real browser/live lanes require `GEMINI_API_KEY` or
`GOOGLE_API_KEY`; pack 005 needs no provider credentials.

## Running packs

After the setup above, enter `examples/` for the following commands:

```bash
cd examples
```

Run the first pack with:

```bash
export OPENAI_API_KEY=...
./001-incident-command-center-pack/examples.sh
```

Run the second pack's offline structure check with:

```bash
./002-foresight-studio-pack/examples.sh --smoke
```

Run the live customized console with:

```bash
export OPENAI_API_KEY=...
./002-foresight-studio-pack/examples.sh --kickoff
```

Run the third pack's browser-driven real Gemini stress smoke with a 300-agent baseline plus a 240-agent burst:

```bash
./003-swarm-stress-pack/examples.sh --browser-smoke
```

Run the MDM console pack's local target smoke:

```bash
npm run mdm:smoke
npm run mdm:browser-smoke
```

Run the access control pack's deterministic ABAC smoke (no API key), or serve
it for per-persona browser exploration:

```bash
./005-access-control-pack/examples.sh
./005-access-control-pack/examples.sh --serve
```

Run the long-term agents example (Python and TypeScript, no API key; it
builds the gateway unless `META_GATEWAY_BIN` names one):

```bash
./006-long-term-agents/examples.sh
```

Run the supervised Python application (it runs its steps that need no API
key, and all of them when `OPENAI_API_KEY` is set; it builds the gateway
unless `META_GATEWAY_BIN` names one):

```bash
./007-supervised-python-app/examples.sh
```

Run the TypeScript quick start (twice on one state directory; with
`OPENAI_API_KEY` set it also asks the agent a question and checks that the
second run remembers the first):

```bash
./008-typescript-quickstart/examples.sh
```

Run the mob announcements example (Python and TypeScript, twice each on one
state directory; with `OPENAI_API_KEY` set it also runs the agents):

```bash
./009-mob-announcements/examples.sh
```

Run the ask-the-user example (Python and TypeScript; with `OPENAI_API_KEY`
set the agent asks, the host answers after a restart, and a second question
is dismissed; without it only the start is checked):

```bash
./010-ask-the-user/examples.sh
```
