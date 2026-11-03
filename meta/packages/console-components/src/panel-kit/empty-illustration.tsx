import type { JSX } from "react";

import { cx } from "./cx";

export type IllustrationName =
  | "agents"
  | "connections"
  | "work"
  | "memory"
  | "approvals-clear"
  | "timeline"
  | "routes"
  | "search"
  | "no-access"
  | "not-configured"
  | "error";

// Small vignettes in the brand's graph grammar (spec A3.7): 1.25 px strokes
// in --_cc-viz-mid, fills in --_cc-viz-ink, at most one accent element, no
// text, no gradients, on a 4 px grid in a 120 x 72 box.
const ART: Record<IllustrationName, JSX.Element> = {
  agents: (
    <>
      <path className="cc-illus__line cc-illus__dash" d="M16 40H104" />
      <circle className="cc-illus__line cc-illus__plate" cx="32" cy="40" r="7" />
      <circle className="cc-illus__ink" cx="60" cy="40" r="8" />
      <circle className="cc-illus__line cc-illus__plate" cx="88" cy="40" r="7" />
    </>
  ),
  connections: (
    <>
      <path className="cc-illus__line cc-illus__dash" d="M40 36H52M68 36H80" />
      <path className="cc-illus__line" d="M56 32 64 40M64 32 56 40" />
      <circle className="cc-illus__ink" cx="32" cy="36" r="7" />
      <circle className="cc-illus__ink" cx="88" cy="36" r="7" />
    </>
  ),
  work: (
    <>
      <rect className="cc-illus__line cc-illus__dash" x="16" y="16" width="88" height="40" rx="4" />
      <path className="cc-illus__line" d="M44 36H52M68 36H76" />
      <rect className="cc-illus__line cc-illus__plate" x="32" y="30" width="12" height="12" rx="2" />
      <rect className="cc-illus__accent" x="52" y="30" width="16" height="12" rx="2" />
      <rect className="cc-illus__line cc-illus__plate" x="76" y="30" width="12" height="12" rx="2" />
    </>
  ),
  memory: (
    <>
      <path className="cc-illus__line cc-illus__plate" d="M28 20C28 17 35 14 44 14C53 14 60 17 60 20V52C60 55 53 58 44 58C35 58 28 55 28 52Z" />
      <path className="cc-illus__line" d="M28 20C28 23 35 26 44 26C53 26 60 23 60 20M28 36C28 39 35 42 44 42C53 42 60 39 60 36" />
      <path className="cc-illus__line cc-illus__dash" d="M81 28A12 12 0 0 0 81 44" />
      <circle className="cc-illus__ink" cx="92" cy="36" r="6" />
    </>
  ),
  "approvals-clear": (
    <>
      <circle className="cc-illus__line" cx="60" cy="36" r="20" />
      <path className="cc-illus__line" d="M50 37 57 44 71 29" />
    </>
  ),
  timeline: (
    <>
      <path className="cc-illus__line" d="M16 52H104M24 52V56M44 52V56M64 52V56M84 52V56" />
      <rect className="cc-illus__ink" x="24" y="20" width="28" height="4" rx="2" />
      <rect className="cc-illus__mid" x="40" y="32" width="36" height="4" rx="2" />
      <rect className="cc-illus__accent" x="68" y="44" width="28" height="4" rx="2" />
    </>
  ),
  routes: (
    <>
      <rect className="cc-illus__line cc-illus__plate" x="20" y="30" width="12" height="12" rx="1" />
      <path className="cc-illus__line cc-illus__dash" d="M32 36H76" />
      <path className="cc-illus__line" d="M72 32 77 36 72 40" />
      <circle className="cc-illus__ink" cx="88" cy="36" r="7" />
    </>
  ),
  search: (
    <>
      <circle className="cc-illus__line" cx="52" cy="32" r="13" />
      <path className="cc-illus__line" d="M61.5 41.5 70 50" />
      <circle className="cc-illus__mid" cx="80" cy="54" r="1.5" />
      <circle className="cc-illus__mid" cx="88" cy="56" r="1.5" />
      <circle className="cc-illus__mid" cx="96" cy="56" r="1.5" />
    </>
  ),
  "no-access": (
    <>
      <path className="cc-illus__line" d="M60 12 80 20V36C80 48 71 56 60 60C49 56 40 48 40 36V20Z" />
      <circle className="cc-illus__ink" cx="60" cy="33" r="3.5" />
      <path className="cc-illus__line cc-illus__bold" d="M60 36V44" />
    </>
  ),
  "not-configured": (
    <>
      <rect className="cc-illus__line cc-illus__dash" x="24" y="24" width="24" height="24" rx="3" />
      <path className="cc-illus__line" d="M60 32H68M60 40H68" />
      <rect className="cc-illus__line cc-illus__plate" x="68" y="28" width="12" height="16" rx="2" />
      <path className="cc-illus__line" d="M80 36C88 36 88 48 96 48H104" />
    </>
  ),
  error: (
    <>
      <path className="cc-illus__line" d="M16 36H50M70 36H104" />
      <path className="cc-illus__crit" d="M60 29 67 36 60 43 53 36Z" />
    </>
  ),
};

/**
 * A small vignette above an empty state's title (spec A3.7): 120 x 72 in a
 * panel, 72 x 44 inline. Decorative (aria-hidden): the title and body say
 * what it shows.
 */
export function EmptyIllustration({
  name,
  size = "panel",
  className,
}: {
  name: IllustrationName;
  size?: "panel" | "inline";
  className?: string;
}): JSX.Element {
  const inline = size === "inline";
  return (
    <svg
      className={cx("cc-illus", className)}
      data-illustration={name}
      data-size={size}
      width={inline ? 72 : 120}
      height={inline ? 44 : 72}
      style={{ width: inline ? 72 : 120, height: inline ? 44 : 72 }}
      viewBox="0 0 120 72"
      aria-hidden="true"
      focusable="false"
    >
      {ART[name]}
    </svg>
  );
}
