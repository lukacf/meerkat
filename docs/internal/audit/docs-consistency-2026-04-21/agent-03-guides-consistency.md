# Agent 03 Findings

## Scope reviewed

- Primary scope: `docs/guides/*`
- Navigation/context checked for consistency: `docs/docs.json`
- Audit focus: guide categorization, ordering, structure, cross-link quality, audience fit, and guide-vs-concept/reference boundaries

## Files reviewed

- `docs/docs.json`
- `docs/guides/auth.mdx`
- `docs/guides/cd-and-distribution.md`
- `docs/guides/comms.mdx`
- `docs/guides/hooks.mdx`
- `docs/guides/memory.mdx`
- `docs/guides/mini-surfaces.mdx`
- `docs/guides/mobpack.mdx`
- `docs/guides/mobs.mdx`
- `docs/guides/realtime.mdx`
- `docs/guides/scheduling.mdx`
- `docs/guides/self-hosted-gemma4.mdx`
- `docs/guides/skills.mdx`
- `docs/guides/structured-output.mdx`

## Findings

### 1. Guide categorization and nav placement are inconsistent with the actual content

- `docs/docs.json:20-46` places `guides/auth` under **Getting started**, while much more advanced material such as realm/binding design, per-member overrides, refresh coordination, and audit logging lives in the page itself (`docs/guides/auth.mdx:43-289`). This page reads like a substantial operational guide, not an onboarding page.
- `docs/docs.json:34-45` orders the Guides group as `self-hosted-gemma4`, `structured-output`, `memory`, `hooks`, `skills`, `mini-surfaces`, `comms`, `mobs`, `mobpack`, `realtime`, `scheduling`. That sequence does not follow a clear user journey such as setup -> runtime behavior -> orchestration -> deployment. It currently front-loads niche/specialized topics (`self-hosted-gemma4`, `mini-surfaces`) ahead of broader platform capabilities.
- `docs/guides/cd-and-distribution.md` lives physically under `docs/guides/`, but `docs/docs.json:92-99` surfaces it in `Reference > Architecture`. That mismatch makes the Guides directory harder to reason about and weakens the meaning of both “guides” and “reference.”

Impact:
- Readers cannot reliably infer audience or depth from where a page appears.
- The Guides section feels assembled incrementally rather than intentionally curated.

### 2. Several pages in `docs/guides/` are really reference/spec pages, not task-oriented guides

- `docs/guides/structured-output.mdx` spends most of its first two-thirds on type inventories and wire structures before reaching actionable usage. The page starts with `## Schema types` at `docs/guides/structured-output.mdx:31`, then goes through `OutputSchema`, `MeerkatSchema`, `SchemaFormat`, `SchemaCompat`, `SchemaWarning`, `CompiledSchema`, `SchemaError`, wire parameters, and `RunResult` fields before the practical SDK section ends at `docs/guides/structured-output.mdx:325-380`. This is excellent reference material, but weak as a guide landing page.
- `docs/guides/skills.mdx` similarly opens with source precedence, transport/source types, XML rendering formats, and injection limits (`docs/guides/skills.mdx:9-188`) before getting to “Invoking skills” at `docs/guides/skills.mdx:236`. The structure teaches implementation details before teaching the user how to accomplish the task.
- `docs/guides/hooks.mdx` is highly spec-shaped: enum tables, payload structures, policy matrices, runtime config, and SDK internals dominate the page (`docs/guides/hooks.mdx:9-375`). There is one short “How to add a hook” section (`docs/guides/hooks.mdx:53-75`), but it is surrounded by reference-heavy content.
- `docs/guides/comms.mdx` is 793 lines and behaves like a combined concept + reference + operations document. It includes crate ownership, cryptographic envelope details, transport codec rules, typed peer lifecycle notices, security notes, and SDK/programmatic material (`docs/guides/comms.mdx:26-776`). It is informative, but it no longer fits a normal “How do I do X?” guide shape.

Impact:
- The “Guides” label stops being predictive.
- Pages become hard to scan because they mix onboarding, architecture, and API reference in one place.

### 3. Audience mismatch is strongest in `cd-and-distribution.md`, which reads like an internal release rulebook

- The page title/frontmatter says “CD and distribution,” but the body immediately becomes a release governance document: `# CD and Distribution Rulebook` (`docs/guides/cd-and-distribution.md:7`), release jobs (`:98-135`), credential setup (`:137-161`), hard constraints (`:169-176`), and a release checklist (`:178-186`).
- `docs/guides/cd-and-distribution.md:139` says, “You only mentioned Cargo is already configured,” which is conversation-specific language and not appropriate for durable docs.
- `docs/guides/cd-and-distribution.md:188-191` explicitly says, “This is a plan/rulebook currently,” confirming that the page is closer to internal process documentation than a user-facing guide.
- It is also the only guide file in this directory using `.md` instead of `.mdx`, and it adds an explicit H1 after frontmatter (`docs/guides/cd-and-distribution.md:7`), unlike most other guide pages.

