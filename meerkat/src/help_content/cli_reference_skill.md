---
name: meerkat-cli-reference
description: "Exact rkat CLI command contract for Meerkat help answers. Use this as the authority for command names, flags, aliases, and negative CLI facts."
---

# Meerkat CLI Reference

Use this skill as the exact authority for `rkat` shell commands. Command
spelling, singular/plural forms, flags, and enum values matter.

## Hard Negative Facts

- There is no `rkat sessions`; use singular `rkat session`.
- There is no `rkat resume`; use `rkat run --resume ...`.
- There is no `rkat rpc`; use the `rkat-rpc` binary for JSON-RPC.
- There is no `rkat image` command.
- There is no `rkat mcp reload` CLI command.
- There is no `--tools all`; valid values are `safe`, `workspace`, `full`, `none`.
- `rkat blob get` has no `-o` or `--out`; use `--output <FILE>`.
- `rkat session show` has no `--json` flag.
- `rkat models` and `rkat capabilities` already print pretty JSON and have no `--json`/`--format` flags.

## Root

```bash
rkat [OPTIONS] <PROMPT>      # shorthand for rkat run <PROMPT>
rkat [OPTIONS] <COMMAND>
```

Global/realm options:

```bash
-r, --realm <REALM>
--isolated
--instance <INSTANCE>
--realm-backend <jsonl|sqlite>
--state-root <PATH>
--context-root <PATH>
--user-config-root <PATH>
```

Top-level commands:

```bash
rkat init
rkat run <PROMPT> [OPTIONS]
rkat help <QUESTION> [OPTIONS]
rkat session list|show|delete|interrupt ...
rkat realtime open-info|status|capabilities|bridge ...
rkat blob get <BLOB-ID> [--output <FILE>] [--json]
rkat realm current|list|show|create|delete|prune ...
rkat mcp add|remove|list|get ...
rkat mob spawn-helper|fork-helper|member-status|force-cancel|respawn|wait-kickoff|run-flow|flow-status|pack|inspect|validate|deploy|web ...
rkat skill add|remove|get|list|inspect ...
rkat config get|set|patch ...
rkat capabilities
rkat models
rkat doctor
rkat auth realms|profiles|profile|profile-delete|bindings|test|status|login|logout|refresh ...
```

## Run

```bash
rkat run <PROMPT>
rkat <PROMPT>
rkat run --resume[=<SESSION>] <PROMPT>
```

Common options:

```bash
--resume [<SESSION>]            # omit value to resume last
-m, --model <MODEL>
-d, --max-duration <DURATION>
-o, --output <text|json>
--json                          # alias for --output json
-s, --stream
--no-stream
--no-web-search                 # disable provider-native web search for this run
--skill <PATH_OR_ID>            # repeatable
-t, --tools <safe|workspace|full|none>
--yolo                          # alias for --tools full
-v, --verbose
--keep-alive
--stdin <auto|blob|lines|off>
```

Advanced options:

```bash
--system <SYSTEM_PROMPT>
-p, --provider <anthropic|openai|gemini|self-hosted>
--max-tokens <N>
--max-tool-calls <N>
--param <KEY=VALUE>             # repeatable
--params-json <JSON>
--schema <SCHEMA_OR_PATH>
--allow-tool <TOOL>             # repeatable first-turn allow overlay
--block-tool <TOOL>             # repeatable first-turn block overlay
--label <KEY=VALUE>             # repeatable
--instructions <TEXT>           # repeatable
--app-context <JSON>
--wait-for-mcp
--line-format <text|json>
--auth-binding <REALM:BINDING[:PROFILE]>
```

Resume targets: full UUID, short UUID prefix, `realm:<uuid>`, `last`, `~`,
`~0`, or `~N`.

Defaults:

- `--tools safe`
- provider-native web search on for supporting models; use `--no-web-search` to disable
- stream on in a TTY, off in pipes/scripts
- piped stdin is blob context unless `--stdin lines`

## Help

```bash
rkat help <QUESTION>
rkat help <QUESTION> --prompt <PROMPT> --plan-execution
```

Options: `--prompt`, `--plan-execution`, `-m/--model`, `-p/--provider`,
`--max-tokens`, `-o/--output`, `--json`, `-s/--stream`, `--no-stream`, plus
global/realm options.

## Image Generation Via CLI

Image generation is assistant-mediated. Ask through `rkat run`, allow the image
tool, and fetch the returned blob id:

```bash
rkat run --allow-tool generate_image "Use generate_image to create a 1024x1024 PNG of a red fox in snow. Return the blob id."
rkat blob get <BLOB-ID> --output fox.png
```

If the assistant did not print the blob id, resume the same session and ask for
the blob id. Do not use `rkat sessions show --json`, `rkat rpc blob/get`, or
`rkat blob get -o`.

## Session

```bash
rkat session list [--limit N] [--offset N] [--label KEY=VALUE]
rkat session show <ID>
rkat session delete <ID>
rkat session interrupt <ID>
```

`rkat session show` is human-readable and has no `--json`.

## Blob

```bash
rkat blob get <BLOB-ID> [--output <FILE>] [--json]
```

`--output` writes raw bytes to a file. `--json` prints the blob payload as JSON.
`blob get` requires an existing blob id; it does not discover blob ids.

## MCP

```bash
rkat mcp add <NAME> [--transport stdio|http|sse] [--scope project|user|local] [-H KEY:VALUE...] [-e KEY=VALUE...] (--url <URL> | -- <CMD...>)
rkat mcp remove <NAME> [--scope project|user|local]
rkat mcp list [--scope project|user|local] [--json]
rkat mcp get <NAME> [--scope project|user|local] [--json]
```

