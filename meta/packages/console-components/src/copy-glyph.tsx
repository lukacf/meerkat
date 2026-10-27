/**
 * Icon for copy buttons: a copy mark, a check after a successful copy, or a
 * cross after a failed one. Purely decorative (`aria-hidden`); the owning
 * button carries the accessible name. `data-icon` exposes the state to tests.
 */
export type CopyGlyphState = "idle" | "copied" | "failed";

export function CopyGlyph({ state = "idle" }: { state?: CopyGlyphState }) {
  const icon = state === "copied" ? "check" : state === "failed" ? "cross" : "copy";
  return (
    <svg
      aria-hidden="true"
      className="cc-copy-glyph"
      data-icon={icon}
      fill="none"
      focusable="false"
      height="14"
      stroke="currentColor"
      strokeLinecap="round"
      strokeLinejoin="round"
      strokeWidth="1.6"
      viewBox="0 0 16 16"
      width="14"
    >
      {icon === "check" ? (
        <path d="M3.5 8.5l3 3 6-7" />
      ) : icon === "cross" ? (
        <path d="M4.5 4.5l7 7M11.5 4.5l-7 7" />
      ) : (
        <>
          <rect height="9" rx="1.5" width="8" x="5.5" y="5" />
          <path d="M10.5 3.5V3a1.5 1.5 0 0 0-1.5-1.5H4A1.5 1.5 0 0 0 2.5 3v6A1.5 1.5 0 0 0 4 10.5h.5" />
        </>
      )}
    </svg>
  );
}
