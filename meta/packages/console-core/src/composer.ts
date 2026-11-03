/**
 * Toolbar item kinds map to visual styles in the composer:
 * - "pill": labeled button with optional chevron (model, provider, reasoning)
 * - "pill-icon": compact icon-only pill, no background (plus/attach button)
 * - "sub-pill": smaller footer-level button (environment, permissions, branch, target)
 * - "icon": icon-only button with background (dictation, refresh)
 */
export type ConsoleComposerToolbarItemKind = "pill" | "pill-icon" | "sub-pill" | "icon";

export interface ConsoleComposerToolbarItem {
  id: string;
  kind: ConsoleComposerToolbarItemKind;
  label?: string;
  iconName?: string | null;
  /** When true, renders a chevron indicator to signal a dropdown/menu. */
  hasMenu?: boolean;
  disabled?: boolean;
  /** When true, the item is omitted from rendering entirely. */
  hidden?: boolean;
}

export interface ConsoleComposerViewState {
  /** Current textarea value. */
  value: string;
  /** When true, the entire composer (textarea + all buttons) is disabled. */
  disabled?: boolean;
  /** Textarea placeholder text. */
  placeholder?: string;
  /** When true, the submit button is disabled. */
  submitDisabled?: boolean;
  /** Accessible label / title for the submit button. */
  submitLabel?: string;
  /** Toolbar items rendered in the main row (inside the shell, below the textarea). */
  mainRowItems: ConsoleComposerToolbarItem[];
  /** Toolbar items rendered in the footer-left zone (below the shell). */
  footerLeftItems: ConsoleComposerToolbarItem[];
  /** Toolbar items rendered in the footer-right zone (below the shell). */
  footerRightItems: ConsoleComposerToolbarItem[];
}
