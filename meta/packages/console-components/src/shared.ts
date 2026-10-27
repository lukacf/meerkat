import type { CSSProperties, ComponentType } from "react";

export type IconRenderer = ComponentType<{ name: string; className?: string }>;

export function toneStyle(
  tone: { variables?: Record<string, string> | null } | null | undefined,
): CSSProperties | undefined {
  if (!tone?.variables) {
    return undefined;
  }
  return tone.variables as unknown as CSSProperties;
}

function fallbackCopyTextToClipboard(text: string): boolean {
  if (typeof document === "undefined" || !document.body || typeof document.execCommand !== "function") {
    return false;
  }

  const textarea = document.createElement("textarea");
  textarea.value = text;
  textarea.setAttribute("readonly", "true");
  textarea.style.position = "fixed";
  textarea.style.top = "0";
  textarea.style.left = "0";
  textarea.style.opacity = "0";
  textarea.style.pointerEvents = "none";
  document.body.appendChild(textarea);

  const selection = typeof document.getSelection === "function" ? document.getSelection() : null;
  const existingRanges = selection
    ? Array.from({ length: selection.rangeCount }, (_value, index) => selection.getRangeAt(index))
    : [];

  textarea.focus();
  textarea.select();
  textarea.setSelectionRange(0, textarea.value.length);

  let copied = false;
  try {
    copied = document.execCommand("copy");
  } catch {
    copied = false;
  }

  document.body.removeChild(textarea);

  if (selection) {
    selection.removeAllRanges();
    existingRanges.forEach((range) => selection.addRange(range));
  }

  return copied;
}

export async function copyTextToClipboard(text: string): Promise<boolean> {
  if (!text.trim()) {
    return false;
  }

  if (typeof navigator !== "undefined" && navigator.clipboard?.writeText) {
    try {
      await navigator.clipboard.writeText(text);
      return true;
    } catch {
      // Fall through to a DOM-based copy fallback when the async clipboard
      // API is unavailable in Electron or denied by the embedding context.
    }
  }

  return fallbackCopyTextToClipboard(text);
}

const copyFeedbackTimers = new WeakMap<HTMLElement, number>();

export function showCopiedButtonState(button: HTMLElement | null): void {
  if (!button || typeof window === "undefined") {
    return;
  }

  if (!button.dataset.copyDefaultIcon) {
    button.dataset.copyDefaultIcon = button.innerHTML;
  }
  if (!button.dataset.copyDefaultLabel) {
    button.dataset.copyDefaultLabel = button.getAttribute("aria-label") || button.getAttribute("title") || "Copy";
  }
  if (!button.dataset.copyCopiedLabel) {
    button.dataset.copyCopiedLabel = "Copied";
  }

  const existingTimer = copyFeedbackTimers.get(button);
  if (existingTimer != null) {
    window.clearTimeout(existingTimer);
  }

  button.dataset.copied = "true";
  button.innerHTML = "<svg><use href=\"#i-check\"></use></svg>";
  button.setAttribute("aria-label", button.dataset.copyCopiedLabel);
  button.setAttribute("title", button.dataset.copyCopiedLabel);

  const resetTimer = window.setTimeout(() => {
    button.removeAttribute("data-copied");
    button.innerHTML = button.dataset.copyDefaultIcon || button.innerHTML;
    const defaultLabel = button.dataset.copyDefaultLabel || "Copy";
    button.setAttribute("aria-label", defaultLabel);
    button.setAttribute("title", defaultLabel);
    copyFeedbackTimers.delete(button);
  }, 1600);

  copyFeedbackTimers.set(button, resetTimer);
}
