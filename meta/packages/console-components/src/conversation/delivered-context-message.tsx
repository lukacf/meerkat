import type { ConsoleContextMessage } from "../../../console-core/src/context-record";
import { CopyButton } from "../copy-button";
import { CopyGlyph } from "../copy-glyph";

function QuoteCopyIcon({ name }: { name: string }) {
  return <CopyGlyph state={name === "i-check" ? "copied" : "idle"} />;
}

/** A source snapshot is user-provided data, never verified source identity. */
export function DeliveredContextMessage({ message }: { message: ConsoleContextMessage }) {
  return <div className="cc-delivered-context">
    <p className="cc-delivered-context__instruction">{message.instruction}</p>
    <div className="cc-delivered-context__sources" aria-label="Quoted context">
      {message.records.map((record) => <figure className="cc-delivered-context__source" key={record.id}>
        <figcaption className="cc-delivered-context__caption">
          <span>
            <strong>Quoted from {record.label}</strong>
            <small>User-provided snapshot</small>
          </span>
          <CopyButton Icon={QuoteCopyIcon} text={record.quote} label={`Copy quote from ${record.label}`} copiedLabel="Copied quote" />
        </figcaption>
        <blockquote className="cc-delivered-context__quote">{record.quote}</blockquote>
      </figure>)}
    </div>
  </div>;
}
