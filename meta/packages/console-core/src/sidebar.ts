import { normalizeSidebarWatchFields, type SidebarWatchFields } from "./control-plane";

export type ConsoleSidebarMetaTone = "default" | "muted" | "accent" | "positive" | "negative" | "warning";
export type ConsoleSidebarBlockKind = "action_strip" | "list";

export interface ConsoleSidebarMeta {
  id?: string;
  label: string;
  tone?: ConsoleSidebarMetaTone;
  iconName?: string | null;
}

export interface ConsoleSidebarAction {
  id: string;
  label: string;
  iconName?: string | null;
  active?: boolean;
  disabled?: boolean;
}

export interface ConsoleSidebarItem extends SidebarWatchFields {
  id: string;
  title: string;
  subtitle?: string | null;
  meta?: ConsoleSidebarMeta[];
  selected?: boolean;
  unread?: boolean;
  pinned?: boolean;
  disabled?: boolean;
  iconName?: string | null;
  badgeIconName?: string | null;
  badgeLabel?: string | null;
  actions?: ConsoleSidebarAction[];
}

export interface ConsoleSidebarSection {
  id: string;
  title: string;
  subtitle?: string | null;
  iconName?: string | null;
  meta?: ConsoleSidebarMeta[];
  selected?: boolean;
  actions?: ConsoleSidebarAction[];
  items: ConsoleSidebarItem[];
}

export interface ConsoleSidebarBlock {
  id: string;
  kind: ConsoleSidebarBlockKind;
  title?: string | null;
  meta?: ConsoleSidebarMeta[];
  actions?: ConsoleSidebarAction[];
  sections?: ConsoleSidebarSection[];
}

export interface ConsoleSidebarViewState {
  blocks: ConsoleSidebarBlock[];
}

function normalizeMeta(meta: ConsoleSidebarMeta[] | null | undefined): ConsoleSidebarMeta[] {
  return (meta || []).filter((item) => Boolean(item?.label));
}

function normalizeActions(actions: ConsoleSidebarAction[] | null | undefined): ConsoleSidebarAction[] {
  return (actions || []).filter((action) => Boolean(action?.id && action?.label));
}

function normalizeItems(items: ConsoleSidebarItem[] | null | undefined): ConsoleSidebarItem[] {
  return (items || []).filter((item) => Boolean(item?.id && item?.title)).map((item) => ({
    ...item,
    ...normalizeSidebarWatchFields(item),
    meta: normalizeMeta(item.meta),
    actions: normalizeActions(item.actions),
  }));
}

function normalizeSections(sections: ConsoleSidebarSection[] | null | undefined): ConsoleSidebarSection[] {
  return (sections || [])
    .filter((section) => Boolean(section?.id && typeof section?.title === "string"))
    .map((section) => ({
      ...section,
      meta: normalizeMeta(section.meta),
      actions: normalizeActions(section.actions),
      items: normalizeItems(section.items),
    }))
    .filter((section) => {
      if (section.items.length > 0) {
        return true;
      }
      return Boolean(
        section.title
        || section.subtitle
        || section.iconName
        || section.actions.length
        || section.meta.length,
      );
    });
}

export function normalizeConsoleSidebarViewState(
  viewState: ConsoleSidebarViewState | null | undefined,
): ConsoleSidebarViewState {
  const blocks = (viewState?.blocks || [])
    .filter((block) => Boolean(block?.id && block?.kind))
    .map((block) => ({
      ...block,
      meta: normalizeMeta(block.meta),
      actions: normalizeActions(block.actions),
      sections: normalizeSections(block.sections),
    }))
    .filter((block) => {
      if (block.kind === "action_strip") {
        return block.actions.length > 0;
      }

      if (block.sections.length > 0) {
        return true;
      }

      return Boolean(block.title || block.meta.length || block.actions.length);
    });

  return { blocks };
}
