import { useId, type MouseEvent, type ReactNode } from "react";

import { Sparkline, type SparkValues } from "../viz/charts/sparkline";
import { cx } from "./cx";
import { StatusMark } from "./status-mark";
import { TechnicalDetails, type TechnicalDetail } from "./technical-details";

export type EntityKind = "agent" | "member" | "task" | "memory" | "route" | "rule" | "approval";

export interface EntityNow {
  /** "Replying to Support Triage", "Running web_search · 12 s". */
  text: ReactNode;
  /** Happening now: the live mark leads the line. */
  live?: boolean;
}

export interface EntityTrend {
  values: SparkValues;
  /** The trend in words ("12 turns today, most at 14:00"). */
  label: string;
  live?: boolean;
  kind?: "line" | "bars";
}

export interface EntityCardProps {
  /** "card" for grids, drawers and tooltips; "row" for lists. */
  layout?: "card" | "row";
  kind: EntityKind;
  title: ReactNode;
  subtitle?: ReactNode;
  /** AgentAvatar, a StatusSquare, a memory kind glyph. */
  mark?: ReactNode;
  /** Usually a StatusPill. */
  status?: ReactNode;
  now?: EntityNow;
  trend?: EntityTrend;
  /** Small facts: "12 turns today", the trust ladder, "blocked by 2". */
  figures?: ReadonlyArray<ReactNode>;
  /** AgentName chips, "+N". */
  relations?: ReactNode;
  /** A RelativeTime or RelativeTimeChip. */
  time?: ReactNode;
  /** Hover-revealed actions (top right on a card, at the end of a row). */
  actions?: ReactNode;
  /** Technical details at the foot of a card. */
  technical?: ReadonlyArray<TechnicalDetail | null | false | undefined>;
  selected?: boolean;
  /** Working now: a 2 px accent left rule. */
  live?: boolean;
  /** Opens the entity; its title becomes the entity's button. */
  onActivate?: () => void;
  /** The button's name when the title is not plain text. */
  activateLabel?: string;
  className?: string;
  testId?: string;
  dataAttributes?: Record<`data-${string}`, string | undefined>;
}

/**
 * An entity (spec A3.4): an agent, member, work item, memory, route, rule or
 * approval, as a card (grids, drawers) or a row (lists). The mark, the title
 * and its subtitle, the status pill, the live line ("Replying to Support
 * Triage"), a trend, small figures, relations, the time and hover-revealed
 * actions. An entity that opens is never one big button: its title is the
 * button, described by the rest, and a click anywhere else on it (not on a
 * control) opens it too.
 */
