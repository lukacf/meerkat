# Foresight Studio Pack

Synthetic strategy/research scenario for a heavily customized Meta console.

This pack uses the TypeScript SDK to provide:

- `rosterProvider` for identity-first studio seating
- `topologyProvider` for the cross-functional peer graph
- `agentCustomizer` for console metadata, labels, prompt chips, and per-agent context
- `sessionService` callback tools for a synthetic signal lake, thesis scoring, evidence cards, red-team challenges, and board memo composition
- `consoleConfig("config/console.toml")` to brand the stock console and customize sidebar controls, custom buttons, rail filters, grouping, subgroups, badges, action labels, and realm overlays

## Source Layout

- `config/console.toml` — the custom console experience
- `config/mob.toml` — the studio profiles and base skills
- `scenario.yaml` — roster metadata, peer links, evidence pack, scorecard, risks, and experiments
- `run.ts` — TypeScript SDK runtime, providers, callback tools, and live console bootstrap
- `ts_smoke.ts` — offline structural smoke test
- `prompts/board-readiness.md` — operator prompt for the live demo

## Run

For live and browser runs, complete the
[shared prerequisites](../README.md#prerequisites), including the console
dependencies. Only the browser smoke needs Playwright Chromium; the
structural `--smoke` mode needs neither the console build nor a browser.

Offline structure check:

```bash
./examples/002-foresight-studio-pack/examples.sh --smoke
```

Browser smoke against a live local console:

```bash
./examples/002-foresight-studio-pack/examples.sh --browser-smoke
```

Live console:

```bash
./examples/002-foresight-studio-pack/examples.sh --kickoff
```

The script prints the local `/console` URL. Without `OPENAI_API_KEY`, the pack uses Meta's deterministic demo LLM so autonomous agents stay live and console sends work. To exercise provider-backed turns, set `OPENAI_API_KEY` and pass `--real-llm`. Omit `--kickoff` if you only want to open the customized console and start the studio manually.
