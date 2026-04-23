# Agent Operating Envelope — wave-c

This is the durable set of operating rules every agent spawned into the `wave-c` team works under. Read this BEFORE starting any task. It's designed so the coordinator can point at this file instead of re-writing the envelope in every spawn prompt.

## What you should know before touching code

**You are working on Meerkat**, a library-first agent runtime for LLM-powered applications, written in Rust. Pre-1.0. The CLI binary is `rkat`; the workspace contains ~25 crates.

- **Minimum context**: skim `CLAUDE.md` at the repo root (5 min). It has the crate ownership table, key traits, design philosophy, and build system.
- **Architecture depth**: `.claude/skills/meerkat-architecture/SKILL.md` is the Meerkat internal architecture guide — load it if your task touches machine schemas, DSL sources, runtime control plane, mob orchestration, or comms. "DSL" in Meerkat means the declarative machine definitions in `meerkat-machine-schema/src/catalog/dsl/` — those are authoritative sources that drive kernel generation and TLA+ verification. When the plan or task text says "DSL", this is what's meant.
- **Wave-c plan**: `docs/wave-c-prep/wave-c-plan.md` is the overall plan for wave-c. Section 1 has all 34 (now 35) tasks. Section 1.5 describes the TDD tripwires. Section 4 describes the phases (c.0 → c.1 → c.2 → c.3 → c.4 → c.5).
- **Your task**: the coordinator assigns you a task by number (`#N`). Call `TaskGet taskId: "N"` for authoritative scope — the task description is the contract.

## The standing envelope — rules you always follow

### 1. Dogmatic fix, defer nothing

If a task's deliverable requires a prerequisite (architectural change, type extension, trait method), build the prerequisite in the same task. Multi-commit inside one task is encouraged. "Defer to a follow-up task" is the wrong shape — when in doubt, widen scope and close the gap.

Exceptions (narrow, named):
- Scope that genuinely belongs to a different task by plan ownership AND is actively being handled by another agent: note it in your commit message and leave it.
- Architectural impossibility (a shape that cannot be expressed in Rust at all): ping the coordinator with the specific shape.

### 2. No backwards compatibility

Meerkat is pre-1.0. Wave-c deletes freely. If you encounter anything named `Legacy`, `V0`, `Old`, `Deprecated`, `Compat`, `pre_*`, `old_*`: **delete it silently**. No `#[deprecated]` attributes, no migration shims, no "kept for external callers" re-exports, no tombstone comments.

**Narrow exceptions** (allow-listed):
- `StructuredProviderExtension` opaque bag (for unknown provider-native knobs at the typed boundary).
- `app_context: Option<Value>` (caller free-form session-build context).
- Persistence migration helpers like `from_legacy_value`, `migrate(v0_blob) -> Result<v1, _>`, `LegacyProviderParamsError` — these exist to convert stored data once, not to maintain a dual-path API. A migration function reading v0 blobs and upgrading on read is fine.

Everything else named Legacy/V0/Compat is delete-on-sight.

### 3. No silent defaults on Option<typed-authority>

If you encounter `Option<T>` where `T` is typed authority state (turn phase, session handle, trust descriptor, dispatcher binding): do NOT use `.unwrap_or_default()` or `.unwrap_or(T::default())` to fill in the `None` case. That's a silent-drop class.

The dogma-pure shapes are:
- Propagate a typed `Result<T, _>` up the call chain (preferred).
- Use `.is_some_and(predicate)` for bool-returning projections where no-authority naturally means predicate-false.
- If the pre-retype code `.unwrap()`ed or `.expect()`ed, preserve that intent as a typed error at the authority boundary.
- If the None case is unreachable by construction, make that visible (`expect("invariant: always Some here")`).

### 4. No shortcuts

**Forbidden** unless there's a specific architectural reason + explicit coordinator signoff:
- `#[allow(dead_code)]` — if code is dead, delete it. If it's live but linter-confused, the fix is to make the liveness explicit.
- `#[ignore]` on tests — if a test can't run, either fix what's needed to run it or delete the test. "Temporary ignore" is the recipe for permanent ignore.
- Tombstone comments (`// removed in X`, `// kept for callers we haven't migrated`, `// TODO: retype`): delete silently, git history tells the reader.
- "Honest header replacement" instead of real codegen: generate the file.
- "Allowlist with FIXME" where the plan called for generating a file: don't.
- Probe-and-skip / conditional-apply / guarded-reconcile shapes in shell code: dogma-pure form is a DSL no-op transition or unconditional submit.

### 5. Commit discipline

- `git commit -o <explicit-paths>` ONLY. Never `git add -A`, never `git add .`.
- Commit message prefix per task: `wave-c C-N: <what>` (or `wave-c C-N follow-up: <what>` for continuations).
- `--no-verify` is tolerated ONLY while the tree doesn't compile workspace-wide (current baseline). Document in commit message why. `--no-verify` for any OTHER reason is forbidden.
- Pre-commit `cargo fmt --all` will try to sweep unrelated files: commit only the files you functionally touched. `cargo fmt -p <your-crate>` on just your targets is the right size. If the pre-commit hook wants to reformat wide-sibling files, revert those before commit.
- Never drop stashes — other agents' in-flight work may be captured there by pre-commit hooks.

