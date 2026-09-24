# Agent 02 Findings

## Scope reviewed

- `docs/concepts/*`
- Adjacent navigation and nearby docs only where needed to judge information architecture, conceptual boundaries, and cross-linking

## Files reviewed

- `docs/concepts/realms.mdx`
- `docs/concepts/sessions.mdx`
- `docs/concepts/configuration.mdx`
- `docs/concepts/tools.mdx`
- `docs/concepts/providers.mdx`
- `docs/docs.json`
- `docs/quickstart.mdx`
- `docs/guides/auth.mdx`
- `docs/guides/memory.mdx`
- `docs/guides/comms.mdx`
- `docs/guides/mobs.mdx`

## Findings

### 1. The `realms` / `sessions` / `configuration` cluster is internally consistent, but their boundaries are still too porous

This is the strongest part of the Concepts section overall. All three pages consistently use the same core mental model: realm-scoped state, deterministic sharing, and cross-surface behavior. The mutual linking is also good in this cluster: `docs/concepts/realms.mdx:90-95`, `docs/concepts/sessions.mdx:76-80`, and `docs/concepts/configuration.mdx:105-109`.

The problem is that each page still carries pieces of the others' source-of-truth material:

- `docs/concepts/realms.mdx:73-82` explains config envelope fields and `expected_generation`
- `docs/concepts/configuration.mdx:39-56` explains the same config envelope fields and CAS behavior in more detail
- `docs/concepts/sessions.mdx:57-66` repeats backend pinning already covered in `docs/concepts/realms.mdx:52-71`

That overlap makes the section feel repetitive rather than intentionally layered. A reader can infer that `realms` is the identity model, `sessions` is the lifecycle model, and `configuration` is the settings model, but the pages do not enforce those boundaries cleanly enough yet.

### 2. The Concepts section lacks an overview page or conceptual map, so page order has to do too much work

`docs/docs.json:33-40` defines the entire Concepts group as five leaf pages:

- `concepts/realms`
- `concepts/sessions`
- `concepts/tools`
- `concepts/providers`
- `concepts/configuration`

There is no landing page that explains how these concepts relate, which audience should read them, or which topics are foundational versus optional. As a result, the nav order implicitly becomes the architecture, but the current order mixes three tightly related runtime/state concepts with two broader capability/integration topics at the same level.

This especially shows up for new readers coming from `docs/quickstart.mdx`, where the first conceptual guidance is split between providers/auth (`docs/quickstart.mdx:40-42`) and realms (`docs/quickstart.mdx:109-111`). The documentation works, but the Concepts section does not currently provide a “how to think about Meerkat” entry point.

### 3. `providers.mdx` is doing concept work, setup work, auth-adjacent work, and catalog/reference work all at once

The page starts with a valid concept statement in `docs/concepts/providers.mdx:7`, but most of the body is operational:

- provider setup tabs and model tables in `docs/concepts/providers.mdx:9-89`
- environment variable precedence in `docs/concepts/providers.mdx:91-103`
- SDK Cargo feature flags in `docs/concepts/providers.mdx:105-126`
- provider-specific parameters in `docs/concepts/providers.mdx:128-163`
- model catalog mechanics in `docs/concepts/providers.mdx:165-179`
- a guide-style CTA in `docs/concepts/providers.mdx:192-194`

That makes the page useful, but conceptually muddy. It is not clear whether “Providers” is supposed to teach the mental model of provider abstraction, serve as a setup guide, or act as a compatibility/reference page.

There is also a cross-link gap with auth. `docs/quickstart.mdx:40-42` sends readers to both Auth and Providers, and `docs/guides/auth.mdx:7-23` explains credential resolution in a way that is conceptually upstream of large parts of `providers.mdx`. But `providers.mdx` does not link back to Auth anywhere, even though it discusses env-var precedence and self-hosted credentials. That creates terminology drift around “provider setup” versus “credential resolution” versus “bindings.”

### 4. `tools.mdx` reads more like a Rust implementation + API operations page than a concept page

