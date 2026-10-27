export interface ConsoleActivityTone {
  variables?: Record<string, string> | null;
}

export interface ConsoleActivityIngress {
  label: string;
  meta: string;
  active?: boolean;
  prominent?: boolean;
}

export interface ConsoleActivityAction {
  id: string;
  label: string;
  active?: boolean;
}

export interface ConsoleActivityItem {
  id: string;
  focusId?: string | null;
  pinId?: string | null;
  title: string;
  subtitle?: string | null;
  meta?: string | null;
  tooltip?: string | null;
  selected?: boolean;
  pinned?: boolean;
  tone?: ConsoleActivityTone | null;
}

export interface ConsoleActivityRosterGroup {
  id: string;
  title: string;
  meta?: string | null;
  inactive?: boolean;
  items: ConsoleActivityItem[];
}

export interface ConsoleActivityRosterPanel {
  id: string;
  kind: "roster";
  title: string;
  meta?: string | null;
  actions?: ConsoleActivityAction[];
  groups: ConsoleActivityRosterGroup[];
  emptyText?: string | null;
  /**
   * When false, items do not emit the `data-workspace-member-key` attribute.
   * Use for panels whose items *reference* a member (via focusId) rather than
   * *represent* one — e.g. a jobs list — so member-key selectors stay unique.
   * Defaults to true.
   */
  itemsRepresentMembers?: boolean;
  /**
   * When false, the panel does not render a Hide/remove action (for synthetic
   * panels that are not part of the user's watch-panel preferences and cannot
   * actually be removed). Defaults to true.
   */
  removable?: boolean;
}

export interface ConsoleActivityPulseItem extends ConsoleActivityItem {
  line: string;
}

export interface ConsoleActivityPulsePanel {
  id: string;
  kind: "pulse";
  title: string;
  meta?: string | null;
  actions?: ConsoleActivityAction[];
  items: ConsoleActivityPulseItem[];
  emptyText: string;
}

export interface ConsoleActivityFeedSlot {
  id: string;
  focusId?: string | null;
  pinId?: string | null;
  eyebrow: string;
  title: string;
  meta: string;
  subtitle: string;
  emptyLabel: string;
  selected?: boolean;
  pinned?: boolean;
  tone?: ConsoleActivityTone | null;
}

export interface ConsoleActivityFeedPanel {
  id: string;
  kind: "feed";
  title: string;
  actions?: ConsoleActivityAction[];
  slots: ConsoleActivityFeedSlot[];
}

export type ConsoleActivityPanel =
  | ConsoleActivityRosterPanel
  | ConsoleActivityPulsePanel
  | ConsoleActivityFeedPanel;

export interface ConsoleActivityRailEmptyState {
  title: string;
  description: string;
  actionLabel: string;
}

export interface ConsoleActivityRailViewState {
  ingress?: ConsoleActivityIngress | null;
  panels: ConsoleActivityPanel[];
  emptyState?: ConsoleActivityRailEmptyState | null;
  footerActionLabel?: string | null;
}
