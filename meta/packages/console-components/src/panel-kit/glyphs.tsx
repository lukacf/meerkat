// Inline glyphs for the kit's own marks (search field, select chevron, empty
// states, notices). They copy the console sprite's 24 px stroke paths, so the
// kit draws the same icons in any host, sprite or not. Always decorative:
// the owning control or text carries the meaning.

import type { JSX } from "react";

export type KitGlyphName = "search" | "chevron" | "info" | "shield" | "check" | "refresh" | "clock" | "close";

const PATHS: Record<KitGlyphName, JSX.Element> = {
  search: (
    <>
      <circle cx="11" cy="11" r="6" />
      <path d="m20 20-4.35-4.35" />
    </>
  ),
  chevron: <path d="m7 10 5 5 5-5" />,
  info: (
    <>
      <circle cx="12" cy="12" r="9" />
      <path d="M12 10v6" />
      <path d="M12 7h.01" />
    </>
  ),
  shield: <path d="M12 3 4 6v6c0 5 3.5 8 8 9 4.5-1 8-4 8-9V6z" />,
  check: <path d="m5 12 4.2 4.2L19 6.5" />,
  refresh: (
    <>
      <path d="M21 12a9 9 0 0 1-15.4 6.4" />
      <path d="M3 12A9 9 0 0 1 18.4 5.6" />
      <path d="M3 16v-4h4" />
      <path d="M21 8v4h-4" />
    </>
  ),
  clock: (
    <>
      <circle cx="12" cy="12" r="9" />
      <path d="M12 7v6l4 2" />
    </>
  ),
  close: <path d="m6 6 12 12M18 6 6 18" />,
};

export function KitGlyph({ name, className }: { name: KitGlyphName; className?: string }): JSX.Element {
  return (
    <svg
      aria-hidden="true"
      focusable="false"
      className={className ? `cc-glyph ${className}` : "cc-glyph"}
      data-glyph={name}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      {PATHS[name]}
    </svg>
  );
}
