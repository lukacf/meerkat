import { formatCount, type ConversationTurnDiff, type ConversationTurnDiffFile, type ConversationTurnDiffLine } from "@console-core";

import { ChangeStatPair } from "./change-stat-pair";

function TurnDiffLineView({ line }: { line: ConversationTurnDiffLine }) {
  const marker = line.type === "add" ? "+" : line.type === "remove" ? "-" : " ";
  return (
    <div className={`cc-turn-diff-card__line is-${line.type}`} key={`${line.oldLine ?? "x"}-${line.newLine ?? "y"}-${line.text}`}>
      <span className="cc-turn-diff-card__line-no">{line.oldLine != null ? String(line.oldLine) : ""}</span>
      <span className="cc-turn-diff-card__line-no">{line.newLine != null ? String(line.newLine) : ""}</span>
      <span className="cc-turn-diff-card__line-mark">{marker}</span>
      <span className="cc-turn-diff-card__line-text">{line.text}</span>
    </div>
  );
}

function TurnDiffFileView({
  expanded,
  file,
  onToggle,
}: {
  expanded: boolean;
  file: ConversationTurnDiffFile;
  onToggle: () => void;
}) {
  return (
    <div className={`cc-turn-diff-card__file${expanded ? " is-expanded" : ""}`}>
      <button className="cc-turn-diff-card__file-row" type="button" onClick={onToggle}>
        <span className="cc-turn-diff-card__file-left">
          <span className="cc-turn-diff-card__file-name">{file.path}</span>
          <ChangeStatPair className="cc-turn-diff-card__file-stats" minus={file.minus} plus={file.plus} />
        </span>
        <span className="cc-turn-diff-card__file-caret">{expanded ? "⌃" : "⌄"}</span>
      </button>
      {expanded ? (
        <div className="cc-turn-diff-card__file-body">
          {file.hunks.map((hunk, index) => (
            <div className="cc-turn-diff-card__hunk" key={`${file.path}-${index}`}>
              <div className="cc-turn-diff-card__hunk-header">@@ -{hunk.oldStart},{hunk.oldLines} +{hunk.newStart},{hunk.newLines} @@</div>
              <div className="cc-turn-diff-card__lines">
                {hunk.lines.map((line) => <TurnDiffLineView key={`${line.oldLine ?? "x"}-${line.newLine ?? "y"}-${line.text}`} line={line} />)}
              </div>
            </div>
          ))}
        </div>
      ) : null}
    </div>
  );
}

type TurnDiffCardProps = {
  turnDiff: ConversationTurnDiff;
  expandedFile: string | null;
  onToggleFile: (filePath: string) => void;
};

export function TurnDiffCard({
  turnDiff,
  expandedFile,
  onToggleFile,
}: TurnDiffCardProps) {
  return (
    <section className="cc-summary-card cc-turn-diff-card">
      <div className="cc-summary-card__header">
        <span className="cc-summary-card__title">
          {`${formatCount(turnDiff.fileCount)} ${turnDiff.fileCount === 1 ? "file" : "files"} changed`} <ChangeStatPair minus={turnDiff.minus} plus={turnDiff.plus} />
        </span>
      </div>
      <div className="cc-turn-diff-card__files">
        {turnDiff.files.map((file) => (
          <TurnDiffFileView
            expanded={expandedFile === file.path}
            file={file}
            key={file.path}
            onToggle={() => onToggleFile(file.path)}
          />
        ))}
      </div>
    </section>
  );
}
