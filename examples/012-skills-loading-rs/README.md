# 012 — Skills Loading (Rust)

Skills inject domain-specific knowledge into agents at runtime. They define
behavioral patterns, review checklists, output formats, and tool usage
guidance — all without changing agent code.

## Concepts

- Skills as composable behavioral modules
- Direct use of `InMemorySkillSource` and `FilesystemSkillSource`
- Source identity records and canonical `SkillKey` resolution
- Composing named sources with `CompositeSkillSource`
- Wiring `SkillRuntime` into `AgentBuilder`
- Explicit typed per-turn activation with `pending_skill_references`

## Skill Sources
These labels describe adapters, not TOML `source` values.

| Adapter | Use Case |
|--------|----------|
| In-memory | Quick experiments, tests |
| Filesystem | Project-local skills checked into git |
| `git` | Shared skills across teams/repos |
| `http` | Dynamic skills from a skill registry |

This runnable example constructs inline and filesystem sources. The architecture
section printed by the program also shows the available Git, HTTP, embedded,
and external source adapters; it does not fetch a Git or HTTP skill.

The filesystem skill lives in a direct `security-auditor/SKILL.md` child with
matching lowercase frontmatter, and its source UUID matches the named source.
Skill identity is `(source_uuid, skill_name)`: equal names from distinct sources
do not automatically shadow one another.

Engine registration, model-visible inventory, and body activation are separate.
This example prints inventory to the host terminal, keeps an empty tool
dispatcher, and explicitly activates only the canonical Rust-review key before
the run. The model receives that body's typed `SkillContext`; no slash text
parser or on-demand skill tools are implied.

The printed repository configuration uses `[[skills.repositories]]` with a
name, stable `source_uuid`, and `type = "filesystem"` or `type = "git"`.
The printed CLI command preloads the embedded `builtin-utilities-workflow`,
whose builtins-only requirements match the default Safe tools:

```bash
rkat run --skill builtin-utilities-workflow "Explain the builtin utility workflow."
```

`shell-patterns` additionally requires shell access; the sample SKILL.md shows
that requirement but is not the least-privilege CLI preload example.

## Check a released binary without a model call

On Linux x86_64, set `BIN` to the absolute directory containing `rkat` and
`rkat-rpc` extracted from the **same release**, after verifying both archives
against its `checksums.sha256`. Set `SRC` to an absolute path to a checkout of
the matching tag. For example, use both `v0.9.16` binaries and the `v0.9.16`
source. This check needs Bash, Python 3 and local loopback access;
it does not need a provider key. The subshell uses a fresh project and home.

```bash
(
  set -eu
  : "${BIN:?set BIN to the release binaries}" "${SRC:?set SRC to the tagged source}"
  scratch=$(mktemp -d)
  trap 'rm -rf -- "$scratch"' EXIT
  mkdir "$scratch/home" "$scratch/project" "$scratch/tmp"
  cd "$scratch/project"
  clean=(env -i PATH=/usr/bin:/bin HOME="$scratch/home" TMPDIR="$scratch/tmp")
  "${clean[@]}" "$BIN/rkat" --isolated skill list
  "${clean[@]}" "$BIN/rkat" --isolated skill inspect \
    --source-uuid 00000000-0000-4b11-8111-000000000001 builtin-utilities-workflow
  "${clean[@]}" python3 "$SRC/scripts/check-release-binary-registrations" \
    --source-root "$SRC" "$BIN/rkat-rpc"
)
```

**CLI inventory and inspection.** On 0.9.16 the default list has six built-in
skills. `builtin-utilities-workflow` is not one of those six, but inspection
by its canonical key prints its body. Neither command starts a session or
proves that the CLI preloaded that body into a prompt.

**Member prompt.** The separate `rkat-rpc` check spawns a member with comms
enabled. Look for `the mob member it spawned carries the` in the successful
`RUN` result. The check finds the first Markdown heading from the registered
`mob-communication` skill body in that member's stored prompt. This does not
compare the whole body, test the CLI skill above, or show that a model used
the instructions.

## When to Use Skills
- Same agent logic, different domains (code review vs. API design)
- Team-shared behavioral standards
- A/B testing different agent behaviors
- Swapping expertise at runtime via canonical `skill_refs`

## Run
```bash
# From the repository root
ANTHROPIC_API_KEY=sk-... ./scripts/repo-cargo run -p meerkat \
  --example 012-skills-loading --features jsonl-store,skills
```
