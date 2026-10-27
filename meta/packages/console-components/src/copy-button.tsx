import clsx from "clsx";
import { useEffect, useRef, useState } from "react";

import { CopyGlyph } from "./copy-glyph";
import { copyTextToClipboard, type IconRenderer } from "./shared";

type CopyButtonProps = {
  text: string;
  label: string;
  copiedLabel?: string;
  className?: string;
  Icon?: IconRenderer | null;
};

export function CopyButton({
  text,
  label,
  copiedLabel = "Copied",
  className,
  Icon,
}: CopyButtonProps) {
  const [copied, setCopied] = useState(false);
  const resetTimerRef = useRef<number | null>(null);
  const disabled = !text.trim();

  useEffect(() => () => {
    if (resetTimerRef.current != null) {
      window.clearTimeout(resetTimerRef.current);
    }
  }, []);

  async function handleClick() {
    if (disabled) {
      return;
    }

    const wasCopied = await copyTextToClipboard(text);
    if (!wasCopied) {
      return;
    }

    setCopied(true);
    if (resetTimerRef.current != null) {
      window.clearTimeout(resetTimerRef.current);
    }
    resetTimerRef.current = window.setTimeout(() => {
      setCopied(false);
      resetTimerRef.current = null;
    }, 1600);
  }

  return (
    <button
      className={clsx("cc-copy-btn", className)}
      type="button"
      aria-label={copied ? copiedLabel : label}
      title={copied ? copiedLabel : label}
      data-copied={copied ? "true" : undefined}
      disabled={disabled}
      onClick={() => {
        void handleClick();
      }}
    >
      {Icon ? <Icon name={copied ? "i-check" : "i-copy"} /> : <CopyGlyph state={copied ? "copied" : "idle"} />}
    </button>
  );
}
