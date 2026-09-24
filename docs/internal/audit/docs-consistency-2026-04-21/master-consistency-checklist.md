# Master Consistency Checklist

This checklist synthesizes the four consistency-focused review reports in this
folder into one action plan for docs information architecture and cross-section
consistency.

## Source reports

- `agent-01-overview-and-structure.md`
- `agent-02-concepts-consistency.md`
- `agent-03-guides-consistency.md`
- `agent-04-examples-consistency.md`

## Priority 0: Decide the documentation information architecture

- [x] Decide the primary onboarding spine for new readers.
  Decision:
  `Introduction -> Quickstart -> Examples gallery -> choose a surface or workflow -> Concepts as needed`

  Intended meaning of each step:
  - `Introduction`: what Meerkat is, who it is for, and what surfaces exist
  - `Quickstart`: get to the first successful run as fast as possible
  - `Examples gallery`: the central real-code hub and the canonical “what should I do next?” page
  - surface/workflow branch: Rust, Python, TypeScript, CLI, REST/RPC/MCP, or multi-agent/deployment examples
  - Concepts: deepen mental models once the reader has a concrete use path

  Auth remains part of onboarding, but as a practical prerequisite linked from `Quickstart`, not as the dominant spine after the first successful run.
  Source: agent 01 + assistant decision

- [x] Decide whether `examples/gallery` belongs in `Getting started`.
  Decision:
  yes, it belongs there because it is the real code-examples hub.
  Follow-up work is about role/positioning, not category removal.
  Source: agents 01 and 04 + user direction

- [x] Decide the intended top-level audience split for the site.
  Decision:
  Meerkat serves all of the above at once: it is library-first, but it also ships a practical product layer.
  Consequence:
  examples and major feature docs should continue to treat the main surfaces as peers rather than privileging one as the “real” path.
  Source: agent 01 + user direction

- [x] Decide whether the architecture corpus under `docs/architecture/` is part of the public docs navigation or effectively internal/maintainer material.
  Decision:
  it is part of the public docs navigation.
  Consequence:
  it needs an intentional public IA and index structure rather than being a hidden file tree.
  Source: agent 01 + user direction

- [x] Decide where operational/process docs belong.
  Decision:
  do not force a hard end-user vs developer split. Meerkat is a composable library, so operational and builder-facing material can still be part of the public docs surface when it serves real product usage.
  Consequence:
  sectioning should be based more on task shape and system level than on a rigid “user vs developer” boundary.
  Source: agents 01 and 03 + user direction

## Navigation And Section Layout

- [x] Rework `Documentation > Guides` ordering into a coherent progression instead of the current mixed sequence.
  Strong candidate subgroups:
  setup/configuration, runtime behavior, multi-agent orchestration, deployment/operations.
  Source: agents 01 and 03

- [x] Refactor `Self-hosted Gemma 4` into a clearer docs shape.
  Preferred direction:
  create a general `Self-hosting models` guide that explains the shared Meerkat model-hosting abstraction, setup patterns, server choices, and validation flow, then use Gemma 4 as the concrete worked example inside that page.
  Alternative:
  keep Gemma 4 as a specific example, but pair it with a generic self-hosting guide and avoid surfacing the Gemma page as the primary generic guide entry.
  Source: agents 01 and 03

- [x] Reclassify or regroup `cd-and-distribution`.
  Options:
  move to `Operations` / `Maintainers`,
  move to `Reference`,
  or rewrite as a true user-facing distribution guide.
  Source: agents 01 and 03

- [x] Clean up `Reference` category fit.
  It currently mixes product reference, architecture, governance, and release/runbook material.
  Source: agent 01

- [x] Add a docs-wide “choose your path” page or section near the top of the site.
  It should route readers to:
  Rust SDK, Python/TypeScript SDKs, CLI, REST/RPC/MCP, or multi-agent docs.
  Source: agent 01

- [x] Keep `examples/gallery` in `Getting started`, but clarify its role in the onboarding spine.
  It should read as the central real-code hub, not as a duplicate or competing next-step destination.
  Source: agents 01 and 04 + user direction

## Concepts Section

- [x] Add a Concepts landing page or conceptual map.
  The current Concepts section is a set of leaf pages with no overview explaining how they fit together.
  Source: agent 02

- [x] Add `Composability` as a first-class concept.
  This should explain how Meerkat is both a composable library and a practical product layer, and why surfaces/examples are treated as peers rather than as separate products.
  Source: user direction

- [x] Tighten the ownership boundaries between:
  `realms`, `sessions`, and `configuration`.
  Suggested ownership:
  realms = identity/isolation/backend pinning,
  sessions = lifecycle/history/concurrency/archive,
  configuration = config structure/precedence/CAS.
  Source: agent 02

- [x] Refocus `providers.mdx` so it is primarily a concept page, not a hybrid concept + setup + reference page.
  Move or split out setup tables, env-var recipes, and parameter/reference-heavy material as needed.
  Source: agent 02

- [x] Reframe `tools.mdx` so the conceptual model comes before the Rust trait/API mechanics.
  The page currently feels implementation-first.
  Source: agent 02

