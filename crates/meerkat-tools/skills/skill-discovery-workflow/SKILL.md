---
name: skill-discovery-workflow
description: How to browse, load, and use Meerkat skills during a session
requires_capabilities: [skills]
---

# Skill Discovery Workflow

Use skills when the available skill inventory contains domain guidance that
would improve the current turn. Skills are explicit instruction manuals; they
are discoverable by default but not automatically injected unless a session or
turn asked for them.

## Operating Rules

- Use `browse_skills` when the inventory is in collection mode, when the user
  mentions a domain, or when you suspect a built-in companion skill exists for
  a tool family.
- Use `load_skill` with a typed builtin `SkillKey` when you need the full
  operating guidance before calling a tool family. On the wire, pass the
  `source_uuid` and `skill_name` fields returned by `browse_skills`; do not
  synthesize a slash-delimited identifier.
- Use `skill_list_resources` and `skill_read_resource` for supporting files a
  skill exposes. Read only the resources needed for the current workflow.
- Use `skill_invoke_function` only for a function explicitly exposed by the
  selected skill. Its `arguments` must be a JSON object, and its output is
  wire-opaque skill-owned JSON.
- Treat tool descriptions as schema and capability summaries. Treat companion
  skills as the source for when, how, and why to use a tool family.
- Respect capability gating. If a skill is absent, continue with the tools that
  are actually available instead of assuming the capability exists.
- Do not preload broad skills just in case. Load only the skill needed for the
  current workflow.
- Source identity can be remapped by the runtime. Continue with the canonical
  key returned by load/resource/function operations rather than caching an old
  source UUID as permanent identity.

## Companion Skills

Companion skills are embedded skills owned by a Meerkat crate or tool family.
They are gated by `requires_capabilities` and teach agent behavior for a
capability such as WorkGraph, Schedule, shell, memory, comms, hooks, or builtin
utilities.
