// Inline glyphs for the kit's own marks (search field, select chevron, empty
// states, notices, icon buttons, views, memory kinds). They copy the console
// sprite's 24 px stroke paths (console/src/icon.tsx, the same name with an
// i- prefix), so the kit draws the same icons in any host, sprite or not.
// Always decorative: the owning control or text carries the meaning.

import type { JSX } from "react";

const PATHS = {
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
  ellipsis: (
    <>
      <circle cx="5" cy="12" r="2" />
      <circle cx="12" cy="12" r="2" />
      <circle cx="19" cy="12" r="2" />
    </>
  ),
  plus: <path d="M12 5v14M5 12h14" />,
  copy: (
    <>
      <rect x="9" y="9" width="11" height="11" rx="2" />
      <rect x="4" y="4" width="11" height="11" rx="2" />
    </>
  ),
  open: (
    <>
      <path d="M4 12V6a2 2 0 0 1 2-2h12" />
      <path d="M20 4v6h-6" />
      <path d="m20 4-9 9" />
      <path d="M20 14v4a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2v-4" />
    </>
  ),
  graph: (
    <>
      <circle cx="6" cy="6" r="2.2" />
      <circle cx="18" cy="8" r="2.2" />
      <circle cx="10" cy="18" r="2.2" />
      <path d="M8.2 6.4 15.8 7.6M6.7 8.1l2.6 7.8M16.6 9.7l-5.2 6.6" />
    </>
  ),
  lanes: (
    <>
      <path d="M3 6h4.25M10.75 6H21M3 12h10.25M16.75 12H21M3 18h2.25M8.75 18H21" />
      <circle cx="9" cy="6" r="1.75" />
      <circle cx="15" cy="12" r="1.75" />
      <circle cx="7" cy="18" r="1.75" />
    </>
  ),
  clusters: (
    <>
      <circle cx="8.5" cy="9" r="4.5" />
      <circle cx="16" cy="15.5" r="4.5" />
    </>
  ),
  tree: (
    <>
      <circle cx="6" cy="5" r="2" />
      <path d="M6 7v11h7M6 12h7" />
      <circle cx="15" cy="12" r="2" />
      <circle cx="15" cy="18" r="2" />
    </>
  ),
  list: (
    <>
      <path d="M9 6h11M9 12h11M9 18h11" />
      <circle cx="4.5" cy="6" r="1" />
      <circle cx="4.5" cy="12" r="1" />
      <circle cx="4.5" cy="18" r="1" />
    </>
  ),
  fit: <path d="M4 9V5h4M20 9V5h-4M4 15v4h4M20 15v4h-4" />,
  "zoom-in": (
    <>
      <circle cx="11" cy="11" r="6.5" />
      <path d="m20.5 20.5-4.9-4.9M11 8.5v5M8.5 11h5" />
    </>
  ),
  "zoom-out": (
    <>
      <circle cx="11" cy="11" r="6.5" />
      <path d="m20.5 20.5-4.9-4.9M8.5 11h5" />
    </>
  ),
  minimap: (
    <>
      <rect x="3" y="5" width="18" height="14" rx="2" />
      <rect x="12" y="11" width="6" height="5" rx="1" />
    </>
  ),
  filter: <path d="M4 6h16M7 12h10M10 18h4" />,
  calendar: (
    <>
      <rect x="4" y="5" width="16" height="15" rx="2" />
      <path d="M4 10h16M9 3v4M15 3v4" />
    </>
  ),
  memory: (
    <>
      <ellipse cx="12" cy="6" rx="7" ry="2.5" />
      <path d="M5 6v12c0 1.4 3.1 2.5 7 2.5s7-1.1 7-2.5V6M5 12c0 1.4 3.1 2.5 7 2.5s7-1.1 7-2.5" />
    </>
  ),
  route: (
    <>
      <rect x="3" y="9" width="6" height="6" rx="1" />
      <path d="M9 12h6.5" />
      <circle cx="18" cy="12" r="2.5" />
    </>
  ),
  flag: <path d="M5 21V4M5 4h11l-2 4 2 4H5" />,
  link: <path d="M10 13a5 5 0 0 0 7.5.5l3-3a5 5 0 0 0-7.1-7.1l-1.7 1.7M14 11a5 5 0 0 0-7.5-.5l-3 3a5 5 0 0 0 7.1 7.1l1.7-1.7" />,
  "arrow-right": <path d="M5 12h14M13 6l6 6-6 6" />,
  expand: <path d="M15 4h5v5M20 4l-6 6M9 20H4v-5M4 20l6-6" />,
  collapse: <path d="M19 10h-5V5M14 10l6-6M5 14h5v5M10 14l-6 6" />,
  warning: (
    <>
      <path d="M12 4 21.5 20h-19z" />
      <path d="M12 10v4M12 17h.01" />
    </>
  ),
  alert: (
    <>
      <path d="m12 3 9 9-9 9-9-9z" />
      <path d="M12 8.5V13M12 16h.01" />
    </>
  ),
  person: (
    <>
      <circle cx="12" cy="8" r="4" />
      <path d="M4.5 20a7.5 7.5 0 0 1 15 0" />
    </>
  ),
  minus: <path d="M5 12h14" />,
  chat: <path d="M4 5h16v11H9l-5 4z" />,
  grip: (
    <>
      <circle cx="9" cy="6" r="0.6" />
      <circle cx="15" cy="6" r="0.6" />
      <circle cx="9" cy="12" r="0.6" />
      <circle cx="15" cy="12" r="0.6" />
      <circle cx="9" cy="18" r="0.6" />
      <circle cx="15" cy="18" r="0.6" />
    </>
  ),
  "split-right": (
    <>
      <rect x="3" y="5" width="18" height="14" rx="2" />
      <path d="M12 5v14" />
    </>
  ),
  "split-down": (
    <>
      <rect x="3" y="5" width="18" height="14" rx="2" />
      <path d="M3 12h18" />
    </>
  ),
  sun: (
    <>
      <circle cx="12" cy="12" r="4" />
      <path d="M12 2.5v2M12 19.5v2M2.5 12h2M19.5 12h2M5.3 5.3l1.4 1.4M17.3 17.3l1.4 1.4M5.3 18.7l1.4-1.4M17.3 6.7l1.4-1.4" />
    </>
  ),
  moon: <path d="M20 14.5A8 8 0 0 1 9.5 4a8 8 0 1 0 10.5 10.5z" />,
  "k-preference": <path d="m12 3 2.8 5.7 6.2.9-4.5 4.4 1.1 6.2L12 17.2 6.4 20.2l1.1-6.2L3 9.6l6.2-.9L12 3z" />,
  "k-fact": (
    <>
      <circle cx="12" cy="12" r="8" />
      <circle cx="12" cy="12" r="1.5" />
    </>
  ),
  "k-gotcha": <path d="M12 4.5 20.5 19.5h-17z" />,
  "k-procedure": <path d="M4 6h8M8 12h8M12 18h8" />,
  "k-relationship": (
    <>
      <circle cx="6" cy="12" r="3" />
      <circle cx="18" cy="12" r="3" />
      <path d="M9 12h6" />
    </>
  ),
  "k-open-loop": <path d="M20 12a8 8 0 1 1-2.3-5.6" />,
  "k-reference": <path d="M7 4h10v16l-5-3.5L7 20z" />,
  topology: (
    <>
      <circle cx="12" cy="5" r="2" />
      <circle cx="5" cy="18" r="2" />
      <circle cx="19" cy="18" r="2" />
      <path d="M11 6.8 6 16.2M13 6.8l5 9.4M7 18h10" />
    </>
  ),
  today: <path d="M3 19h18M7 19a5 5 0 0 1 10 0M12 8v2.5M5.6 11.6l1.6 1.6M18.4 11.6l-1.6 1.6" />,
  approvals: (
    <>
      <rect x="4" y="4" width="16" height="16" rx="3" />
      <path d="m8.5 12 2.5 2.5 4.5-5" />
    </>
  ),
  roster: (
    <>
      <circle cx="8" cy="8.5" r="3" />
      <path d="M3 19a5 5 0 0 1 10 0M15.5 8h5M15.5 12h5M15.5 16h3.5" />
    </>
  ),
  routing: <path d="M3 12h6l4-5h7M13 17h7M9 12l4 5M17 4l3 3-3 3M17 14l3 3-3 3" />,
  logs: <path d="M4 6h2M9 6h11M4 12h2M9 12h11M4 18h2M9 18h7" />,
  health: <path d="M3 12h4l2-5 4 10 2-5h6" />,
  access: (
    <>
      <circle cx="8" cy="15" r="4" />
      <path d="m10.8 12.2 9.2-9.2M17 6l3 3M14.5 8.5l2 2" />
    </>
  ),
  workgraph: (
    <>
      <rect x="3" y="4" width="6" height="5" rx="1" />
      <rect x="15" y="4" width="6" height="5" rx="1" />
      <rect x="9" y="15" width="6" height="5" rx="1" />
      <path d="M6 9v1.5a2 2 0 0 0 2 2h8a2 2 0 0 0 2-2V9M12 12.5V15" />
    </>
  ),
  schedules: (
    <>
      <path d="M20 12a8 8 0 1 1-2.3-5.6M20 4v3h-3" />
      <path d="M12 8v4l2.5 1.5" />
    </>
  ),
} satisfies Record<string, JSX.Element>;

export type KitGlyphName = keyof typeof PATHS;

/** Every glyph name, in definition order (the kit gallery shows them all). */
export const KIT_GLYPH_NAMES = Object.keys(PATHS) as KitGlyphName[];

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
