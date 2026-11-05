# meerkat-tui (`rkat-tui`)

An interactive terminal coding harness built on the Meerkat agent engine. It
hosts its sessions in-process on Meerkat's runtime-backed session path (the
one `rkat`, REST and RPC use) and gates every tool call through an
**auto-mode** approval model: routine actions run without interruption,
risky ones are judged by a reasoning-blind classifier, and you are asked only
after repeated blocks. The shell runs inside an OS sandbox by default.

```bash
cargo run -p meerkat-tui -- "fix the failing test"
rkat-tui -c                       # continue the latest session of this workspace
rkat-tui --resume ~2
rkat-tui --model gpt-5.5 --classifier-model gpt-5.5
rkat-tui --no-auto                # tier-3 actions are denied instead of classified
rkat-tui --model llama3:8b        # bare ids are looked up in every endpoint's model list
rkat-tui --endpoint-def litellm=openai=http://localhost:4000,key_env=LITELLM_KEY --model litellm:gpt-5.5
rkat-tui --allow 'shell=cargo nextest run*' --set auto.compact=auto
rkat-tui --no-confinement         # run the shell unsandboxed (see "Confinement")
```

## Sessions

Every session lives in a `PersistentSessionService` over the realm's stores,
with a `MeerkatMachine` owning input admission, interruption and resume. The
transcript, the model and the cost totals survive restarts: `-c` continues
the workspace's latest session, `--resume <ref>` or `/resume [ref]` any other
(`last`, `~N` for the N-th most recent, an id or id prefix; `/resume` alone
opens a picker), `/clear` (`/new`) starts a fresh one.

**Typing while the agent works.** Enter on an idle session starts a turn
(a `Queue` input). While a turn runs, Enter *steers*: the message becomes a
durable `Steer` input that the runtime writes into the running turn as an
ordinary user message at its next model boundary (between tool calls), or
runs as one follow-up turn when no boundary comes. Its placeholder in the
transcript reads *queued*, then *delivering*, and becomes your message once
the runtime reports it written. A steer the runtime did not deliver (an
interrupt, a failed turn) comes back to the editor.

*Esc limitation:* a steered message belongs to the runtime as soon as it is
admitted, so it cannot be pulled back. Esc recalls only messages the UI still
holds (typed while `/compact now` is arming, or right as a turn ended);
with nothing to recall, Esc interrupts the turn. Ctrl-C interrupts too:
the current run is stopped (steers that joined it are abandoned, not re-run),
outstanding inputs are cancelled and running helpers are cancelled.

Ctrl-D on an empty editor (or `/quit`) quits at once when idle. While a turn
runs or messages are pending it warns first; pressing it again within 3 s
interrupts the turn and appends every message the model never received to
`<workspace>/.rkat/tui/tui_unsent.txt` (the next start says so).

## How approval works

Every tool call of every session (and of every helper, see below) passes the
approval gate, installed as the session build's `ToolDispatchAdmission`, so
resumed sessions and helpers carry it too. First match wins:

| Tier | What | Review |
|---|---|---|
| 0 | A user **deny** rule matches | denied |
| 1 | Read-only tools (`read_file`, `glob`, `grep` inside the project, task tools, `memory_search`, `memory_list`/`memory_read`, `blob_inspect`, skill browsing, `shell_jobs`, read-only `mob_*` queries, `delegate`), read-only shell commands (`cat`, `ls`, `rg`, `git status/diff/log`, `cargo check/test` when builds are trusted, …), narrow user **allow** rules | allowed |
| 2 | `write_file` / `edit_file` / `apply_patch` where every target is inside the project and not protected (`.rkat/`, `.git/`, `.env*`, …); workspace memory notes | allowed |
| 3 | Everything else: other shell commands, MCP tools, web search, comms sends, mutating mob tools, out-of-project paths, protected paths, user-level notes | classifier |

A denial is an ordinary tool result (`policy_denied` with the reason); the
model reads it and the run continues.

The classifier is a separate LLM call on the start-up endpoint. It is given
the user's messages and the agent's prior tool calls, never the agent's
reasoning or its tool outputs, so the agent cannot argue with it directly.
Stage 1 is a one-word ALLOW/BLOCK told to err on the side of blocking; stage 2
re-evaluates flagged actions with reasoning and may read project files itself
(read-only `read_file`, `glob`, `grep` inside the project; at most 6 calls,
16 KiB per result, 60 s) to judge what an action will really execute. File
content is framed as untrusted evidence, files the agent wrote this session
are named as the agent's, and environment files (`.env`, `.env.*`, `.envrc`)
are never read. `auto.classifier_tools = false` makes stage 2 decide from the
attached context alone.

