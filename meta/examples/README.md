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