Omit `--scope` on `list`/`get` to search all scopes. Do not pass `--scope all`.
Live mutation of an already-running session uses RPC/REST/MCP/SDK surfaces, not
`rkat mcp reload`.

## Realm

```bash
rkat realm current
rkat realm list
rkat realm show <REALM_ID>
rkat realm create <REALM_ID> [--backend jsonl|sqlite]
rkat realm delete <REALM_ID> [--force]
rkat realm prune [--isolated-only] [--older-than-hours N] [--force]
```

## Realtime

```bash
rkat realtime open-info session <SESSION-ID>
rkat realtime status session <SESSION-ID>
rkat realtime capabilities session <SESSION-ID>
rkat realtime bridge session <SESSION-ID>
rkat realtime open-info member <MOB-ID> <AGENT-IDENTITY>
rkat realtime status member <MOB-ID> <AGENT-IDENTITY>
rkat realtime capabilities member <MOB-ID> <AGENT-IDENTITY>
rkat realtime bridge member <MOB-ID> <AGENT-IDENTITY>
```

`status` is public realtime channel status, not the attachment-status enum.
`bridge` proxies typed realtime frames over stdin/stdout.

## Mob

Direct `rkat mob` is helper/artifact oriented. Lifecycle creation, wiring, and
member management are done with agent `mob_*` tools or RPC `mob/*`.

```bash
rkat mob spawn-helper <mob_id> <prompt> [--profile <profile>] [--agent-identity <id>] [--json]
rkat mob fork-helper <mob_id> <source_member> <prompt> [--profile <profile>] [--agent-identity <id>] [--fork-context full-history|last-messages] [--last-messages N] [--json]
rkat mob member-status <mob_id> <agent_identity> [--json]
rkat mob force-cancel <mob_id> <agent_identity>
rkat mob respawn <mob_id> <agent_identity> [--initial-message <MSG>]
rkat mob wait-kickoff <mob_id> [--member <agent_identity>...] [--timeout-ms N] [--json]
rkat mob run-flow <mob_id> --flow <flow_id> [--params <json>] [-s|--stream] [--no-stream]
rkat mob flow-status <mob_id> <run_id>
rkat mob pack <dir> -o <pack> [--sign <key> --signer-id <id>]
rkat mob inspect <pack>
rkat mob validate <pack> [--trust-policy permissive|strict]
rkat mob deploy <pack> <prompt> [--model <model>] [--max-total-tokens N] [--max-duration D] [--max-tool-calls N] [--trust-policy permissive|strict] [--surface cli|rpc]
rkat mob web build <pack> -o <dir> [--trust-policy permissive|strict]
```

For `rkat run` prompts that need `mob_*` tools, use `--tools full` or config
that enables mob tools.

## Skill

```bash
rkat skill add <PATH> [--name <NAME>]
rkat skill remove <NAME_OR_SOURCE_UUID_OR_PATH>
rkat skill get <NAME_OR_SOURCE_UUID_OR_PATH> [--json]
rkat skill list [--json]
rkat skill inspect <SKILL_NAME> --source-uuid <SOURCE_UUID> [--json]
```

`skill add` takes a filesystem path, not an arbitrary skill id.

## Config

```bash
rkat config get [--format toml|json] [--with-generation]
rkat config set [FILE] [--json <JSON> | --toml <TOML>] [--expected-generation <N>]
rkat config patch [FILE | --json <JSON>] [--expected-generation <N>]
```

`set <FILE>` imports a file into the active realm config. Raw JSON merge patch
uses `rkat config patch --json '{...}'`.

## Models And Capabilities

```bash
rkat models
rkat capabilities
```

Both commands print pretty JSON by default. Neither accepts `--json` or
`--format`.

`rkat models` prints the effective runtime model registry for the active realm:
built-in catalog entries plus configured `self_hosted` aliases.

## Doctor

```bash
rkat doctor
```

No doctor-specific flags. Output is tab-delimited `ok|warn <area> <message>`.
Hard config/MCP/self-hosted issues exit 1; missing provider env vars and missing
`wasm-pack` are warnings.

## Auth

```bash
rkat auth realms
rkat auth profiles
rkat auth profile <PROFILE_ID>
rkat auth profile-delete <PROFILE_ID> [-y|--yes]
rkat auth bindings
rkat auth test <BINDING_ID>
rkat auth status <PROFILE_ID>
rkat auth login [PROVIDER] [--backend <BACKEND>] [--method <METHOD>] [--non-interactive --secret <SECRET>]
rkat auth logout <PROFILE_ID>
rkat auth refresh <PROFILE_ID>
```

`rkat auth login <provider>` is a convenience bootstrap for `dev:*` bindings.
Interactive OAuth writes `dev:anthropic_oauth`, `dev:openai_oauth`, or
`dev:google_oauth`; non-interactive api-key login writes `dev:default_<provider>`.

Runtime env fallback order:

- Anthropic: `RKAT_ANTHROPIC_API_KEY`, `ANTHROPIC_API_KEY`
- OpenAI: `RKAT_OPENAI_API_KEY`, `OPENAI_API_KEY`
- Gemini: `RKAT_GEMINI_API_KEY`, `GEMINI_API_KEY`, `RKAT_GOOGLE_API_KEY`, `GOOGLE_API_KEY`