After **3 consecutive** or **20 total** classifier blocks, auto mode pauses
and each tier-3 action opens a dialog: allow once, allow and add a narrow
rule, deny, or stop the turn. `/auto resume` resumes; `/auto` shows the
state and counters. Escalation decisions are kept in the realm's approval
ledger.

## Confinement

`--confinement <mode>` decides how the shell (foreground commands,
background jobs, monitors) runs:

| Mode | Read and write | Read only | Network |
|---|---|---|---|
| `workspace` (default) | the workspace, the temp dir, the Cargo home, the user cache dir | the rustup home, `/usr/{local,share,include}`, `/opt`, CA certificates, `/etc/{resolv.conf,hosts,nsswitch.conf,passwd,group,gitconfig}` | allowed |
| `strict` | as `workspace` | as `workspace` | denied |
| `off` (`--no-confinement`) | no sandbox | | the host's |

Command hooks and local MCP stdio servers are not confined. The mode is
announced at start-up; a realm `[confinement]` section is ignored (the
command line wins). Every shell call runs in durable process custody, so an
interrupted call leaves no process behind.

A host that cannot enforce the mode refuses to start and says why.
**Ubuntu 24.04 and later** restrict unprivileged user namespaces
(`kernel.apparmor_restrict_unprivileged_userns=1`), which the Linux sandbox
needs: start with `--no-confinement`, or allow user namespaces for
`rkat-tui` with an AppArmor profile. Where `/etc/resolv.conf` is a symbolic
link (systemd-resolved), name resolution fails inside the Linux sandbox.

## Helpers

The agent can hand self-contained subtasks to helper agents with `delegate`
(Meerkat mobs: each helper is a mob member). A helper runs on the session's
model and endpoint, with the TUI's file tools and a helper persona, does one
turn and returns its final text. Its tool calls pass the **delegating
session's** gate (same rules, counters and Stop; escalations name the
helper), so delegating never unlocks an action. The transcript nests helper
activity under `[label · model]`, the status bar counts running helpers, and
an interrupt cancels them. `fork_off` is not usable from a TUI session (it
requires the caller to be a durable mob member).

## Rules

Rules live in TOML, layered (later settings win): the user-global
`<config-dir>/tui.toml`, then `<realm>/tui.toml` (written by `/rules add`),
then the command line, then `<project>/.rkat/tui.toml`. The project file may
only add deny rules and `[auto]` settings; broad allow rules (blanket shell,
interpreters, package-manager run commands) are dropped when auto mode
starts.

```toml
[auto]
enabled = true
classifier_model = "claude-sonnet-4-6"   # optional, per-dialect default otherwise
trust_project_builds = true              # cargo build/test/check are tier 1
compact = "warn"                         # or "auto"; see "Context and compaction"
compact_warn_percent = 75
compact_auto_percent = 85
classifier_tools = true                  # stage 2 may read project files

[[allow]]
tool = "shell"
arg = "cargo nextest run*"

[[deny]]
tool = "shell"
arg = "git push*"
```

### Configuration reference (`tui.toml`)

Every key can be set in a user layer or for one run with
`--set <dotted.key>=<value>`. The value is TOML (`true`, `85`, `"x"`,
`[{tool = "shell"}]`); anything that does not parse as TOML is a string, so
`--set auto.compact=auto` needs no quotes. Command-line settings form a layer
after the global and realm files and before the project file: tables merge,
scalars replace, arrays append, endpoints merge by name. A bad `--set`
(unknown key, wrong type, invalid pattern or dialect) stops start-up and
names the argument. Command-line rules show as `(cli)` in `/rules list` and
are never written to a file.

| Key | Flag | Default |
|---|---|---|
| `auto.enabled` | `--no-auto` (= `false`) | `true` |
| `auto.classifier_model` | `--classifier-model <id>` | per-dialect default of the start-up endpoint |
| `auto.trust_project_builds` | `--trust-project-builds` / `--no-trust-project-builds` | `false` |
| `auto.compact` (`"warn"`/`"auto"`) | `--auto-compact` / `--no-auto-compact` | `"warn"` |
| `auto.compact_warn_percent` | `--set auto.compact_warn_percent=N` | `75` |
| `auto.compact_auto_percent` | `--set auto.compact_auto_percent=N` | `85` |
| `auto.classifier_tools` | `--set auto.classifier_tools=false` | `true` |
| `[[allow]]` / `[[deny]]` `tool`, `arg` (glob) or `literal` | `--allow` / `--deny <tool>[=<glob>]` (repeatable) | none |
| `endpoints.<name>.dialect`, `.base_url`, `.api_key_env`, `.api_key`, `.priority` | `--endpoint-def <name>=<dialect>=<url>[,key_env=VAR]` | |
| `pricing."<model id>".input`, `.output`, `.cached_input`, `.cache_write` (USD per 1M tokens) | `--set 'pricing."<id>".input=15'` | fetched prices |