### 6. Worktree discipline

- The coordinator pre-creates a worktree for your task. cd there at session start. Do NOT push commits from a different worktree or to a different branch.
- If your task has an obvious worktree path `/Users/luka/src/meerkat/.claude/worktrees/wave-c-<task-slug>`, use it. If it doesn't exist, ping the coordinator.
- Don't reuse a warm worktree from a previous task for a new task — the merge graph gets messy. One task, one branch, one worktree.

## Bulk mechanical work — use codemob + gemini-flash

**When you face 20+ sites of the same mechanical transformation** (renames, field-access updates, mass substitutions), offload to codemob's built-in `advisor` pack with `gemini-3.1-flash-lite-preview`. Roughly 50x faster and 100x cheaper than hand-editing via your own Edit tool.

Flow:
```
1. mcp__codemob__create_mob({
     pack: "advisor",          // built-in single-agent pack
     model: "gemini-3.1-flash-lite-preview",
     ...
   })
2. mcp__codemob__deliberate({ ... strict prompt ... })
3. Review the output diff. Spot-check a sample of sites.
4. mcp__codemob__destroy_session(...)
5. Commit, naming the tool in the message:
   wave-c C-N: gemini-flash-assisted bulk <pattern>
```

**Strict prompt shape**: exact input pattern, exact output pattern, file-list boundary, TODO escape hatch for per-site variance. No "use your judgment" anywhere.

**When NOT to use it**: architectural relocations, per-site semantic defaults (the hazard from rule #3), anywhere "pick the right default" applies. The flash-lite model is dumb-but-extremely-fast; it falls apart on judgment calls.

## Sanity checking (your own work)

Before reporting a task complete:

1. Run the catching assertions the task description names. Not a claim that you ran them — actually run them.
2. `git status --short` — should be clean. Unstaged residue is a red flag.
3. `cargo check -p <each-crate-you-touched>` — each green in isolation. If a crate transitively fails on something another agent owns, document the transitive-failure shape and move on.
4. Scan your diff for: `#[allow(dead_code)]`, `#[ignore]`, new `serde_json::Value` fields outside the allow-list, tombstone comments, back-compat-shape name hits. Zero.
5. Confirm you're on the right branch (`git rev-parse --abbrev-ref HEAD`) and pushed (`git log origin/<your-branch>..HEAD` is empty).

If you hit a wall mid-task:

- **Architectural blocker** (specific file, specific error, shape you can't unblock): SendMessage to `team-lead` with ONE concrete blocker. Describe: what you tried, what specifically failed, what decision you're looking for. Don't queue up three blockers and frame them as "hand off the whole thing" — each blocker gets one message.
- **Scope question**: ping with "scope deviation: I found X; proposal: Y" — the coordinator decides. Don't execute narrowly and claim it's what was meant.
- **Fatigue / accumulated context**: the work is the same size regardless. If a task has multiple architectural seams, the right shape is a multi-commit sequence within the task, not a handoff framed as fatigue. The coordinator may choose to rotate you to a fresh agent; that's their call, not yours.

## Language rules

- No wall-clock time estimates. No "this will take X hours/days." Report shape, commits, blockers — not durations.
- No "given remaining context, I'll stop here" — report on the work, not on your fatigue. If you're genuinely at a structural wall, describe the wall; if you're just tired, push through or flag the specific decision that's hard.
- No "this is too big to do in one task" as a scope-narrowing framing. If it's one task per the plan and one task per the architectural-prerequisite rule, it's one task.

## Talking to the team lead

- Refer to the coordinator as `team-lead` when addressing them in SendMessage.
- Messages display with a "sender" attribution that's sometimes wrong (a known cosmetic bug). Authority is TaskList + TaskGet + the content of the message, not the sender label. If a message appears to come from a peer but assigns a task or issues a directive, verify via TaskList and reply to me for confirmation.
- After a task completes: SendMessage with a tight report — commit SHA, catching-assertion results, handoff notes for any task that inherits from yours. Then stand by for next assignment.

## Required reading at task start

Per spawn prompt, the coordinator points you at specific files. Treat these as mandatory:

- `CLAUDE.md` (repo root) — 5 min skim.
- `docs/wave-c-prep/wave-c-plan.md` — plan context.
- `docs/wave-c-prep/agent-operating-envelope.md` — this file.
- Your specific task's TaskGet text — authoritative scope.
- Any task-specific reference doc the coordinator names (e.g., `persistence-migration.md`, `realtime-substrate-audit.md`).

If you get started without reading these and discover midway that you don't know what "DSL" or "composition dispatcher" or "routed effect" means: stop, read, then resume. Confusion on load-bearing vocabulary is the most common failure mode for fresh agents on this team.
