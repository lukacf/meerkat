import type { ConversationSummaryEntry } from "@console-core";

import { ChangeStatPair } from "./change-stat-pair";

type SummaryCardProps = {
  entry: ConversationSummaryEntry;
  onAction?: (entry: ConversationSummaryEntry) => void;
};

export function SummaryCard({ entry, onAction }: SummaryCardProps) {
  return (
    <section className="cc-summary-card">
      <div className="cc-summary-card__header">
        <span className="cc-summary-card__title">
          {entry.title} <ChangeStatPair minus={entry.minus} plus={entry.plus} />
        </span>
        {entry.actionLabel && onAction ? (
          <button className="cc-summary-card__action" type="button" onClick={() => onAction(entry)}>
            {entry.actionLabel}
          </button>
        ) : null}
      </div>
      <div className="cc-summary-card__files">
        {entry.files.map((file) => (
          <div className="cc-summary-card__file" key={file.name}>
            <span className="cc-summary-card__file-name">{file.name}</span>
            <ChangeStatPair className="cc-summary-card__file-stats" minus={file.minus} plus={file.plus} />
          </div>
        ))}
      </div>
    </section>
  );
}
