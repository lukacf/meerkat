import "@console-components/styles";

import { render, screen } from "@testing-library/react";

import {
  groupConversationTimelineEntries,
  normalizeConsoleSidebarViewState,
  type ConversationEmptyStateSpec,
  type ConversationTimelineEntry,
  type ConversationViewState,
  type ConsoleActivityRailViewState,
  type ConsoleSidebarViewState,
  type ConsoleDockTarget,
} from "@console-core";
import {
  ConsoleActivityRail,
  ConsoleDock,
  ConsoleSidebar,
  ConsoleWorkbench,
  ConversationEmptyState,
  ConversationPane,
  ConversationTranscript,
  useConsoleDockController,
  type ConsoleActivityRailProps,
  type ConsoleDockController,
  type ConsoleDockProps,
  type ConsoleWorkbenchProps,
  type UseConsoleDockControllerOptions,
} from "@console-components";

function Icon({ name, className }: { name: string; className?: string }) {
  return (
    <svg className={className} data-icon={name}>
      <title>{name}</title>
    </svg>
  );
}

function buildConversationViewState(): ConversationViewState {
  const entries: ConversationTimelineEntry[] = [
    {
      id: "user-1",
      kind: "message",
      variant: "plain",
      identity: { id: "user", label: "You", role: "user", presentation: "user" },
      text: "Open the shared console in Meta.",
    },
    {
      id: "assistant-1",
      kind: "message",
      variant: "plain",
      identity: { id: "assistant", label: "Assistant", role: "assistant" },
      text: "The dock, sidebar, and transcript are ready to vendor.",
    },
  ];

  return {
    conversationId: "smoke-conversation",
    entries,
    groups: groupConversationTimelineEntries(entries),
    turnDiff: null,
    emptyState: null,
  };
}

function buildStandaloneTranscriptViewState(): ConversationViewState {
  const entries: ConversationTimelineEntry[] = [
    {
      id: "user-2",
      kind: "message",
      variant: "plain",
      identity: { id: "user", label: "You", role: "user", presentation: "user" },
      text: "Show the transcript surface by itself.",
    },
    {
      id: "assistant-2",
      kind: "message",
      variant: "plain",
      identity: { id: "assistant", label: "Assistant", role: "assistant" },
      text: "Transcript-only mounting works through the root exports too.",
    },
  ];

  return {
    conversationId: "standalone-transcript",
    entries,
    groups: groupConversationTimelineEntries(entries),
    turnDiff: null,
    emptyState: null,
  };
}

function buildEmptyState(): ConversationEmptyStateSpec {
  return {
    title: "Ready for the next target",
    subtitle: "Use the launcher to open a thread, agent, or helper into the focused panel.",
    projectLabel: "workspace",
    iconName: "i-box",
    suggestions: [
      {
        id: "prototype-vendor",
        label: "Prototype the Meta vendor pass.",
        value: "Prototype the Meta vendor pass.",
        iconName: "i-pencil",
      },
    ],
  };
}

function buildSidebarViewState(): ConsoleSidebarViewState {
  return normalizeConsoleSidebarViewState({
    blocks: [
      {
        id: "primary",
        kind: "action_strip",
        actions: [{ id: "new", label: "New thread", iconName: "i-new-thread" }],
      },
      {
        id: "threads",
        kind: "list",
        title: "Threads",
        sections: [{
          id: "workspace",
          title: "workspace",
          items: [{ id: "thread-1", title: "Dock smoke test", selected: true }],
        }],
      },
    ],
  });
}

function buildActivityViewState(): ConsoleActivityRailViewState {
  return {
    ingress: {
      label: "Meerkat",
      meta: "Ingress",
    },
    panels: [{
      id: "watch",
      kind: "feed",
      title: "Watch",
      slots: [{
        id: "slot-1",
        focusId: "thread-1",
        title: "Dock smoke test",
        eyebrow: "Slot 1",
        meta: "1m",
        subtitle: "Shared components are mounted through the root exports.",
        emptyLabel: "No activity yet.",
      }],
    }],
    footerActionLabel: "Mobs",
  };
}

function DockHarness() {
  type TestTarget = ConsoleDockTarget & {
    kind: "thread";
    threadId: string;
  };

  const controllerOptions: UseConsoleDockControllerOptions<TestTarget> = {
    initialTarget: {
      id: "thread-1",
      kind: "thread",
      title: "Dock smoke test",
      subtitle: "workspace",
      threadId: "thread-1",
    },
    createPanelState: ({ target }) => ({
      id: "",
      target,
      mode: "console",
    }),
  };

  const controller: ConsoleDockController<TestTarget> = useConsoleDockController<TestTarget>(controllerOptions);
  const dockProps: ConsoleDockProps<TestTarget> = {
    Icon,
    viewState: controller.viewState,
    onCreateTab: controller.createTab,
    onClosePanel: (panel) => controller.closePanel(panel.id),
    onCloseTab: (tab) => controller.closeTab(tab.id),
    onFocusPanel: (panel) => controller.focusPanel(panel.id),
    onResizeSplit: controller.resizeSplit,
    onSelectTab: (tab) => controller.selectTab(tab.id),
    onSplitPanel: (panel, direction) => controller.splitPanel(panel.id, direction),
    renderPanelBody: (panel) => <ConversationPane Icon={Icon} viewState={buildConversationViewState()} footer={<div>{panel.title}</div>} />,
  };

  return <ConsoleDock {...dockProps} />;
}

describe("shared console public API", () => {
  test("renders a fake host composition from root exports only", () => {
    const activityProps: ConsoleActivityRailProps = {
      Icon,
      viewState: buildActivityViewState(),
      onCollapse: () => {},
      onTogglePicker: () => {},
      renderSlotPreview: () => <div>Preview</div>,
    };
    const workbenchProps: ConsoleWorkbenchProps = {
      launcher: <ConsoleSidebar Icon={Icon} viewState={buildSidebarViewState()} />,
      main: <DockHarness />,
      activityRail: <ConsoleActivityRail {...activityProps} />,
    };

    render(
      <>
        <ConsoleWorkbench {...workbenchProps} />
        <ConsoleTranscriptMount />
        <ConversationEmptyState Icon={Icon} state={buildEmptyState()} />
      </>,
    );

    expect(screen.getAllByText("Dock smoke test").length).toBeGreaterThan(0);
    expect(screen.getByText("Threads")).toBeInTheDocument();
    expect(screen.getByText("Watch")).toBeInTheDocument();
    expect(screen.getByText("The dock, sidebar, and transcript are ready to vendor.")).toBeInTheDocument();
    expect(screen.getByText("Transcript-only mounting works through the root exports too.")).toBeInTheDocument();
    expect(screen.getByText("Ready for the next target")).toBeInTheDocument();
    expect(screen.getByText("Prototype the Meta vendor pass.")).toBeInTheDocument();
  });
});

function ConsoleTranscriptMount() {
  return <ConversationTranscript Icon={Icon} viewState={buildStandaloneTranscriptViewState()} />;
}