The opening move of the page is a Rust trait implementation example in `docs/concepts/tools.mdx:9-46`. Later sections dive into specific runtime mutation mechanics and RPC method names in `docs/concepts/tools.mdx:86-114`. That material is accurate and useful, but it is pitched at implementers and API consumers more than concept-level readers.

This causes an audience mismatch inside the Concepts section:

- `realms`, `sessions`, and `configuration` explain mental models first
- `tools` explains code shape and control surfaces first

The page also has weaker conceptual cross-linking than the other concept pages. It links to the MCP reference and built-in tools reference (`docs/concepts/tools.mdx:57`, `docs/concepts/tools.mdx:72`, `docs/concepts/tools.mdx:104`, `docs/concepts/tools.mdx:114`), but it has no “See also” section and does not cross-link to other concept-adjacent topics it names directly:

- built-in comms and mob categories in `docs/concepts/tools.mdx:68-70`
- hooks in `docs/concepts/tools.mdx:136`

That leaves the reader without a clear path from “tools as a core concept” to related core behaviors such as hooks, comms, mobs, scheduling, or memory-backed tools.

### 5. The Concepts section feels conceptually incomplete because several foundational topics live only under Guides

The current Concepts group covers runtime identity, lifecycle, settings, tools, and providers, but several equally foundational topics are outside it:

- Auth is introduced as a first-run concern in `docs/quickstart.mdx:40-42`, yet it lives under `docs/guides/auth.mdx`
- Memory and compaction are clearly described as core long-running conversation machinery in `docs/guides/memory.mdx:7-12`
- Inter-agent communication is a platform capability with its own mental model in `docs/guides/comms.mdx:13-25`
- Mobs is explicitly framed as a core orchestration model in `docs/guides/mobs.mdx:7-9`

At the nav level, those pages are all classified as Guides rather than Concepts in `docs/docs.json:43-56`. That is not wrong for task-oriented walkthroughs, but it leaves the Concepts section incomplete as an architecture map of the product.

The result is that the current Concepts section explains the single-agent runtime model well, but it underrepresents identity/auth, long-horizon memory behavior, and multi-agent coordination as first-class concepts.

### 6. Cross-linking quality is uneven across the section

The realm/session/config pages cross-link well and reinforce each other. That area looks good.

By contrast:

- `docs/concepts/providers.mdx` ends with a single guide CTA (`docs/concepts/providers.mdx:192-194`) instead of a broader related-links block
- `docs/concepts/tools.mdx` has no “See also” section at all
- `docs/concepts/sessions.mdx:30` links to realms for default surface behavior, but the reverse side of that relationship is stronger than the tools/providers side of the concept graph

This unevenness matters because the Concepts section is already small. When one page lacks explicit onward paths, readers are more likely to misclassify the topic or miss adjacent concepts entirely.

## Suggested follow-ups

- Add a short Concepts landing page or overview page that explains the conceptual map: identity (`realms`), lifecycle (`sessions`), settings (`configuration`), capability surface (`tools`), and model/provider abstraction (`providers`).
- Tighten boundaries between `realms`, `sessions`, and `configuration` so each page owns one primary concept:
  - `realms`: identity, isolation, sharing, backend pinning
  - `sessions`: lifecycle, history, concurrency, archiving
  - `configuration`: config structure, precedence, merge semantics, CAS
- Split or refocus `docs/concepts/providers.mdx`:
  - keep provider abstraction, capability differences, and model resolution as the concept page
  - move setup tables, env-var recipes, and some parameter/reference material to guides or reference pages if possible
- Reframe `docs/concepts/tools.mdx` so the first sections explain the tool model conceptually before dropping into Rust trait examples and live RPC mutation details.
- Add explicit related-links sections to `providers.mdx` and `tools.mdx`, including at minimum:
  - Auth
  - Built-in tools reference
  - MCP reference
  - Hooks
  - Memory and compaction
  - Comms / Mobs where applicable
- Consider promoting or duplicating concept-heavy guides into the Concepts layer, especially:
  - auth / bindings
  - memory and compaction
  - inter-agent communication
  - mobs
