export type ConsoleQuoteSelection = { text: string; messageId: string; sourceText?: string };
export type ConsoleQuoteSelectionResult =
  | { kind: "selected"; quote: ConsoleQuoteSelection }
  | { kind: "empty" }
  | { kind: "rejected"; message: string };

/** Wrappers opt in explicitly; selection in navigation and tool chrome is excluded. */
export function readConsoleQuoteSelection(root: HTMLElement, selection: Selection | null): ConsoleQuoteSelectionResult {
  if (!selection || selection.isCollapsed || selection.rangeCount !== 1) return { kind: "empty" };
  const range = selection.getRangeAt(0);
  const elementFor = (node: Node): Element | null => node.nodeType === 1 ? node as Element : node.parentElement;
  const start = elementFor(range.startContainer)?.closest<HTMLElement>("[data-quote-message-id]");
  const end = elementFor(range.endContainer)?.closest<HTMLElement>("[data-quote-message-id]");
  if (!start || !end || !root.contains(start) || !root.contains(end)) return { kind: "empty" };
  if (start !== end) return { kind: "rejected", message: "Select text from one message at a time." };
  const excluded = start.querySelectorAll("[data-quote-exclude], button, nav, input, textarea, [role=button]");
  for (const node of excluded) {
    if (range.intersectsNode(node)) return { kind: "rejected", message: "Select only the message text, without tool controls or metadata." };
  }
  const text = selection.toString();
  const messageId = start.dataset.quoteMessageId;
  if (!text.trim() || !messageId) return { kind: "empty" };
  return { kind: "selected", quote: { text, messageId, sourceText: start.dataset.quoteSource } };
}