Impact:
- This page breaks the voice and audience expectations of the section.
- It likely belongs in architecture/process/reference docs, or should be rewritten as a narrowly scoped release guide for contributors.

### 4. Cross-linking is inconsistent: markdown links are healthy, but several pages end with weak or non-doc links

- Positive: I found **no missing internal markdown link targets** in `docs/guides/*`. Existing markdown links resolve.
- Weak-link cases still exist:
  - `docs/guides/realtime.mdx:326-327` ends with raw filesystem-style references to `.claude/skills/meerkat-architecture/references/realtime-attachment.md` and `docs/architecture/identity-first-live-voice-proposal.md`. These are not rendered as normal docs links and one points outside the published docs tree.
  - `docs/guides/auth.mdx:289-290` ends with plain text `Design doc \`/architecture/meerkat-runtime-dogma\`` rather than a normal clickable markdown link.
  - `docs/guides/mobpack.mdx:131-138` points readers to raw repo paths like `examples/028-mobpack-release-triage-sh` instead of the docs examples pages (`/examples/mobpack`, `/examples/wasm`) or clickable repository links.
- Cross-link density is also uneven. Some pages have good `See also` sections (`auth`, `comms`, `memory`, `mobs`, `realtime`, `scheduling`, `self-hosted-gemma4`), while other major guides have none (`hooks`, `skills`, `structured-output`, `mini-surfaces`, `mobpack`).

Impact:
- The section does not offer a consistent “next step” experience.
- Some of the best adjacent material exists, but readers are not reliably routed to it.

### 5. Guide scaffolding is inconsistent across the section

- Some pages use a helpful guide pattern:
  - `docs/guides/self-hosted-gemma4.mdx` has “What you configure,” “Quick use,” provider-specific setup, a validation checklist, and `See also` (`docs/guides/self-hosted-gemma4.mdx:26-266`).
  - `docs/guides/memory.mdx` has a clear problem statement, feature gating, mechanics, examples, and `See also` (`docs/guides/memory.mdx:7-272`).
  - `docs/guides/auth.mdx` does a strong job of splitting “fast path” vs “powerful path” (`docs/guides/auth.mdx:24-105`).
- Other pages stop without a guide-style ending or orientation:
  - `docs/guides/hooks.mdx` ends on SDK override examples with no `See also` or “when to use which runtime” summary (`docs/guides/hooks.mdx:364-375`).
  - `docs/guides/skills.mdx` ends on configuration layering (`docs/guides/skills.mdx:355-372`) rather than closing with related examples or recommended starting paths.
  - `docs/guides/structured-output.mdx` ends on Rust handling code (`docs/guides/structured-output.mdx:365-380`) with no follow-on links to examples or API docs.
  - `docs/guides/mini-surfaces.mdx` ends with internal composition-seam language (`docs/guides/mini-surfaces.mdx:143-151`) and no next-step navigation.
  - `docs/guides/mobpack.mdx` ends with example repo paths (`docs/guides/mobpack.mdx:131-138`) rather than a standard closeout.
- There is also light formatting inconsistency:
  - `docs/guides/scheduling.mdx:7` and `docs/guides/cd-and-distribution.md:7` add explicit H1 headings after frontmatter while most guide pages do not.

Impact:
- Readers cannot count on a common rhythm across guides.
- The section feels less curated than Concepts, API, or Reference.

## Suggested follow-ups

- Rework the Guides navigation into explicit subgroups such as:
  - Setup and configuration: `auth`, `self-hosted-gemma4`
  - Runtime behavior: `structured-output`, `memory`, `hooks`, `skills`, `realtime`, `scheduling`
  - Multi-agent orchestration: `comms`, `mobs`, `mobpack`
  - Build and distribution: `mini-surfaces`, `cd-and-distribution` if it remains user-facing
- Move or rewrite `docs/guides/cd-and-distribution.md`:
  - Best option: move it conceptually into architecture/contributor process docs.
  - Alternative: rewrite it into a contributor-facing “How to release Meerkat” guide and remove conversational/internal wording.
- Split the most reference-heavy guide pages into guide + reference pairs:
  - `structured-output`: “How to use structured output” + schema/reference appendix
  - `skills`: “How to create/load/invoke skills” + runtime/reference details
  - `hooks`: “How to add a hook” + hook API reference
  - `comms`: guide overview + separate protocol/reference page
- Standardize a guide template for this section:
  - What this guide is for
  - When to use it / prerequisites
  - Quickstart or first successful example
  - Deeper details
  - Validation/troubleshooting
  - See also
- Normalize link quality:
  - Replace plain text design-doc paths in `auth`, `realtime`, and `mobpack` with clickable docs links where possible.
  - Prefer docs example pages over raw repo paths when the examples already have published docs counterparts.
- Keep `auth.mdx`, `memory.mdx`, and `self-hosted-gemma4.mdx` as reference models for future guide rewrites. They are the clearest examples in this section of pages that still feel like guides while carrying substantial detail.