export function EntityCard({
  layout = "card",
  kind,
  title,
  subtitle,
  mark,
  status,
  now,
  trend,
  figures,
  relations,
  time,
  actions,
  technical,
  selected,
  live,
  onActivate,
  activateLabel,
  className,
  testId,
  dataAttributes,
}: EntityCardProps) {
  const partId = useId();
  const row = layout === "row";
  const subId = subtitle ? `${partId}-sub` : undefined;
  const nowId = now ? `${partId}-now` : undefined;
  const titleNode = onActivate ? (
    <button
      type="button"
      className="cc-entity__hit"
      aria-label={activateLabel}
      aria-current={selected ? "true" : undefined}
      aria-describedby={[subId, nowId].filter(Boolean).join(" ") || undefined}
      onClick={onActivate}
    >
      <span className="cc-entity__title">{title}</span>
    </button>
  ) : (
    <span className="cc-entity__title">{title}</span>
  );
  const onClick = onActivate
    ? (event: MouseEvent<HTMLDivElement>) => {
      const target = event.target as Element | null;
      if (target?.closest?.("a, button, input, select, textarea, summary, label, [role='button']")) return;
      onActivate();
    }
    : undefined;
  const nowNode = now ? (
    <span className="cc-entity__now" id={nowId}>
      {now.live ? <StatusMark tone="live" /> : null}
      <span className="cc-entity__now-text">{now.text}</span>
    </span>
  ) : null;
  const trendNode = trend ? (
    <Sparkline
      className="cc-entity__trend"
      values={trend.values}
      label={trend.label}
      live={trend.live}
      kind={trend.kind ?? "bars"}
      width={row ? 64 : 96}
      height={row ? 16 : 20}
    />
  ) : null;
  const figuresNode = figures?.length ? (
    <span className="cc-entity__figures">
      {figures.map((figure, index) => (
        <span className="cc-entity__figure" key={index}>
          {figure}
        </span>
      ))}
    </span>
  ) : null;
  const actionsNode = actions ? <span className="cc-entity__actions cc-reveal">{actions}</span> : null;
  const classes = cx(
    "cc-entity",
    row ? "cc-entity--row" : "cc-entity--card",
    "cc-reveal-host",
    onActivate && "is-interactive",
    selected && "is-selected",
    live && "is-live",
    className,
  );
  if (row) {
    return (
      <div className={classes} data-kind={kind} data-testid={testId} onClick={onClick} {...dataAttributes}>
        {mark ? <span className="cc-entity__mark">{mark}</span> : null}
        <span className="cc-entity__main">
          {titleNode}
          {subtitle ? (
            <span className="cc-entity__sub" id={subId}>
              {subtitle}
            </span>
          ) : null}
          {nowNode}
        </span>
        {figuresNode}
        {trendNode}
        {status ? <span className="cc-entity__status">{status}</span> : null}
        {time ? <span className="cc-entity__time">{time}</span> : null}
        {actionsNode}
      </div>
    );
  }
  return (
    <div className={classes} data-kind={kind} data-testid={testId} onClick={onClick} {...dataAttributes}>
      <div className="cc-entity__head">
        {mark ? <span className="cc-entity__mark">{mark}</span> : null}
        <span className="cc-entity__main">
          {titleNode}
          {subtitle ? (
            <span className="cc-entity__sub" id={subId}>
              {subtitle}
            </span>
          ) : null}
        </span>
        {status ? <span className="cc-entity__status">{status}</span> : null}
        {actionsNode}
      </div>
      {nowNode}
      {trendNode || figuresNode || time ? (
        <div className="cc-entity__facts">
          {trendNode}
          {figuresNode}
          {time ? <span className="cc-entity__time">{time}</span> : null}
        </div>
      ) : null}
      {relations ? <div className="cc-entity__relations">{relations}</div> : null}
      {technical ? <TechnicalDetails items={technical} className="cc-entity__tech" /> : null}
    </div>
  );
}

/** An entity as a list row (A3.4): 40 px comfortable, 30 px compact. */
export function EntityRow(props: Omit<EntityCardProps, "layout">) {
  return <EntityCard {...props} layout="row" />;
}

const TRUST_WORDS = ["", "Untrusted", "Agent claimed", "Agent verified", "Steward verified", "Operator"];

/**
 * A memory's trust as a small ladder (A3.4): five tiny bars filled to the
 * level, untrusted 1 to operator 5, then the word.
 */
export function TrustLadder({
  level,
  label,
  className,
}: {
  /** 1 (untrusted) to 5 (operator). */
  level: number;
  /** The trust in words; the default word for the level otherwise. */
  label?: string;
  className?: string;
}) {
  const value = Math.min(5, Math.max(0, Math.round(level)));
  const word = label ?? TRUST_WORDS[value] ?? "";
  return (
    <span className={cx("cc-trust", className)} data-level={value}>
      <span className="cc-trust__bars" role="img" aria-label={`Trust: ${word}, ${value} of 5`}>
        {[1, 2, 3, 4, 5].map((step) => (
          <i key={step} className={cx("cc-trust__bar", step <= value && "is-on")} />
        ))}
      </span>
      {word ? <span className="cc-trust__word">{word}</span> : null}
    </span>
  );
}
