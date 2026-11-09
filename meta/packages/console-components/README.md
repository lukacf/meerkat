# console-components

`console-components` owns the reusable React surfaces for the shared console UI.

## Workspace-stable exports

This package is currently private to the Meta console workspace. The exports
below are the curated internal surface used by local console code and proving
fixtures; they are not an npm-public API promise.

Components:

- `ConversationEmptyState`
- `ConsoleConversationPanel`
- `ConversationPane`
- `ConversationTranscript`
- `ConsoleActivityRail`
- `ConsoleComposer`
- `ConsoleDock`
- `BrowserDockTargetHost`
- `ConsolePendingStack`
- `PendingStack`
- `ConsoleSidebar`
- `ConnectionPicker`
- `TopologyPanel`
- `ConsoleWorkbench`

Hooks:

- `useConsoleDockController`

Helpers:

- `topologyEdgeKey` — normalizes an endpoint pair for host lookup maps

Types:

- `ConversationEmptyStateProps`
- `ConsoleConversationPanelPhase`
- `ConsoleConversationPanelProps`
- `ConversationPaneProps`
- `ConversationTranscriptProps`
- `ConsoleActivityRailProps`
- `ConsoleComposerProps`
- `ConsoleDockController`
- `ConsoleDockProps`
- `BrowserDockTargetHostProps`
- `ConsolePendingDropWhere`
- `ConsolePendingItem`
- `ConsolePendingStackProps`
- `PendingDropWhere`
- `PendingItem`
- `PendingStackProps`
- `ConsoleSidebarActionButtonScope`
- `ConsoleSidebarItemTrailingRenderArgs`
- `ConsoleSidebarProps`
- `ConsoleSidebarSectionContainerRenderArgs`
- `ConsoleSidebarSectionHeaderRenderArgs`
- `ConsoleTopologyNode`
- `ConsoleTopologyNodePresentation`
- `ConnectionPickerProps`
- `TopologyActionCapability`
- `TopologyAuthorityRevisionTransition`
- `TopologyBoundedAction`
- `TopologyEdgeAffordance`
- `TopologyEdgeRef`
- `TopologyEndpoint`
- `TopologyManagementState`
- `TopologyMutationIntent`
- `TopologyOperationReceipt`
- `TopologyPanelProps`
- `TopologyPanelView`
- `ConsoleWorkbenchProps`
- `IconRenderer`
- `UseConsoleDockControllerOptions`

## Shared stylesheet

```ts
import "@console-components/styles";
```

The shared stylesheet is organized by domain:

- `tokens.css`
- `themes.css`
- `conversation.css`
- `conversation-panel.css`
- `workbench.css`
- `dock.css`
- `sidebar.css`
- `activity.css`
- `composer.css`
- `pending-stack.css`
- `topology.css`

## Host token contract

Shared styles now speak only `--cc-*` tokens. Set them on a single host root or wrapper that contains the shared surfaces.

Theme activation is wrapper-driven too:

- Set `data-cc-theme="light"` on the wrapper to force light mode in a package-owned way.
- Set `data-cc-theme="dark"` on a nested wrapper to force a local dark island inside a light host.
- Hosts with their own theme system should mirror the resolved theme onto `data-cc-theme`.

The most useful overrides are:

- `--cc-window-scale`
- `--cc-content-width`
- `--cc-bubble-max-width`
- `--cc-composer-width`
- `--cc-workbench-sidebar-width`
- `--cc-workbench-activity-width`
- `--cc-sidebar-safe-top`
- `--cc-sidebar-pad-left`
- `--cc-sidebar-pad-right`
- `--cc-member-accent`
- `--cc-member-accent-soft`
- `--cc-member-surface`

## Integration model

- Shared components render normalized props only.
- Shared components do not import app stores, Electron APIs, or network code.
- `useConsoleDockController` owns dock layout, tabs, splits, focus, and target placement only.
- Hosts keep panel-local session state outside the shared hook.
- External hosts must not depend on the private `@console-components` package
  name until Meta makes an explicit public-package or SDK-subpath decision.

### Optional topology management

`TopologyPanel` stays passive unless the host supplies a trusted
`TopologyManagementState`. A mutation callback alone never enables editing.
When policy mode is `read_only`, the searchable Connections roster explains
pair-specific denied reasons without emitting intents. Mode `disabled` removes
the management surface entirely.

```tsx
<TopologyPanel
  nodes={nodes}
  agents={agents}
  activity={activity}
  management={{
    revision: snapshot.revision
      ?? topologyAuthorityRevisionToken(snapshot.authority_revisions),
    authorityRevisions: snapshot.authority_revisions,
    policy: snapshot.policy,
    affordances: snapshot.affordances,
    operations: snapshot.operations,
  }}
  view={view}
  onViewChange={setView}
  connectionSourceId={focusedEndpointId}
  onRequestMutation={applyTopologyIntent}
/>
```

`TopologyMutationIntent` carries `action`, canonical endpoint `edge`, the
snapshot `expectedRevision`, and a generic UI `origin`. The host plans/applies
the mutation and then replaces the controlled snapshot. Pending approval,
queued/running work, conflicts, partial results, degraded links, reconnect, and
retry states come from explicit affordances and operation receipts.

Authority-local and bilateral hosts intentionally keep different wire
adapters. The stock Meta console maps its local query revision to
`revision` and calls the authority-local `meta/topology/*` RPCs. A
same-process bilateral coordinator additionally maps the exact
`snapshot.authority_revisions` object to `authorityRevisions`; the shared
component copies it unchanged to
`TopologyMutationIntent.expectedAuthorityRevisions`. That host callback must
send the map as the coordinator's `expected_revisions` CAS input. It must not
collapse the map to `expectedRevision` or route a cross-authority intent
through the local RPC adapter. `topologyAuthorityRevisionToken` only supplies
the component's backward-compatible scalar view token when a bilateral
snapshot has no scalar revision; it is never the coordinator CAS input.

On the graph, authorized endpoint pairs support direct connect/disconnect
gestures. The Connections roster remains the primary scalable path for large
agent sets. Optional `ConsoleTopologyNode.presentation` metadata controls
captions, sections, scope labels, search terms, and accents; it never carries
runtime authority.

There is no built-in connect-all or disconnect-all action. A host may pass
`bulkActions`, but each action must declare a finite operation count and
ceiling, the topology policy must separately enable bulk mutation with a finite
`maxBatchSize`, and both capabilities must be actionable. The default is no
bulk UI.

## Minimal recipe

1. Import the shared stylesheet.
2. Build a host target union on top of `ConsoleDockTarget`.
3. Use `useConsoleDockController` with a host `createPanelState` callback.
4. Render:
   - `ConsoleSidebar`
   - `ConsoleDock`
   - `ConsoleActivityRail`
   - `ConsoleComposer`
   - `ConsolePendingStack`
   - `TopologyPanel`
   - `ConsoleWorkbench`
5. Resolve each dock target into timeline entries plus host callbacks and render
   `ConsoleConversationPanel`.
