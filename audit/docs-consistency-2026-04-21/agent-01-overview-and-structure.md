# Scope reviewed

- Docs site navigation and tab/group structure in `docs/docs.json`
- Primary landing flow in `docs/introduction.mdx` and `docs/quickstart.mdx`
- High-level category fit across the public docs site
- Reachability of major docs sections from the configured navigation

# Files reviewed

- `docs/docs.json`
- `docs/introduction.mdx`
- `docs/quickstart.mdx`
- `docs/examples/gallery.mdx`
- `docs/reference/architecture.mdx`
- `docs/guides/cd-and-distribution.md`
- Docs tree inventory under `docs/`

# Findings

## 1. The getting-started path is split across too many entry points and duplicates the same destination

The onboarding flow is not yet opinionated enough for a new reader. `docs/docs.json:29-30` puts `examples/gallery` directly inside the `Getting started` group, while `docs/introduction.mdx:56-70` repeats the examples entry twice with two separate cards (`Hello examples` and `Examples`) that both go to `/examples/gallery`. `docs/quickstart.mdx:142-148` then sends readers to `/examples/sessions` instead of the gallery page used elsewhere.

This creates three different "next step" stories:

- browse a gallery of repository examples
- jump into feature-by-feature examples
- continue through auth/setup docs

That is not a factual problem, but it is an information-architecture problem: the site does not clearly answer "what should I do after the first successful run?" A newcomer can land on a broad GitHub-backed gallery (`docs/examples/gallery.mdx:7-23`) before they have a mental model of concepts, surfaces, or supported workflows.

What looks good: `introduction` and `quickstart` are present and correctly surfaced first in nav, so the site already has the right foundational pages. The problem is the branching after that first step.

## 2. Group ordering inside `Documentation` leads with niche guides before foundational workflows

The `Guides` group in `docs/docs.json:43-56` starts with `guides/self-hosted-gemma4`, then moves into structured output, memory, hooks, skills, mini surfaces, comms, mobs, realtime, and scheduling. That ordering is backwards for most readers. A self-hosted Gemma setup is a specialized deployment path, but it is currently the first guide exposed after the concepts section.

More broadly, the site has concept pages for `realms`, `sessions`, `tools`, `providers`, and `configuration` (`docs/docs.json:33-40`), but no equivalent concept-layer pages for other major topics that appear prominently in guides and examples, such as:

- memory
- hooks
- skills
- comms
- mobs
- scheduling
- realtime

Because of that gap, the docs jump from high-level concepts straight into implementation/tutorial pages for several advanced systems. The structure is consistent for the first five concepts, but not for the rest of the product surface.

What looks good: the existing concept group itself is clean and coherent. The issue is that the conceptual layer stops too early relative to the rest of the platform.

## 3. Important architecture material exists on disk but is barely represented in site navigation

Navigation integrity is good in one important respect: all pages listed in `docs/docs.json` resolve to real files. I did not find broken nav entries.

However, the reverse is not true. There is a large body of architecture/design material under `docs/architecture/`, plus `docs/design/mobpack.md` and `docs/reference/comms-redesign-v6-hard-cut.md`, that is not represented in the left-nav at all. The currently unlisted set includes:

- `docs/architecture/RMAT.md`
- `docs/architecture/finite-ownership-ledger.md`
- `docs/architecture/formal-seam-closure.md`
- `docs/architecture/identity-generation-and-runtime-epoch.md`
- multiple additional proposals and ledgers under `docs/architecture/`
- `docs/design/mobpack.md`
- `docs/reference/comms-redesign-v6-hard-cut.md`

`docs/reference/architecture.mdx:100-120` does link to a subset of those files, but that makes the architecture corpus discoverable only after a reader already knows to open the architecture reference page. From a site-structure perspective, this means the docs have a hidden secondary information hierarchy that is stored in files but not expressed in navigation.

This is especially odd because the public nav already has a `Reference` tab and an `Architecture` group in `docs/docs.json:135-142`, so the site signals that architecture is a first-class category while surfacing only one index page plus a handful of adjacent references.

## 4. Category fit is inconsistent in the `Reference` tab

`docs/docs.json:124-145` mixes at least three different content types inside `Reference`:

- user-facing reference (`reference/api-reference`, `reference/builtin-tools`)
- maintainer/operational material (`reference/skills-governance-runbook`)
- architectural background (`reference/architecture`, `reference/design-philosophy`, `reference/session-contracts`)

The most visible mismatch is `guides/cd-and-distribution`, which appears inside `Reference > Architecture` (`docs/docs.json:135-142`) even though the file itself is a release rulebook for publishing binaries and packages (`docs/guides/cd-and-distribution.md:7-205`). That is a maintainer operations document, not architecture reference and not really a general user guide either.

This matters because readers use category labels as promises. When "Reference" includes runbooks and release process documents, the tab stops being a reliable place to find stable product behavior and API facts.

What looks good: the split between `SDKs`, `CLI & APIs`, and `Reference` is directionally strong. The problem is mostly with what has been placed inside `Reference`, not with the existence of the tab itself.

## 5. The landing pages describe multiple audiences, but the nav does not make those audiences explicit

`docs/introduction.mdx:6-31` positions Meerkat as a library/runtime for builders rather than an end-user agent product, and `docs/introduction.mdx:33-54` does a good job exposing the main surfaces: Rust, Python, TypeScript, CLI, REST, and JSON-RPC. `docs/quickstart.mdx:8-107` follows that same multi-surface pattern with install and first-run tabs.

The structural issue is that the navigation does not fully capitalize on that framing. There is no explicit "Choose your path" or equivalent audience split for:

- embedding Meerkat in Rust
- using an SDK from Python/TypeScript
- operating Meerkat via CLI
- integrating via REST/RPC/MCP
- building multi-agent systems

Instead, those audiences are spread across separate top-level tabs and examples/guides pages, with no single orientation page bridging the introduction to the right section for each reader. The result is not broken, but it feels like the nav was organized by repository ownership more than by user journey.

# Suggested follow-ups

- Simplify the onboarding sequence into one dominant path: `Introduction -> Quickstart -> Auth -> choose a surface or examples`. Remove the duplicate `/examples/gallery` cards in `docs/introduction.mdx` and decide whether `examples/gallery` or `examples/sessions` is the canonical next step.
- Reorder `Documentation > Guides` so foundational operational topics appear first and niche setup pages like `self-hosted-gemma4` move later.
- Add either new concept pages or overview/index pages for the major systems that currently jump straight from nav into advanced guides: memory, hooks, skills, comms, mobs, scheduling, and realtime.
- Decide whether the architecture corpus under `docs/architecture/` is meant to be public-navigation content. If yes, expose it through a dedicated architecture subsection or index page with child pages. If no, move or label it more explicitly as internal/maintainer material.
- Remove category mismatches in `Reference`, especially `guides/cd-and-distribution`, by creating a separate `Operations`, `Maintainers`, or `Release` grouping if those docs are intentionally public.
- Consider adding a single audience router page near the top of the site that maps readers to Rust, SDKs, CLI, APIs, or multi-agent docs based on what they are trying to build.