- [x] Add `See also` sections to concept pages that currently have weak onward navigation.
  Highest priority:
  `providers.mdx`, `tools.mdx`.
  Cross-link targets should include Auth, Built-in tools, MCP, Hooks, Memory, Comms, and Mobs where relevant.
  Source: agent 02

- [x] Expand the Concepts layer to cover currently concept-heavy topics that live only under Guides.
  Strong candidates:
  auth/bindings,
  memory/compaction,
  comms,
  mobs,
  scheduling,
  realtime.
  Source: agents 01 and 02

## Guides Section

- [x] Rework guide categorization so placement reflects audience and task shape.
  Candidate structure:
  setup/configuration,
  runtime behavior,
  orchestration,
  deployment/ops.
  Source: agent 03

- [x] Split the most reference-heavy guide pages into guide + reference layers.
  Strong candidates:
  `structured-output`,
  `skills`,
  `hooks`,
  `comms`.
  Source: agent 03

- [x] Standardize a guide template across the section.
  Suggested structure:
  what this guide is for,
  when to use it / prerequisites,
  first successful path,
  deeper details,
  validation/troubleshooting,
  see also.
  Source: agent 03

- [x] Normalize guide endings.
  Several pages still end on raw implementation detail instead of giving readers a next step.
  Highest-priority targets:
  `hooks`,
  `skills`,
  `structured-output`,
  `mini-surfaces`,
  `mobpack`.
  Source: agent 03

- [x] Replace weak/non-doc links and plain-text path references in Guides with intentional docs navigation.
  Highest-priority targets:
  `auth`,
  `realtime`,
  `mobpack`.
  Source: agent 03

- [x] Use the strongest current guides as style references for future rewrites.
  The reports consistently call out:
  `auth.mdx`,
  `memory.mdx`,
  `self-hosted-gemma4.mdx`
  as better examples of guide-shaped pages.
  Source: agent 03

## Examples Section

- [x] Establish a consistent “start here / next step” pattern for every examples page.
  Add at least one entry-point note near the top and one onward link near the bottom.
  Source: agent 04

- [x] Define a learning-path spine for Examples.
  Good baseline today:
  `gallery -> sessions -> tools/skills/memory -> comms -> mobs -> mobpack -> wasm`
  but it is not expressed clearly inside the pages themselves.
  Source: agent 04

- [x] Rework `gallery.mdx` so the final bucket is not a catch-all.
  Split `Portable deployment` into more coherent groups such as:
  deployment/packaging,
  browser/WASM apps,
  integrations/full applications.
  Source: agent 04

- [x] Reorder `examples/tools.mdx`.
  Put broadly useful flows first:
  built-in tools,
  MCP registration,
  live controls,
  tool scoping,
  then Rust custom tools later.
  Source: agent 04

- [x] Simplify `examples/skills.mdx` into a clearer progression.
  Recommended order:
  discover -> inspect -> preload for session -> per-turn injection.
  Avoid repeating `--skill` explanations across multiple sections.
  Source: agent 04

- [x] Strengthen the advanced cross-links between:
  `examples/mobs`,
  `examples/mobpack`,
  `examples/wasm`.
  Readers should be able to follow the multi-agent -> package -> browser path from inside the pages.
  Source: agent 04

- [x] Mark intentionally surface-specific example pages explicitly.
  Add short framing notes near the top of:
  `examples/mobpack.mdx`,
  `examples/wasm.mdx`.
  Source: agent 04

- [x] Clarify the CLI hook-override story in `examples/hooks.mdx`.
  The page currently mixes “CLI does not expose this” and “here is a CLI override example” patterns in a way that undermines trust.
  Source: agent 04

- [x] Treat the strongest examples pages as templates for the rest.
  Best current structural models:
  `examples/sessions.mdx`,
  `examples/comms.mdx`,
  `examples/memory.mdx`.
  Source: agent 04

## Cross-Cutting Consistency Work

- [x] Decide and document the canonical difference between:
  Concepts,
  Guides,
  Examples,
  Reference,
  Architecture,
  and Maintainer/Operations material.
  This is the root consistency issue beneath many of the local page problems.
  Source: all four agents

- [x] Define section-level content rules and apply them systematically.
  For example:
  Concepts = mental model first,
  Guides = task/procedure first,
  Examples = runnable progression first,
  Reference = exact surface/shape lookup,
  Architecture = rationale/ownership.
  Source: all four agents

- [x] Normalize cross-link behavior across the docs set.
  Every page should make it clear:
  what this page assumes,
  what page should be read first,
  and what page should be read next.
  Source: all four agents

## Suggested Execution Order

- [x] Phase 1: decide the target docs taxonomy and audience routing
- [x] Phase 2: restructure nav/groups in `docs/docs.json`
- [x] Phase 3: add missing concept/index pages and tighten Concept boundaries
- [x] Phase 4: split/refactor the most reference-heavy Guides
- [x] Phase 5: rebuild the Examples learning path and cross-links
- [x] Phase 6: clean up remaining category mismatches in Reference/Architecture/Operations
