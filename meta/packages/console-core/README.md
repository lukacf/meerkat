# console-core

`console-core` owns the normalized, UI-facing model for the shared console surfaces.

## Workspace-stable domains

- `conversation`
- `composer`
- `dock`
- `sidebar`
- `activity`
- `adapters`
- `contract`
- `headless`
- `network`
- `rich-content`
- `runtime-types`

## What stays here

- conversation timeline-entry, grouping, and view-state helpers
- composer view-state types
- dock state, presets, reducer-style operations, and view-state builders
- placement-only browser dock targets
- sidebar schema normalization
- activity-rail view-model types
- Meta console target/adapters, contract constants, headless controller, network client, and runtime frame types
- rich-content parsing and structured block helpers

## What stays app-side

- Zustand or other app stores
- Electron/Desktop bridge code
- Electron/Desktop bridge code and app-owned network transport implementations
- panel-local session state such as drafts, provider/model selection, permissions, branches, and execution mode
- host-specific adapters from app contracts into shared models

## Internal consumer recipe

1. Define a host target union that extends `ConsoleDockTarget`.
2. Use `createConsoleDockState`, `openConsoleDockTarget`, `splitConsoleDockPanel`, and related helpers directly, or pair them with `useConsoleDockController` from `console-components`.
3. Resolve host transcript/sidebar/activity data into:
   - `ConversationViewState`
   - `ConsoleSidebarViewState`
   - `ConsoleActivityRailViewState`
4. Use `createMetaConsoleController` with a host transport or HTTP transport for Meta console command/timeline flows.
5. Keep any panel-local state in a host-owned map keyed by `panelId`.

## Workspace entrypoint

Import from `@console-core` inside this repository's console workspace. The root
barrel is curated and explicit, but this package is currently private and is
not an npm-public API promise.

External hosts must not depend on this package name until Meta makes an
explicit public-package or SDK-subpath decision. Compatibility aliases should
not be treated as stable package API.

The legacy Node runtime helpers remain available at `@console-core/runtime` for existing consumers that still rely on the package root helper bundle.
