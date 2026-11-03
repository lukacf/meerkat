# Example 003: Swarm Stress Pack

This pack is intentionally extreme. It boots 300 durable, real Gemini-backed Meerkat agents across four labeled mobs. The initial topology wires every baseline agent to 150 other baseline agents. The stress action then has 12 parent agents launch staggered burst waves over a few seconds, dynamically creating 240 more sub-agents through normal Meta JSON-RPC calls. The target post-burst steady-state is 540 visible agents.

It is designed to stress:

- late member/session creation after runtime registration
- roster discovery and console identity projection
- targeted timeline visibility for newly spawned sessions
- direct `meta/send_message` aggregate console frames
- Meerkat session-service capacity under dense baseline peer wiring
- parent-driven sub-agent burst waves spaced over a few seconds
- sub-agent replies back to parent timelines followed by parent peer fan-out
- Meerkat session-service capacity under broad parallel fan-out
- real `gemini-3.1-flash-lite-preview` completions at high roster cardinality

The configured model is `gemini-3.1-flash-lite-preview`, matching the cheap/fast flash-lite family available in the local Meerkat model catalog. The live example requires `GEMINI_API_KEY` or `GOOGLE_API_KEY`; `--demo-llm` is only for an explicitly shape-only local check and is not the real stress scenario.

For live and browser runs, complete the
[shared prerequisites](../README.md#prerequisites), including console build
dependencies and, for the browser smoke, Playwright Chromium. The structural
`--smoke` mode needs only the example JavaScript dependencies, not a provider
key, console build, or browser.

## Structural Smoke

```bash
./examples/003-swarm-stress-pack/examples.sh --smoke
```

## Real Browser Stress Smoke

```bash
./examples/003-swarm-stress-pack/examples.sh --browser-smoke
```

The browser smoke opens the console, verifies the 300-agent baseline and dense peer topology, has 12 parent agents launch 240 burst sub-agents in staggered waves, verifies at least 540 identities, sends sub-agent return probes, fans parent messages out to wired peers, sends both direct `meta/send_message` probes and browser-origin `meta/console/send` probes, waits for real non-`ok` Gemini replies in identity timelines, and writes Playwright screenshots under `output/playwright/example-003`. The live `--autoburst` path in `run.ts` uses the same parent-driven SDK choreography.

For diagnosis, the default burst concurrency is 240. You can lower only the Playwright fan-out pressure while keeping the same 540-agent post-burst target:

```bash
META_SWARM_SPAWN_CONCURRENCY=32 ./examples/003-swarm-stress-pack/examples.sh --browser-smoke
```

## Live Run

```bash
./examples/003-swarm-stress-pack/examples.sh --autoburst --kickoff
```

This uses real Gemini-backed agents by default. Use `--demo-llm` only when you intentionally want a shape-only run that does not validate model behavior.

To keep the same sessions and console history across restarts while investigating
backfill/replay behavior, run the TypeScript entrypoint with persistent state
enabled:

```bash
(cd examples && META_KEEP_EXAMPLE_STATE=1 npx tsx ./003-swarm-stress-pack/run.ts --skip-build)
```

`--skip-build` assumes the gateway is already built. Dense topology reapplication
is skipped when keep-state is enabled, recognized persistent state exists, and
the restored undirected edge count reaches `DENSE_RESTORE_EDGE_FLOOR`
(currently 22,500). Otherwise the example reapplies dense wiring, deferring it
on recognized retained-state boots; requested autoburst/kickoff waits for that
deferred application.

`META_SWARM_SKIP_DENSE=1` bypasses both the dense topology provider and its
application regardless of the restored edge count. It does not guarantee
either zero edges or a 540-agent roster. Burst roster restoration is a separate
decision: `META_SWARM_RESTORE_BURST=1` enables it, `=0` disables it, and
otherwise it follows keep-state plus recognized persistent state. These flags
describe the example's restart decisions, not a guarantee that every stored
topology or session will restore successfully.
