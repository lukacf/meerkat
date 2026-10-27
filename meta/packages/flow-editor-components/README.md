# flow-editor-components

`@flow-editor-components` is the Meta Flow Editor's React view layer,
moved verbatim from the legacy `flow-editor/src/*.jsx` files. Components
render props and call the controller facade; they own no networking and no
document state.

## Module map

- `agents/agents.tsx` — `AgentsView` (agent list, detail, access, schema editing)
- `builder/builder.tsx` — `BuilderView` (Build mode: step lanes, step inspector, pickers)
- `graph/graph.tsx` — `GraphEditor`, `useStudioState` (Flow canvas, undo/redo wiring)
- `inspector/inspector.tsx` — `Inspector`, `AddNodeMenu`
- `overlays/overlays.tsx` — `DeployPlanTrace`, `ValidateSheet`, `SourceDrawer`, `InlineSourceEditor`
- `tweaks/tweaks-panel.tsx` — `TweaksPanel`, `useTweaks`, and the `Tweak*` control family. The shell no longer mounts the floating `TweaksPanel` (its settings render as a full-page SETTINGS section using the `Tweak*` controls directly); the panel stays exported for embedders that want the draggable overlay + edit-mode host protocol.
- `styles/` — reserved; the CSS stays at `flow-editor/src/{tokens.css,styles.css}` because the designer-handoff visual contract pins those paths

`src/index.ts` is the curated barrel; the exports are a workspace-internal
surface, not an npm-public API promise.

## How the shell consumes it

`flow-editor/build.cjs` resolves the esbuild alias `@flow-editor-components`
to `src/index.ts`; the shell entry (`flow-editor/src/app.tsx`) imports the
views and esbuild bundles everything into the single `flow-editor.js` IIFE.
React is **not** bundled: the package uses the classic JSX transform
(`React.createElement`), and the emitted free `React` identifier resolves to
the window global that `react-globals.js` provides (ambient declaration in
`src/globals.d.ts`).

## The facade contract

Components call `window.MetaFlowController.*` at render/handler time — not
import time. That global is the runtime contract between the views and
`@flow-editor-core`'s `createMetaFlowController` (the exact ordinary and
test-only export sets in `flow-editor/test/controller-export-manifest.json`
are pinned by `controller-export-keys.test.cjs`); the shell assigns it once
at module scope. Views must not import the RPC client or call `fetch` themselves —
enforced by `flow-editor/test/package-boundaries.test.cjs`.

## DOM output is contractual

Class names, element structure, and `data-*` attributes are pinned by the
browser smokes, the designer-handoff visual contract, and the live
verification scripts. Treat renders as byte-stable: behavior-preserving
refactors still need those gates green.

## Typing: migration window and ratchet

The components moved verbatim and currently typecheck without any
`// @ts-nocheck` under the package's loose `tsconfig.json`
(`strict: false`, `noImplicitAny: false`). The ratchet plan is the same as
`@flow-editor-core`'s: tighten compiler options module by module without
rewriting moved render bodies.

## Workspace-private

This package is private to the Meta repository. `@flow-editor-components`
is a build-time alias, not an npm-published package name; external consumers
must not depend on it until Meta makes an explicit public-package decision.

## Window-global surface change (extraction note)

The legacy concat build leaked every view component onto `window`
(`window.GraphEditor`, `window.Inspector`, the `Tweak*` family, …) as its
linking mechanism. The bundled shell no longer exposes those globals — the
only intentional window surface of the shipped artifact is
`window.MetaFlowController` (and `window.META_BOOT`). Page scripts or
embedders that touched the old component globals must import this package
instead.
