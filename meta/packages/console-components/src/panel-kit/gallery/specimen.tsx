import type { CSSProperties, ReactNode } from "react";

import { cx } from "../cx";

/** A specimen card in the gallery: the part on the card surface with a
 * caption above it. */
export function Specimen({
  title,
  note,
  width,
  bare,
  className,
  children,
}: {
  title?: string;
  note?: ReactNode;
  /** A fixed width (px) for responsive parts, as a pane would give them. */
  width?: number;
  /** No inner padding: panels and strips run to the card's edges. */
  bare?: boolean;
  className?: string;
  children?: ReactNode;
}) {
  const style: CSSProperties | undefined = width ? { width, maxWidth: "100%" } : undefined;
  return (
    <figure className={cx("kg-specimen", className)} style={style}>
      {title || note ? (
        <figcaption className="kg-specimen__cap">
          {title ? <span className="kg-specimen__title">{title}</span> : null}
          {note ? <span className="kg-specimen__note">{note}</span> : null}
        </figcaption>
      ) : null}
      <div className={cx("kg-specimen__body", bare && "kg-specimen__body--bare")}>{children}</div>
    </figure>
  );
}

/** A row of specimens that wraps. */
export function SpecimenRow({ children, className }: { children?: ReactNode; className?: string }) {
  return <div className={cx("kg-row", className)}>{children}</div>;
}

/** A labelled item inside a specimen (a mark and its word, a chart and its size). */
export function Item({ label, children }: { label?: ReactNode; children?: ReactNode }) {
  return (
    <div className="kg-item">
      <div className="kg-item__part">{children}</div>
      {label ? <div className="kg-item__label">{label}</div> : null}
    </div>
  );
}
