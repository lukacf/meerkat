// The goal header (a goal frame's top strip, which is the goal's node):
// flag, sentence-case title, progress ring, meta, and a right-aligned
// trailer ("Coordinator · Leading the work"). When the strip is too narrow
// the title truncates first, then the meta goes, then the trailer.

import type { TextMeasure } from "../layout/geometry";
import { fitText } from "./labels";
import type { GoalHeaderLayout } from "./svg-layers";
import type { VizProgress } from "./types";

export const GOAL_HEADER = {
  titleX: 32,
  padRight: 14,
  /** Title end to the ring's centre, and the ring's centre to the meta. */
  ringLead: 16,
  ringTail: 13,
  metaLead: 10,
  trailingGap: 20,
  avatar: 16,
  avatarGap: 5,
  minTitle: 72,
} as const;

export interface GoalHeaderMeasures {
  title: TextMeasure;
  meta: TextMeasure;
}

export function layoutGoalHeader(
  width: number,
  input: { title: string; meta?: string; progress?: VizProgress; trailing?: { text: string; avatar?: string }; collapsed: boolean },
  measures: GoalHeaderMeasures,
): GoalHeaderLayout {
  const room = width - GOAL_HEADER.titleX - GOAL_HEADER.padRight;
  const trailingText = input.trailing?.text ?? "";
  const trailingAvatar = input.trailing?.avatar;
  const trailingWidth = trailingText
    ? measures.meta(trailingText) + (trailingAvatar ? GOAL_HEADER.avatar + GOAL_HEADER.avatarGap : 0) + GOAL_HEADER.trailingGap
    : 0;
  const ringWidth = input.progress ? GOAL_HEADER.ringLead + GOAL_HEADER.ringTail : 0;
  const metaWidth = input.meta ? measures.meta(input.meta) + (input.progress ? 0 : GOAL_HEADER.metaLead) : 0;
  const titleWidth = measures.title(input.title);
  let showMeta = Boolean(input.meta);
  let showTrailing = Boolean(trailingText);
  const titleRoom = () => room - (showTrailing ? trailingWidth : 0) - ringWidth - (showMeta ? metaWidth : 0);
  if (titleWidth > titleRoom() && titleRoom() < GOAL_HEADER.minTitle && showMeta) showMeta = false;
  if (titleWidth > titleRoom() && titleRoom() < GOAL_HEADER.minTitle && showTrailing) showTrailing = false;
  const title = fitText(input.title, Math.max(24, titleRoom()), measures.title);
  const titleEnd = GOAL_HEADER.titleX + measures.title(title);
  const layout: GoalHeaderLayout = { title, titleX: GOAL_HEADER.titleX, collapsed: input.collapsed };
  let cursor = titleEnd;
  if (input.progress) {
    layout.ring = { cx: cursor + GOAL_HEADER.ringLead, progress: input.progress };
    cursor = layout.ring.cx + GOAL_HEADER.ringTail;
  } else {
    cursor += GOAL_HEADER.metaLead;
  }
  if (showMeta && input.meta) layout.meta = { x: cursor, text: input.meta };
  if (showTrailing && trailingText) {
    const right = width - GOAL_HEADER.padRight + 2;
    const textWidth = measures.meta(trailingText);
    layout.trailing = {
      right,
      text: trailingText,
      avatar: trailingAvatar,
      avatarCx: trailingAvatar ? right - textWidth - GOAL_HEADER.avatarGap - GOAL_HEADER.avatar / 2 : undefined,
    };
  }
  return layout;
}
