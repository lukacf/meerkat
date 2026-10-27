import {
  conversationEntryText,
  conversationIdentityPresentation,
  type ConversationTurnDiff,
  type ConversationTimelineGroup,
} from "@console-core";

export interface ConversationTranscriptTurn {
  id: string;
  groups: ConversationTimelineGroup[];
}

export interface ConversationTurnPreviewFile {
  name: string;
  iconName: string | null;
}

export interface ConversationTurnPreview {
  title: string;
  body: string;
  files: ConversationTurnPreviewFile[];
  hiddenFileCount: number;
}

function firstEntryText(group: ConversationTimelineGroup): string {
  for (const entry of group.entries) {
    const text = conversationEntryText(entry).trim();
    if (text) {
      return text;
    }
  }
  return "";
}

export function groupConversationTranscriptTurns(
  groups: ConversationTimelineGroup[],
): ConversationTranscriptTurn[] {
  const turns: ConversationTranscriptTurn[] = [];

  for (const group of groups) {
    const presentation = conversationIdentityPresentation(group.identity);
    const startsTurn = presentation === "user";
    const current = turns.at(-1);

    if (!current || startsTurn) {
      turns.push({
        id: `turn-${group.id}`,
        groups: [group],
      });
      continue;
    }

    current.groups.push(group);
  }

  return turns;
}

export function conversationTurnPreview(
  turn: ConversationTranscriptTurn,
  turnDiff: ConversationTurnDiff | null = null,
): ConversationTurnPreview {
  let title = "";
  let body = "";
  const files: ConversationTurnPreviewFile[] = [];

  for (const group of turn.groups) {
    const presentation = conversationIdentityPresentation(group.identity);
    if (!title && presentation === "user") {
      title = firstEntryText(group);
    }
    if (!body && presentation !== "user") {
      body = firstEntryText(group);
    }

    for (const entry of group.entries) {
      if (entry.kind !== "summary") {
        continue;
      }
      for (const file of entry.files) {
        files.push({
          name: file.name,
          iconName: null,
        });
      }
    }
  }

  if (turnDiff) {
    for (const file of turnDiff.files) {
      files.push({
        name: file.path,
        iconName: null,
      });
    }
  }

  if (!title) {
    title = turn.groups[0] ? firstEntryText(turn.groups[0]) || "Turn" : "Turn";
  }
  if (!body) {
    body = "No response yet.";
  }

  return {
    title,
    body,
    files: files.slice(0, 2),
    hiddenFileCount: Math.max(0, files.length - 2),
  };
}
