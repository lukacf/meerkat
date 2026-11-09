# flow-editor-core

`@flow-editor-core` is the Meta Flow Editor's headless controller plane:
the pure projection and transition logic extracted module by module from the
legacy `flow-editor/src/controller.js` monolith. Framework-free — no React,
no DOM access, no `window` (the shell owns the global assignment).

## Module map

- `shared/constants.ts` — RPC method table, schema command keys, layout constants
- `shared/normalize.ts` — generic normalizers (`numberOrNull`, `escapeHtml`, …)
- `rpc/client.ts` — JSON-RPC transport (`callRpc`), endpoint/method config; `controllerConfig`/`requestId` are module-level singletons; `callRpc` calls the bare global `fetch` at call time so harnesses can stub it
- `rpc/doc-ops.ts` — authoring operation runner and document RPC helpers
- `views/view-config.ts` — `*ViewFromSchema` / `*ViewForState` projections
- `domain/tool-skill-access.ts` — member/step tool and skill access state and patches
- `schema/field-edit.ts` — schema field editing and error-view state
- `contract/options.ts` + `flow/launch-modes.ts` — contract option builders and launch modes (runtime-only import cycle, co-moved)
- `drafts/mob-settings.ts` — mob/deploy settings drafts
- `flow/reconcile.ts` + `members/patches.ts` — edit reconciliation and member patch semantics (runtime-only import cycle, co-moved)
- `flow/step-tree.ts` — flow step-tree patches and validation
- `studio/state.ts` — studio document transitions, undo/redo history
- `editors/basic-editor.ts`, `editors/graph-editor.ts` — Build/Flow editor control states (co-moved cycle)
- `editors/agent-editor.ts` — agent editor control states
- `shell/outcomes.ts` — top-rail/overlay transitions, validate/export/deploy outcomes, error-outcome family
- `source/view.ts` — source documents, drawers, TOML highlighting
- `registry/flow-registry.ts` — flow registry rows and bootstrap state
- `document/build-projection.ts` — authoring document build and projection
- `catalogs/hydration.ts` — Meta catalog projections and mobpack document hydration
- `controller-facade.ts` — `createMetaFlowController`

## How the shell consumes it

`flow-editor/build.cjs` resolves the esbuild alias `@flow-editor-core` to
`src/index.ts` and bundles the package into the single `flow-editor.js` IIFE
together with the shell and `@flow-editor-components`. The shell entry
(`flow-editor/src/app.tsx`) constructs the facade once at module scope and
assigns `window.MetaFlowController` as a deliberate back-compat surface.
The projection test lane (`node build.cjs --test-bundle`) instead bundles the
package index as a `window.MetaFlowCore` IIFE plus a bootstrap shim, so the
Node test suite loads the same code the browser runs.

## The facade contract

`createMetaFlowController({ includeTestExports })` assembles the exact
`window.MetaFlowController` key set the views consume stringly, with
additional test-only exports behind `includeTestExports`. Both sets are
recorded in the checked-in manifest. Key-set parity with
`flow-editor/test/controller-export-manifest.json` is enforced by
`controller-export-keys.test.cjs` on every change.

## Typing: migration window and ratchet

The modules moved byte-verbatim as plain JS into `.ts`, so every module
except the barrel and `shared/` carries a file-level `// @ts-nocheck` whose
header comment documents the suppressed error class (mostly TS2339 from
`= {}` parameter defaults on otherwise-correct JS). The ratchet plan: remove
the directive per module by typing parameter shapes — never by rewriting
moved bodies — then tighten `tsconfig.json` toward `strict: true`. Until
then, behavior is guarded by the export-keys parity test and the projection
suite (which load the bundle and exercise the functions), and the package
boundary is guarded by `flow-editor/test/package-boundaries.test.cjs`
(window-free outside the facade/bridge files, no React, no DOM document
access, no `require`/Node imports).

## Workspace-private

This package is private to the Meta repository. `@flow-editor-core` is a
build-time alias, not an npm-published package name; external consumers must
not depend on it until Meta makes an explicit public-package decision.
