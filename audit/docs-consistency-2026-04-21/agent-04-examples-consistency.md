# Agent 04 Findings

## Scope reviewed

- `docs/examples/*`
- Focus areas: section-level consistency, beginner-to-advanced progression, category design, duplication, cross-linking, and page/order structure
- Out of scope: implementation correctness outside what was needed to assess consistency

## Files reviewed

- `docs/examples/gallery.mdx`
- `docs/examples/sessions.mdx`
- `docs/examples/tools.mdx`
- `docs/examples/skills.mdx`
- `docs/examples/structured-output.mdx`
- `docs/examples/memory.mdx`
- `docs/examples/hooks.mdx`
- `docs/examples/comms.mdx`
- `docs/examples/mobs.mdx`
- `docs/examples/mobpack.mdx`
- `docs/examples/wasm.mdx`

## Findings

### 1. The examples section does not have a consistent learning-path spine

The section currently mixes tutorial-like pages, reference-like pages, and a repo gallery, but only a few pages tell the reader what to do next. `docs/examples/gallery.mdx:7` and `docs/examples/gallery.mdx:23` explicitly tell new users to start with the hello-world examples and the Sessions page, which is good. After that, most pages stop at a single top-of-page guide link such as `docs/examples/comms.mdx:7`, `docs/examples/memory.mdx:7`, `docs/examples/skills.mdx:7`, and `docs/examples/mobpack.mdx:7`, without “next step” links or related-example links at the end.

`docs/examples/mobs.mdx:380` is the main exception because it includes a `See also` section, which makes it feel more navigable than the rest of the section. The inconsistency makes the examples area feel like a set of isolated islands instead of a sequenced path from “first session” to “advanced orchestration/deployment”.

### 2. `gallery.mdx` has a useful top-level split, but the final category is too broad to teach the right mental model

`docs/examples/gallery.mdx` is one of the stronger pages in the set because it establishes an entry point and groups examples by intent. The first two sections, `Starter examples` (`docs/examples/gallery.mdx:9`) and `Multi-agent systems` (`docs/examples/gallery.mdx:25`), are clear.

The `Portable deployment` bucket at `docs/examples/gallery.mdx:56` is much less coherent. It currently includes:

- `.mobpack` packaging and release-triage (`docs/examples/gallery.mdx:59`)
- browser/WASM war-room and dashboard demos (`docs/examples/gallery.mdx:62`, `docs/examples/gallery.mdx:65`, `docs/examples/gallery.mdx:68`, `docs/examples/gallery.mdx:71`)
- an MCP server (`docs/examples/gallery.mdx:77`)
- a TUI device-management demo (`docs/examples/gallery.mdx:80`)

Those are different axes: packaging, browser deployment, integrations, and full applications. As written, the category works more like “everything advanced and shippable” than a real deployment grouping, so it weakens the gallery’s value as a map for learners deciding what kind of example they need.

### 3. Surface-specific pages are mixed into cross-surface examples without enough framing

Most example pages use a repeated multi-surface pattern with `CLI`, `JSON-RPC`, `REST`, `MCP`, `Python`, `TypeScript`, and sometimes `Rust` tabs. That establishes an expectation that “Examples” pages are cross-surface references. Two pages break that pattern in a way that is understandable, but not well signposted:

- `docs/examples/mobpack.mdx` is effectively CLI-only (`docs/examples/mobpack.mdx:9-74`)
- `docs/examples/wasm.mdx` is effectively JavaScript/WASM-only (`docs/examples/wasm.mdx:29-148`)

Neither page clearly says up front that it is intentionally surface-specific, so the reader has to infer why these pages are structured differently from the rest of the section. This is especially noticeable because the surrounding pages teach the same product through heavily parallel tabs. A short note near the top of each page would make the structure feel intentional rather than inconsistent.

### 4. `tools.mdx` starts with a Rust-only advanced extension point before the more common user entry points

`docs/examples/tools.mdx` opens with `## Custom tools (Rust SDK)` at `docs/examples/tools.mdx:9`, then only later moves to `## Enable built-in tools` at `docs/examples/tools.mdx:46`, followed by MCP registration and live controls.

For an examples section that is supposed to serve both new and advanced users, that order is backwards. The likely first questions are “how do I turn tools on?”, “how do I add MCP?”, and “how do I limit tool access?”. Opening with a Rust trait implementation makes the page read as SDK-internals-first even though the rest of the page is more broadly useful. This is a structure/order problem, not a correctness problem.

### 5. `skills.mdx` duplicates the session-create vs preload distinction instead of building it progressively