Other flags: `--model`/`-m <[endpoint:]id>`, `--endpoint <name>`
(`--provider` for the built-ins), `--resume <ref>`, `-c`/`--continue`,
`-w`/`--workspace`, `--config-dir`, `--user-config-root`, `--realm`,
`--isolated`, `--realm-backend`, `--state-root`, `--confinement`,
`--no-confinement`, `--no-mouse`, `--wait-for-mcp`, `--no-pricing-fetch`.

## Models, endpoints and cost

An **endpoint** is a named place to send requests in one of Meerkat's client
dialects: `anthropic`, `openai` (also LiteLLM and other OpenAI-compatible
proxies) or `gemini`. The built-in endpoints exist when their key resolves
(realm config, provider environment variable, or the `rkat` token store).
More come from `[endpoints.<name>]` in `tui.toml`; they are wired as auth
bindings of an in-memory config layer and never written to a realm file.
Model references are `endpoint:id` or a bare id, looked up in the endpoints'
live model lists (fetched in parallel in the background, 10 s each).

`/model` opens a picker grouped by endpoint (type to filter), `/model <ref>`
switches directly and `/model refresh` refetches the lists. A switch is a hot
swap of the live session's model through the runtime (between turns), also
across endpoints and dialects; the choice is kept next to the session and
restored by `-c`/`/resume`. Helpers follow the session's model; the
classifier stays on the start-up endpoint.

The status bar shows the context gauge, `ctx 42% (84k/200k)` (yellow from 70%,
red from 90%, `~` for an estimate), and the session's cost, `$0.42`: each
model call's tokens are priced per model from `[pricing]` overrides, a
LiteLLM endpoint's `/model/info`, or LiteLLM's community price list (cached
for 7 days in `<config-dir>`; `--no-pricing-fetch` skips the download).
Helpers' usage counts too. `/cost` prints the breakdown. The readout is an
estimate, not a bill.

## Instructions and memory

The system prompt of a new session is built from, in order:

1. the base prompt: the user-global `SYSTEM.md` replaces Meerkat's default
   when it is non-empty and the realm config sets no `system_prompt` /
   `system_prompt_file` of its own;
2. the workspace `AGENTS.md` (or `.rkat/AGENTS.md`), loaded by Meerkat, and
   the skill inventory;
3. the coding persona, then the user-global `AGENTS.md`, every `AGENTS.md`
   (or `.rkat/AGENTS.md`) from the git root down to the workspace's parent
   directory, and the workspace `AGENTS.local.md` (or
   `.rkat/AGENTS.local.md`, meant to stay uncommitted), each under
   `# Instructions (from <path>)`. Each file is capped at Meerkat's
   `AGENTS.md` limit (32 KiB) and all of them together at 96 KiB. The git
   root is the nearest directory with a `.git` directory or file; your home
   directory is never one;
4. the memory-notes index (below), when there are notes.

Instruction files and notes are re-read whenever a session is built
(`/clear`, `/resume`). A resumed session keeps the system prompt it was
created with (Meerkat does not reassemble it on resume).

**`/remember <text>`** (or a single-line `#text`; `##…`, multi-line input and
`\#…` are sent as prompts) appends `- <text>` to the workspace `AGENTS.md`
(created with an `# AGENTS.md` header if missing) and appends it to the live
session as durable system context under `# Remembered instruction`, so the
model reads it from its next request; it is fully applied, as part of
`AGENTS.md`, from the next session on.

**Memory notes** are small Markdown files the agent keeps across sessions with
`memory_save`, `memory_read`, `memory_list` and `memory_forget`. Workspace
notes live in `<workspace>/.rkat/tui/notes/`, user notes (personal
preferences that follow you into every project) in `<config-dir>/notes/`;
each directory has a generated `MEMORY.md` index. The system prompt carries
only a capped index; the agent reads full notes on demand. Reads are tier 1,
workspace writes tier 2, user-note writes go to the classifier.

**Semantic memory** is Meerkat's own: when a compaction discards messages,
they are indexed into the realm's semantic index (`<realm>/memory/`), and the
agent finds them again with `memory_search` (tier 1). The index is opened once
at start-up; when it cannot be opened, the session runs without
`memory_search` and a start-up notice says so.

**MCP servers** come from the realm config's `[[tools.mcp_servers]]`, the
project's `.rkat/mcp.toml` and the user's `~/.rkat/mcp.toml` (`rkat mcp add`
writes them), loaded once at start-up. They connect in the background (the
model sees a pending notice until they are ready); `--wait-for-mcp` waits for
them before the first turn. MCP tools are tier 3 unless a rule allows or
denies them by name. Servers from the project file start with your privileges
and are named in a start-up notice.

### Context and compaction

