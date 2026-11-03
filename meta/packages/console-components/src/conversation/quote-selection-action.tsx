import { useEffect, useRef, useState, type RefObject } from "react";
import { readConsoleQuoteSelection, type ConsoleQuoteSelection } from "./context-selection";

/** A selection belongs to its transcript, never to another pane or the composer. */
export function QuoteSelectionAction({ viewportRef, onQuote, onError, disabled = false }: {
  viewportRef: RefObject<HTMLElement | null>;
  onQuote: (quote: ConsoleQuoteSelection) => void;
  onError: (message: string | null) => void;
  disabled?: boolean;
}) {
  const anchorRef = useRef<HTMLDivElement>(null);
  const [position, setPosition] = useState<{ left: number; top: number } | null>(null);
  useEffect(() => {
    const viewport = viewportRef.current;
    if (!viewport || disabled) { setPosition(null); return; }
    const update = () => {
      const selection = window.getSelection();
      if (readConsoleQuoteSelection(viewport, selection).kind === "empty") { setPosition(null); return; }
      const range = selection!.getRangeAt(0);
      const bounds = viewport.getBoundingClientRect();
      // Range geometry is absent in non-visual DOM hosts.
      const rect = range.getBoundingClientRect?.() ?? bounds;
      if (rect.bottom < bounds.top || rect.top > bounds.bottom) { setPosition(null); return; }
      const anchor = anchorRef.current!.getBoundingClientRect();
      const left = Math.max(bounds.left + 8, Math.min(rect.left, bounds.right - 42));
      const top = rect.top - 40 >= bounds.top + 4 ? rect.top - 40 : Math.min(rect.bottom + 6, bounds.bottom - 38);
      setPosition({ left: left - anchor.left, top: top - anchor.top });
    };
    const dismiss = (event: KeyboardEvent) => { if (event.key === "Escape") setPosition(null); };
    document.addEventListener("selectionchange", update);
    document.addEventListener("keydown", dismiss);
    window.addEventListener("resize", update);
    viewport.addEventListener("scroll", update, { passive: true });
    return () => {
      document.removeEventListener("selectionchange", update);
      document.removeEventListener("keydown", dismiss);
      window.removeEventListener("resize", update);
      viewport.removeEventListener("scroll", update);
    };
  }, [viewportRef, disabled]);
  return <div ref={anchorRef} className="cc-selection-anchor">
    {position && !disabled ? <button
      type="button" className="cc-conversation-quote" style={position}
      aria-label="Add to message" title="Quote selection"
      onPointerDown={(event) => event.preventDefault()}
      onClick={() => {
        const viewport = viewportRef.current;
        if (!viewport) return;
        const selection = window.getSelection();
        const result = readConsoleQuoteSelection(viewport, selection);
        onError(result.kind === "rejected" ? result.message : null);
        setPosition(null);
        if (result.kind === "selected") {
          onQuote(result.quote);
          selection?.removeAllRanges();
        }
      }}
    ><svg viewBox="0 0 20 20" width="18" height="18" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="M8 5H4v6h4V5Zm8 0h-4v6h4V5ZM8 11c0 2-1 3-3 4m11-4c0 2-1 3-3 4" /></svg></button> : null}
  </div>;
}