`docs/examples/skills.mdx` introduces `Inject skills on session create` at `docs/examples/skills.mdx:143` and explicitly contrasts CLI `--skill` with typed `skill_refs` at `docs/examples/skills.mdx:145`. It then introduces a separate `Preload skills` section at `docs/examples/skills.mdx:277`, followed by `Per-turn skill injection` at `docs/examples/skills.mdx:348`.

The result is that the page teaches three overlapping concepts in a slightly circular order:

- session-create injection via `skill_refs`
- preload behavior via CLI `--skill` and `preload_skills`
- per-turn injection via `skill_refs`

Because CLI `--skill` appears both before and after the `Preload skills` section (`docs/examples/skills.mdx:149-153`, `docs/examples/skills.mdx:283-285`, `docs/examples/skills.mdx:353-357`), the page feels duplicative rather than progressive. A cleaner learner flow would be “discover -> inspect -> preload for whole session -> inject per turn”, with one explicit comparison table for `--skill`, `preload_skills`, and `skill_refs`.

### 6. Related advanced pages are not cross-linked strongly enough, especially `mobpack`, `wasm`, and `mobs`

There is an implicit progression across several advanced pages:

- define and run a mob in `docs/examples/mobs.mdx`
- package/deploy it in `docs/examples/mobpack.mdx`
- optionally ship it to the browser in `docs/examples/wasm.mdx`

That relationship is only partially visible. `docs/examples/mobs.mdx:384` links to Mobpack, which is good, but `docs/examples/mobpack.mdx` does not link onward to WASM/browser deployment, and `docs/examples/wasm.mdx` does not link back to Mobpack or Mobs even though its first example literally starts from a `.mobpack` artifact at `docs/examples/wasm.mdx:13-17`. This weakens the advanced learning path because the reader has to discover the next page through navigation rather than through the content itself.

### 7. The hooks page contains an internal consistency break around CLI support, which makes its structure harder to trust

The first three major examples on `docs/examples/hooks.mdx` repeat that run-scoped hook override flags are not exposed on the normal CLI surface (`docs/examples/hooks.mdx:15`, `docs/examples/hooks.mdx:139`, `docs/examples/hooks.mdx:290`). Later, `Disable a hook` shows a CLI example using `--hooks-override-json` on `rkat run` (`docs/examples/hooks.mdx:430-435`).

Even if there is a subtle distinction intended here, the page currently reads as self-contradictory. From a documentation consistency standpoint, that is a structure/framing problem: the page does not establish a stable mental model for what the CLI can and cannot do with hook overrides, so later examples feel like exceptions instead of part of a progression.

### 8. Some pages already have strong internal progression and should be treated as the baseline pattern

A few pages are notably stronger and more consistent:

- `docs/examples/sessions.mdx` progresses well from create -> multi-turn -> resume/list/read -> archive/interrupt -> streaming -> model/config/realm. It works as both a first stop and a durable reference page.
- `docs/examples/comms.mdx` has a clean operational sequence: keep-alive session -> send command -> discover peers -> inject external event (`docs/examples/comms.mdx:9`, `docs/examples/comms.mdx:63`, `docs/examples/comms.mdx:135`, `docs/examples/comms.mdx:182`).
- `docs/examples/memory.mdx` also reads coherently from enablement -> search -> compaction -> budgets -> retry policy (`docs/examples/memory.mdx:13`, `docs/examples/memory.mdx:90`, `docs/examples/memory.mdx:124`, `docs/examples/memory.mdx:169`, `docs/examples/memory.mdx:274`).

These pages are good reference points for how the rest of the examples section could balance breadth with progression.

## Suggested follow-ups

- Add a lightweight “start here / next step” pattern to every examples page, ideally with one prerequisite link near the top and one “continue with” link at the bottom.
- Rework `docs/examples/gallery.mdx` so the last bucket is split into clearer categories, for example: `Deployment & packaging`, `Browser/WASM apps`, and `Integrations & full applications`.
- Reorder `docs/examples/tools.mdx` so broadly useful entry points come first: built-in tools, MCP registration, live controls, tool scoping, then Rust custom tools last.
- Simplify `docs/examples/skills.mdx` into an explicit progression and remove repeated CLI `--skill` explanations from multiple sections.
- Add reciprocal cross-links among `docs/examples/mobs.mdx`, `docs/examples/mobpack.mdx`, and `docs/examples/wasm.mdx` to make the advanced path discoverable from within the content.
- Normalize how surface-specific pages are introduced by adding a short note at the top of `docs/examples/mobpack.mdx` and `docs/examples/wasm.mdx` explaining why they are intentionally not multi-surface-tab pages.
- Clarify the CLI hook-override story on `docs/examples/hooks.mdx` so the page does not alternate between “not exposed on CLI” and “use CLI override JSON” without an explanation.