Compaction is Meerkat's native auto-compaction: before a model call the
runtime summarizes older history once the context crosses the session's
threshold, keeps the recent turns, and indexes what it discarded into
semantic memory. rkat-tui only chooses the threshold:

- `compact = "warn"` (the default): the size trigger is off (Meerkat still
  compacts to recover a request the provider refused as too large). The gauge
  warns once when the context crosses `compact_warn_percent` (75%) and after
  every turn from `compact_auto_percent` (85%) on.
- `compact = "auto"` (or `--auto-compact`; `--no-auto-compact` forces
  `warn`): the threshold is `compact_auto_percent` of the model's context
  window, and the runtime compacts before the first model call that would
  cross it. A model switch that moves the threshold rebuilds the idle
  session.

For a model whose window is unknown the thresholds are 120k (warn) and 150k
(auto) tokens. `compact_auto_percent` below `compact_warn_percent` is raised
to it, with a notice.

`/compact` shows the policy and threshold. Meerkat has no on-demand
compaction call, so **`/compact now`** (idle only, after at least one turn)
rebuilds the session with a one-turn threshold of one token: the next turn's
first model call compacts first, and when that turn ends the session is
rebuilt on the policy's threshold again. A long tool loop in that turn may
compact again after Meerkat's cadence (3 model calls by default). Messages
typed while it arms wait for it. The `✓ compacted` notice appears when the
session next runs (Meerkat publishes it once the semantic index took the
discarded messages).

## Keys and commands

`/help`, `/quit`, `/clear`, `/resume [ref]`,
`/rules [list|add allow|deny <tool> [pattern]|rm <n>]`, `/auto [on|off|resume]`,
`/model [[endpoint:]id|refresh]`, `/remember <text>`, `/compact [now]`,
`/cost`. `/clear`, `/resume`, `/model` and `/compact now` work only while
idle; the transcript is replaced only once the new session is live.

Enter sends (steers while a turn runs); Alt+Enter, Shift+Enter or Ctrl-J
insert a newline; Ctrl-C interrupts (twice to quit when idle); Esc recalls
held messages, else interrupts; ↑ on an empty editor edits the newest held
message, else recalls prompt history; Ctrl-R searches history (workspace,
then global); Ctrl-X Ctrl-S interrupts and sends the input; Ctrl-O toggles
the last reasoning block; PgUp/PgDn and Shift+↑/↓ scroll; Home/End jump when
the editor is empty.

The mouse is captured by default: the wheel scrolls and a click expands or
collapses a tool call's output (Ctrl-T for the latest). Dragging selects text
and copies it with OSC 52 on release (double click: a word, triple click: a
line; Ctrl-Y copies again or copies the last answer). Hold Shift for native
selection, or start with `--no-mouse`.

## Where things live

All state for a workspace lives inside it, under `<workspace>/.rkat/tui/`:
the realm (sessions, config, `tui.toml` written by the UI, approval ledger,
process custody, semantic index), the endpoint/cost sidecars
(`tui_endpoints/`), input history, unsent messages, notes and logs
(`logs/rkat-tui.log`). Nothing is keyed by the absolute path, so a renamed
or moved workspace still continues with `-c`. A `.gitignore` keeps the
directory out of commits.

The workspace is `--workspace <dir>` (`-w`), else the nearest ancestor of the
current directory that already has `.rkat/tui/` (the search stops at the git
root and never picks your home directory), else the current directory.
Running from your home directory requires an explicit `--workspace`.
`--realm`, `--isolated` and `--state-root` opt into the shared realm model
that `rkat` uses instead.

User-global files live in one config directory: `--config-dir <dir>`, else
`<--user-config-root>/.config/rkat-tui`, else `$XDG_CONFIG_HOME/rkat-tui`,
else `~/.config/rkat-tui` (on Windows `%APPDATA%\rkat-tui`). It holds
`tui.toml`, `AGENTS.md`, `SYSTEM.md`, `notes/`, `history` and the price-list
cache.

## Testing

The default lane needs no keys, no network and no sandbox (unit tests plus
headless runner tests over the real runtime-backed service with a scripted
LLM):

```bash
cargo test -p meerkat-tui
```

The confinement tests skip themselves, printing why, where the host's sandbox
backend is unavailable (for example Ubuntu's user-namespace restriction).

The live lanes are ignored by default and skip a provider whose key is not in
the environment:

```bash
ANTHROPIC_API_KEY=… OPENAI_API_KEY=… GEMINI_API_KEY=… cargo test -p meerkat-tui --test e2e_classifier -- --ignored
ANTHROPIC_API_KEY=… OPENAI_API_KEY=… GEMINI_API_KEY=… cargo test -p meerkat-tui --lib e2e_list_models -- --ignored
```

Inside this repository use `./scripts/repo-cargo` in place of `cargo`.
