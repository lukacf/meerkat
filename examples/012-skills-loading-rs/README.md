# 012 — Skills Loading (Rust)

Skills inject domain-specific knowledge into agents at runtime. They define
behavioral patterns, review checklists, output formats, and tool usage
guidance — all without changing agent code.

## Concepts
- Skills as composable behavioral modules
- Skill sources: inline, file, git, HTTP
- Skill references in session creation
- Composing multiple skills in one agent

## Skill Sources
| Source | Use Case |
|--------|----------|
| `inline` | Quick experiments, tests |
| `path` | Project-local skills checked into git |
| `git` | Shared skills across teams/repos |
| `http` | Dynamic skills from a skill registry |

## When to Use Skills
- Same agent logic, different domains (code review vs. API design)
- Team-shared behavioral standards
- A/B testing different agent behaviors
- Swapping expertise at runtime via canonical `skill_refs`

## Run
```bash
ANTHROPIC_API_KEY=sk-... cargo run --example 012_skills_loading
```
