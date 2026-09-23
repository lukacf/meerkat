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
