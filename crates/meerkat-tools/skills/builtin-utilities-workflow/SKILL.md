---
name: builtin-utilities-workflow
description: How to use Meerkat built-in utility tools safely and productively
requires_capabilities: [builtins]
---

# Builtin Utilities Workflow

Use builtin utility tools for local, concrete actions that support the current
turn. They are not a durable planning system by themselves.

## Operating Rules

- Use `datetime` when relative dates, local time, deadlines, or schedule inputs
  need grounding.
- Use `apply_patch` for intentional source edits and keep patches scoped.
- Use `view_image` when the user references a local image or when visual
  verification matters.
- Use `generate_image` only when the runtime exposes image generation and the
  task genuinely asks for new or edited imagery.
- Use `blob_load_file` to import a project file into the realm blob store,
  `blob_save_file` to write a blob into the project, and `blob_inspect` to read
  blob metadata/content. Do not use blob tools as a replacement for normal
  text inspection.
- Use `tool_catalog_search` and `tool_catalog_load` when deferred tool entries
  exist and the current visible tool list is not enough. Search reads the
  catalog; load requests visibility at the next admitted tool boundary and can
  be refused by session filters or missing scope authority.

## Boundaries

- Builtin task tools are private/local task tracking; WorkGraph is the shared
  durable commitment graph.
- Shell has separate operational safety guidance in `shell-patterns`.
- Some utility tools are model, store, profile, or runtime conditional. If a
  tool is not visible, do not assume the underlying capability exists.
- Read-only tool policy admits only tools explicitly classified read-only.
  Shell, patching, image generation, blob writes, and undeclared tools remain
  unavailable under that policy.
