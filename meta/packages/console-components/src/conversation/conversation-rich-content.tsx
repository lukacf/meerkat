import { canFoldCompletedTools, CompletedToolDisclosure, explicitDisplayLabel, groupRoutineToolRows, peerDisplayLabel, useConversationDisplayLabels, useInsideCompletedToolDisclosure, useRowState } from "./presentation-policy";
import clsx from "clsx";

import {
  memberKickoffPhaseLabel,
  conversationRichPeerBodyForDisplay,
  conversationRichPeerIntentForDisplay,
  normalizeConversationDisplayText,
  renderConversationInlineMarkdown,
  type ConversationRichBlock,
  type ConversationRichCodeBlock,
  type ConversationRichCommandBlock,
  type ConversationRichFileChangeBlock,
  type ConversationRichImageBlock,
  type ConversationRichToolCallBlock,
  type ConversationTableAlignment,
  type ConversationRichThinkingBlock,
} from "@console-core";

import { cloneElement, useEffect, useRef, useState, type KeyboardEvent, type ReactElement } from "react";

import { copyTextToClipboard } from "../shared";

import { ConversationMarkdown, type MarkdownUrlPolicy } from "./conversation-markdown";
import { useConversationImageSource } from "./blob-image";

import { ChangeStatPair } from "./change-stat-pair";
import { CopyButton } from "../copy-button";
import { CopyGlyph } from "../copy-glyph";
import type { IconRenderer } from "../shared";

type ConversationRichContentProps = {
  blocks: ConversationRichBlock[];
  markdownUrlPolicy?: MarkdownUrlPolicy;
  richStyle?: "default" | "streaming";
  Icon?: IconRenderer | null;
  // Default true preserves meerkat-studio's display normalization; the Meta
  // console passes false for faithful rendering of raw agent text (pairs with
  // the parse-side `displayNormalization` option).
  displayNormalization?: boolean;
};

function markdownHtml(text: string, displayNormalization = true) {
  return { __html: renderConversationInlineMarkdown(text, { displayNormalization }) };
}

function commandCopyText(block: ConversationRichCommandBlock): string {
  return [block.title, block.body, block.output || "", block.footer || ""].filter(Boolean).join("\n").trim();
}

function fileChangeCopyText(block: ConversationRichFileChangeBlock): string {
  return [
    block.verb,
    block.before || "",
    block.name,
    block.after || "",
    `+${block.plus}`,
    `-${block.minus}`,
  ].filter(Boolean).join(" ").replace(/\s+/g, " ").trim();
}

function alignmentAttr(alignment: ConversationTableAlignment | null | undefined) {
  return alignment || "left";
}

function ThinkingBlock({ block, index, displayNormalization = true }: { block: ConversationRichThinkingBlock; index: number; displayNormalization?: boolean }) {
  // Hydration changes provenance, not the reader's disclosure choice.
  const [initiallyOpen] = useRowState(`thinking-initial:${index}`, () => !(block.final && block.persisted));
  const [open, setOpen] = useRowState(`thinking:${index}`, () => initiallyOpen);
  if (!block.label?.trim() && !block.text?.trim()) {
    return null;
  }
  const collapsedByDefault = !initiallyOpen;
  return (
    <details
      className={clsx(
        "cc-rich-thinking",
        block.final && "cc-rich-thinking--final",
        block.persisted && "cc-rich-thinking--persisted",
        collapsedByDefault && "cc-rich-thinking--collapsed",
      )}
      open={open}
      onToggle={(event) => { if (event.currentTarget.open !== open) setOpen(event.currentTarget.open); }}
    >
      <summary className="cc-rich-thinking__label">{block.label?.trim() ? block.label : "Thinking"}</summary>
      <p className="cc-rich-paragraph cc-rich-thinking__body" dangerouslySetInnerHTML={markdownHtml(block.text, displayNormalization)} />
    </details>
  );
}

function renderBlock(
  block: ConversationRichBlock,
  index: number,
  Icon?: IconRenderer | null,
  displayNormalization = true,
  markdownUrlPolicy?: MarkdownUrlPolicy,
) {
  if (block.type === "markdown") {
    return <ConversationMarkdown block={block} urlPolicy={markdownUrlPolicy} key={block.id} />;
  }

  if (block.type === "background-job") {
    const statusLabels: Record<string, string> = {
      completed: "Completed", failed: "Failed", aborted: "Aborted",
      cancelled: "Cancelled", retired: "Retired", terminated: "Terminated",
    };
    const statusLabel = Object.hasOwn(statusLabels, block.status) ? statusLabels[block.status] : block.status;
    return (
      <section className="cc-background-job" data-job-id={block.jobId} key={`background-job-${index}`}>
        <header className="cc-background-job__header">
          <svg className="cc-background-job__icon" viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="1.5" aria-hidden="true">
            <rect x="4" y="3.5" width="12" height="13" rx="2" />
            <path d="M7 7h6M7 10h6M7 13h3" strokeLinecap="round" />
          </svg>
          <div className="cc-background-job__heading">
            <span className="cc-background-job__kind">Background job</span>
            {block.displayName?.trim() ? <strong className="cc-background-job__name">{block.displayName}</strong> : null}
          </div>
          <span className="cc-background-job__status" data-status={block.status}>
            <span className="cc-background-job__status-dot" aria-hidden="true" />{statusLabel}
          </span>
        </header>
        {block.detail ? <div className="cc-background-job__detail">{block.detail}</div> : null}
      </section>
    );
  }

  if (block.type === "member-kickoff") {
    return (
      <section
        className="cc-member-kickoff"
        data-phase={block.phase}
        aria-label={block.copyText}
        key={`member-kickoff-${index}`}
      >
        <svg className="cc-member-kickoff__icon" viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="1.5" aria-hidden="true">
          <circle cx="10" cy="7" r="3" />
          <path d="M4.5 16.5c.8-2.8 3-4.5 5.5-4.5s4.7 1.7 5.5 4.5" strokeLinecap="round" />
        </svg>
        <span className="cc-member-kickoff__kind">Kickoff</span>
        <strong className="cc-member-kickoff__member">{block.member}</strong>
        {block.role ? <span className="cc-member-kickoff__role">{block.role}</span> : null}
        <span className="cc-member-kickoff__phase" data-phase={block.phase}>
          <span className="cc-member-kickoff__phase-dot" aria-hidden="true" />
          {memberKickoffPhaseLabel(block.phase)}
        </span>
      </section>
    );
  }

  if (block.type === "paragraph") {
    return <p className="cc-rich-paragraph" dangerouslySetInnerHTML={markdownHtml(block.text, displayNormalization)} key={`paragraph-${index}`} />;
  }

  if (block.type === "heading") {
    return (
      <h3
        className={`cc-rich-heading cc-rich-heading--${Number(block.level) || 2}`}
        dangerouslySetInnerHTML={markdownHtml(block.text, displayNormalization)}
        key={`heading-${index}`}
      />
    );
  }

  if (block.type === "code") {
    const codeBlock = block as ConversationRichCodeBlock;
    return (
      <section className="cc-rich-code-card" key={`code-${index}`}>
        <div className="cc-rich-code-card__header">
          <span className="cc-rich-code-language">{codeBlock.language || "text"}</span>
          <CopyButton
            copiedLabel="Copied code"
            Icon={Icon}
            label="Copy code"
            text={codeBlock.body}
          />
        </div>
        <pre className="cc-rich-code-body">
          {codeBlock.highlightedHtml ? (
            <code
              className={`cc-rich-code-content language-${codeBlock.language || "text"}`}
              dangerouslySetInnerHTML={{ __html: codeBlock.highlightedHtml }}
            />
          ) : (
            <code className={`cc-rich-code-content language-${codeBlock.language || "text"}`}>{codeBlock.body}</code>
          )}
        </pre>
      </section>
    );
  }

  if (block.type === "table") {
    return (
      <div className="cc-rich-table-wrap" key={`table-${index}`}>
        <table className="cc-rich-table">
          <thead>
            <tr>
              {block.headers.map((header, cellIndex) => (
                <th
                  data-align={alignmentAttr(block.alignments[cellIndex])}
                  dangerouslySetInnerHTML={markdownHtml(header, displayNormalization)}
                  key={`header-${cellIndex}`}
                />
              ))}
            </tr>
          </thead>
          <tbody>
            {block.rows.map((row, rowIndex) => (
              <tr key={`row-${rowIndex}`}>
                {block.headers.map((_header, cellIndex) => (
                  <td
                    data-align={alignmentAttr(block.alignments[cellIndex])}
                    dangerouslySetInnerHTML={markdownHtml(row[cellIndex] || "", displayNormalization)}
                    key={`cell-${rowIndex}-${cellIndex}`}
                  />
                ))}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    );
  }

  if (block.type === "command") {
    return (
      <div className="cc-rich-command-stack" key={`command-${index}`}>
        <div className="cc-rich-command-caption">{block.caption}</div>
        <div className="cc-rich-command-card">
          <div className="cc-rich-command-card__header">
            <div className="cc-rich-command-card__title">{block.title}</div>
            <CopyButton
              copiedLabel="Copied command output"
              Icon={Icon}
              label="Copy command output"
              text={commandCopyText(block)}
            />
          </div>
          <pre className="cc-rich-command-card__body">{block.body}</pre>
          {block.output ? <pre className="cc-rich-command-card__output">{block.output}</pre> : null}
          {block.footer ? <div className="cc-rich-command-card__footer">{block.footer}</div> : null}
        </div>
      </div>
    );
  }

  if (block.type === "file-change") {
    return (
      <section className="cc-rich-file-change" key={`file-change-${index}`}>
        <div className="cc-rich-file-change__main">
          <span className="cc-rich-file-change__verb">{block.verb}</span>
          {block.before ? <span className="cc-rich-file-change__context" dangerouslySetInnerHTML={markdownHtml(block.before, displayNormalization)} /> : null}
          <button className="cc-rich-file-change__link" type="button">{block.name}</button>
          {block.after ? <span className="cc-rich-file-change__context" dangerouslySetInnerHTML={markdownHtml(block.after, displayNormalization)} /> : null}
        </div>
        <div className="cc-rich-file-change__stats">
          <ChangeStatPair minus={block.minus} plus={block.plus} />
          <span className="cc-rich-file-change__dot" />
          <CopyButton
            copiedLabel="Copied file change"
            Icon={Icon}
            label="Copy file change"
            text={fileChangeCopyText(block)}
          />
        </div>
      </section>
    );
  }

  if (block.type === "divider") {
    return (
      <div className="cc-rich-divider" key={`divider-${index}`}>
        <span className="cc-rich-divider__line" />
        <span className="cc-rich-divider__label">{block.text}</span>
        <span className="cc-rich-divider__line" />
      </div>
    );
  }

  if (block.type === "image") {
    const image = block as ConversationRichImageBlock;
    return (
      <button
        className="cc-rich-image-button"
        key={`image-${index}`}
        onClick={() => window.open(image.src, "_blank", "noopener,noreferrer")}
        type="button"
      >
        <img
          alt={image.alt || ""}
          className="cc-rich-image"
          height={image.height}
          loading="lazy"
          src={image.src}
          width={image.width}
        />
      </button>
    );
  }

  if (block.type === "tool-call") {
    return <ToolCallBlock block={block} key={`tool-call-${index}`} />;
  }

  return <div key={`thinking-${index}`}><ThinkingBlock block={block} index={index} displayNormalization={displayNormalization} /></div>;
}

const PEER_TOOL_NAMES = new Set(["send_request", "send_message", "send_response"]);

// `navigator.clipboard` exists ONLY in a secure context - https, or localhost.
// The console is routinely opened over plain http on a LAN address, and there
// this expression short-circuits the WHOLE optional chain: `.catch` never runs,
// nothing throws, and the caller's `setCopied(true)` executed unconditionally.
// The button showed a checkmark having copied nothing, which is worse than an
// error - the user pastes whatever was already on the clipboard.
//
// Removed in favour of the shared `copyTextToClipboard`, which falls back to
// `document.execCommand` and reports whether it actually worked.

/// Pretty-print JSON-shaped strings into a 2-space-indented form so
/// the expanded peer/tool body shows readable params instead of one
/// long line. Non-JSON strings pass through unchanged.
function formatJsonIfPossible(text: string): string {
  const trimmed = text.trim();
  if (!trimmed) return text;
  if (
    !((trimmed.startsWith("{") && trimmed.endsWith("}")) ||
      (trimmed.startsWith("[") && trimmed.endsWith("]")))
  ) {
    return text;
  }
  try {
    const parsed = JSON.parse(trimmed);
    return JSON.stringify(parsed, null, 2);
  } catch {
    return text;
  }
}

function toolBlockCopyText(block: ConversationRichToolCallBlock): string {
  if (block.peerTarget) {
    const dir = block.peerIncoming ? "← from" : "→ to";
    const peerBody = conversationRichPeerBodyForDisplay(block.peerBody, block.peerBodyFormat ?? "legacy");
    const result = meaningfulPeerResult(block.result);
    return [
      `${dir} ${block.peerIdentity || block.peerTarget || "Unknown peer"}`,
      conversationRichPeerIntentForDisplay(block.peerIntent, peerBody),
      peerBody,
      result,
    ].filter(Boolean).join(": ").trim();
  }
  const parts = [`$ ${block.name}`];
  if (block.arguments) parts.push(`Input: ${block.arguments}`);
  if (block.result) parts.push(`Result: ${block.result}`);
  return parts.join("\n");
}

function parseObjectJson(text: string | null | undefined): Record<string, unknown> | null {
  const trimmed = String(text || "").trim();
  if (!trimmed || !trimmed.startsWith("{") || !trimmed.endsWith("}")) {
    return null;
  }
  try {
    const parsed = JSON.parse(trimmed);
    return parsed && typeof parsed === "object" && !Array.isArray(parsed)
      ? parsed as Record<string, unknown>
      : null;
  } catch {
    return null;
  }
}

function textFromUnknown(value: unknown): string {
  if (value == null) {
    return "";
  }
  if (typeof value === "string") {
    return normalizeConversationDisplayText(value).trim();
  }
  return normalizeConversationDisplayText(JSON.stringify(value, null, 2));
}

function meaningfulPeerResult(value: string | null | undefined): string {
  const text = normalizeConversationDisplayText(String(value || "")).trim();
  if (!text || /^(completed|delivered|ok|success)$/i.test(text)) {
    return "";
  }
  return formatJsonIfPossible(text);
}

function peerDetailRows(block: ConversationRichToolCallBlock): Array<{ label: string; value: string }> {
  const args = parseObjectJson(block.arguments) || {};
  const peerBody = conversationRichPeerBodyForDisplay(block.peerBody, block.peerBodyFormat ?? "legacy");
  const peerIntent = conversationRichPeerIntentForDisplay(block.peerIntent, peerBody);
  const body = peerBody
    || textFromUnknown(args.body)
    || textFromUnknown(args.message)
    || textFromUnknown(args.content)
    || textFromUnknown(args.text);
  const params = textFromUnknown(args.params);
  const requestId = textFromUnknown(args.in_reply_to)
    || textFromUnknown(args.inReplyTo)
    || textFromUnknown(args.request_id)
    || textFromUnknown(args.requestId);
  const result = meaningfulPeerResult(block.result);
  const primaryLabel = block.name === "send_request"
    ? "Request"
    : block.name === "send_response"
      ? "Response"
      : "Message";
  return [
    body ? { label: primaryLabel, value: body } : null,
    peerIntent ? { label: "Intent", value: peerIntent } : null,
    params ? { label: "Params", value: params } : null,
    requestId ? { label: "Request ID", value: requestId } : null,
    result ? { label: "Result", value: result } : null,
  ].filter(Boolean) as Array<{ label: string; value: string }>;
}

function CopyBtn({ text, label = "Copy" }: { text: string; label?: string }) {
  const [outcome, setOutcome] = useState<"idle" | "copied" | "failed">("idle");
  // The reset timer must not outlive the component. An uncleared timer fires
  // into an unmounted tree, and React reads `window` to resolve update priority
  // before it discovers the update is a no-op - so on a torn-down DOM it throws
  // rather than quietly doing nothing.
  const resetTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(
    () => () => {
      if (resetTimer.current) clearTimeout(resetTimer.current);
    },
    [],
  );
  const title = outcome === "copied" ? "Copied" : outcome === "failed" ? "Copy failed" : label;
  return (
    <button
      className="cc-tool-call__copy"
      type="button"
      title={title}
      aria-label={title}
      data-copy-outcome={outcome === "idle" ? undefined : outcome}
      onClick={(e) => {
        e.stopPropagation();
        // Await the RESULT. The previous version marked success before the
        // copy could have happened, so the mark carried no information.
        void copyTextToClipboard(text).then((ok) => {
          setOutcome(ok ? "copied" : "failed");
          if (resetTimer.current) clearTimeout(resetTimer.current);
          resetTimer.current = setTimeout(() => setOutcome("idle"), 1500);
        });
      }}
    >
      <CopyGlyph state={outcome} />
    </button>
  );
}

function onToolHeaderKeyDown(event: KeyboardEvent<HTMLDivElement>, toggle: () => void) {
  if (event.key !== "Enter" && event.key !== " ") {
    return;
  }
  event.preventDefault();
  toggle();
}

function toolCompletionLabel(block: ConversationRichToolCallBlock): string {
  const outcome = block.completionEvidence?.outcome;
  if (outcome === "unknown") return "Completion unknown";
  if (outcome === "cancelled") return "Cancelled";
  if (outcome === "interrupted") return "Interrupted";
  return block.status === "pending" ? "Running" : block.status === "success" ? "Success" : "Failed";
}

function toolAttentionKey(block: ConversationRichToolCallBlock): string | null {
  const outcome = block.completionEvidence?.outcome ?? (block.status === "pending" ? "running" : block.status);
  return block.status === "error" || ["running", "error", "cancelled", "interrupted", "unknown"].includes(outcome)
    ? JSON.stringify([block.toolCallId, block.status, outcome]) : null;
}

/** New actionable information reopens details once; a later manual close holds. */
function useToolDisclosure(blocks: ConversationRichToolCallBlock[], initiallyOpen: boolean) {
  const keys = blocks.map(toolAttentionKey).filter((key): key is string => key !== null);
  const signature = JSON.stringify(keys);
  const [state, setState] = useRowState(`tool:${JSON.stringify(blocks.map((block) => block.toolCallId))}`, () => ({ signature, keys, expanded: initiallyOpen }));
  const expanded = state.expanded || (state.signature !== signature && keys.some((key) => !state.keys.includes(key)));
  if (state.signature !== signature) setState({ signature, keys, expanded });
  const toggle = () => setState({ signature, keys, expanded: !expanded });
  return { expanded, toggle };
}

function ToolCallBlock({
  block,
  className,
}: {
  block: ConversationRichToolCallBlock;
  className?: string;
}) {
  const insideDisclosure = useInsideCompletedToolDisclosure();
  const { expanded, toggle } = useToolDisclosure([block], insideDisclosure || block.status === "pending" || block.status === "error" || ["cancelled", "interrupted", "unknown"].includes(block.completionEvidence?.outcome ?? ""));
  const displayLabels = useConversationDisplayLabels();
  const isPeer = PEER_TOOL_NAMES.has(block.name);
  const statusIcon = block.status === "success" ? "✓" : block.status === "error" ? "✗" : "⋯";
  const statusClass = `cc-tool-call--${block.status}`;

  if (isPeer || block.peerIncoming) {
    const target = peerDisplayLabel(block, displayLabels?.peers);
    const peerBody = conversationRichPeerBodyForDisplay(block.peerBody, block.peerBodyFormat ?? "legacy");
    const peerIntent = conversationRichPeerIntentForDisplay(block.peerIntent, peerBody);
    const content = peerBody || peerIntent || "";
    const arrow = block.peerIncoming ? "↙" : "↗";
    const detailRows = peerDetailRows(block);
    return (
      <section data-quote-exclude className={clsx("cc-tool-call cc-tool-call--peer", block.peerIncoming && "cc-tool-call--incoming", statusClass, className)}>
        <div
          className="cc-tool-call__header"
          role="button"
          tabIndex={0}
          onClick={toggle}
          onKeyDown={(event) => onToolHeaderKeyDown(event, toggle)}
          aria-expanded={expanded}
        >
          <span className="cc-tool-call__chevron">{expanded ? "▾" : "▸"}</span>
          <span className="cc-tool-call__icon">{arrow}</span>
          <span className="cc-tool-call__name" title={block.peerIdentity || block.peerTarget}>{block.peerIncoming ? `Received from ${target}` : target}</span>
          <span className="cc-tool-call__peer-summary">
            {peerIntent && <span className="cc-tool-call__peer-intent">{peerIntent}</span>}
            {content && <span className="cc-tool-call__peer-body">{content}</span>}
          </span>
          <span className="cc-tool-call__status">{statusIcon}</span>
          <CopyBtn text={toolBlockCopyText(block)} />
        </div>
        <ToolCallImages images={block.peerImages} />
        {expanded && detailRows.length > 0 && (
          <div className="cc-tool-call__body">
            {detailRows.map((row) => (
              <div className="cc-tool-call__section" key={`${row.label}:${row.value}`}>
                <div className="cc-tool-call__section-label">{row.label}</div>
                <pre className="cc-tool-call__pre">{formatJsonIfPossible(row.value)}</pre>
              </div>
            ))}
          </div>
        )}
      </section>
    );
  }

  // Generic tool call
  let argsPreview = block.arguments || "";
  try {
    const parsed = JSON.parse(argsPreview);
    if (typeof parsed === "object" && parsed !== null) {
      argsPreview = Object.entries(parsed)
        .map(([k, v]) => `${k}: ${typeof v === "string" ? v : JSON.stringify(v)}`)
        .join(", ");
    }
  } catch { /* use raw */ }

  return (
    <section data-quote-exclude className={clsx("cc-tool-call", statusClass, className)}>
      <div
        className="cc-tool-call__header"
        role="button"
        tabIndex={0}
        onClick={toggle}
        onKeyDown={(event) => onToolHeaderKeyDown(event, toggle)}
        aria-expanded={expanded}
      >
        <span className="cc-tool-call__chevron">{expanded ? "▾" : "▸"}</span>
        <span className="cc-tool-call__icon">⚙</span>
        <span className="cc-tool-call__name" title={block.name}>{explicitDisplayLabel(block.name, displayLabels?.tools)}</span>
        {argsPreview && <span className="cc-tool-call__preview">{argsPreview}</span>}
        <span className="cc-tool-call__status">{statusIcon} {toolCompletionLabel(block)}</span>
        <CopyBtn text={toolBlockCopyText(block)} />
      </div>
      <ToolCallImages images={block.images} />
      {expanded && (
        <div className="cc-tool-call__body">
          {argsPreview && (
            <div className="cc-tool-call__section">
              <div className="cc-tool-call__section-label">Input</div>
              <pre className="cc-tool-call__pre">{block.arguments}</pre>
            </div>
          )}
          {block.result && (
            <div className="cc-tool-call__section">
              <div className="cc-tool-call__section-label">Result</div>
              <pre className="cc-tool-call__pre">{block.result}</pre>
            </div>
          )}
        </div>
      )}
    </section>
  );
}

/// Images attached to a tool call: a peer message's images or the images a
/// tool returned in its result. Each opens full size in a new tab.
function ToolCallImages({ images }: { images?: ConversationRichImageBlock[] }) {
  if (!images || images.length === 0) return null;
  return (
    <div className="cc-tool-call__attachments">
      {images.map((image, index) => (
        <ToolCallImage image={image} key={`${image.blobId || image.imageId || image.src}-${index}`} />
      ))}
    </div>
  );
}

function ToolCallImage({ image }: { image: ConversationRichImageBlock }) {
  const shown = useConversationImageSource(image);
  if (shown.unavailable) {
    return <span className="cc-tool-call__image-unavailable">Image unavailable</span>;
  }
  const src = shown.src;
  if (!src) return null;
  return (
    <button
      className="cc-tool-call__image-button"
      onClick={() => window.open(src, "_blank", "noopener,noreferrer")}
      type="button"
    >
      <img
        alt={image.alt || ""}
        className="cc-tool-call__image"
        height={image.height}
        loading="lazy"
        src={src}
        width={image.width}
      />
    </button>
  );
}

/// Group of N tool calls of the same `name` that aren't peer tools.
/// Renders one collapsible card with `<icon> <name> ×N <status>` in
/// the header; expanded body lists each call's input + result. The
/// status icon is composite — success only when every call succeeded.
function ToolCallGroup({ blocks }: { blocks: ConversationRichToolCallBlock[] }) {
  const insideDisclosure = useInsideCompletedToolDisclosure();
  const { expanded, toggle } = useToolDisclosure(blocks, insideDisclosure || !canFoldCompletedTools(blocks));
  const displayLabels = useConversationDisplayLabels();
  const allSuccess = blocks.every((b) => b.status === "success");
  const anyError = blocks.some((b) => b.status === "error");
  const statusIcon = anyError ? "✗" : allSuccess ? "✓" : "⋯";
  const statusLabel = anyError ? "Failed" : blocks.some((block) => block.completionEvidence?.outcome === "interrupted") ? "Interrupted" : blocks.some((block) => block.completionEvidence?.outcome === "cancelled") ? "Cancelled" : blocks.some((block) => block.completionEvidence?.outcome === "unknown") ? "Completion unknown" : allSuccess ? "Success" : "Running";
  const statusClass = anyError
    ? "cc-tool-call--error"
    : allSuccess
      ? "cc-tool-call--success"
      : "cc-tool-call--pending";
  const name = blocks[0]?.name || "tool";

  return (
    <section data-quote-exclude className={clsx("cc-tool-call cc-tool-call--group", statusClass)}>
      <div
        className="cc-tool-call__header"
        role="button"
        tabIndex={0}
        onClick={toggle}
        onKeyDown={(event) => onToolHeaderKeyDown(event, toggle)}
        aria-expanded={expanded}
      >
        <span className="cc-tool-call__chevron">{expanded ? "▾" : "▸"}</span>
        <span className="cc-tool-call__icon">⚙</span>
        <span className="cc-tool-call__name" title={name}>{explicitDisplayLabel(name, displayLabels?.tools)}</span>
        <span className="cc-tool-call__count">×{blocks.length}</span>
        <span className="cc-tool-call__status">{statusIcon} {statusLabel}</span>
        <CopyBtn text={blocks.map((b) => toolBlockCopyText(b)).join("\n")} />
      </div>
      {expanded && (
        <div className="cc-tool-call__body">
          {blocks.map((block, i) => {
            const args = block.arguments
              ? formatJsonIfPossible(block.arguments)
              : "";
            const result = block.result
              ? formatJsonIfPossible(block.result)
              : "";
            const completionLabel = toolCompletionLabel(block);
            const showCompletionLabel = ["unknown", "cancelled", "interrupted"].includes(block.completionEvidence?.outcome ?? "");
            return (
              <div className="cc-tool-call__sub" key={block.toolCallId || i}>
                <div className="cc-tool-call__sub-head">
                  <span className="cc-tool-call__sub-index">#{i + 1}</span>
                  <span className={clsx("cc-tool-call__peer-status", `cc-tool-call__peer-status--${block.status}`, showCompletionLabel && "cc-tool-call__peer-status--explicit")}
                    role="status" aria-label={completionLabel} title={completionLabel}>
                    {showCompletionLabel ? completionLabel : <span aria-hidden="true">{block.status === "success" ? "✓" : block.status === "error" ? "✗" : "⋯"}</span>}
                  </span>
                </div>
                {args && (
                  <div className="cc-tool-call__section">
                    <div className="cc-tool-call__section-label">Input</div>
                    <pre className="cc-tool-call__pre">{args}</pre>
                  </div>
                )}
                {result && (
                  <div className="cc-tool-call__section">
                    <div className="cc-tool-call__section-label">Result</div>
                    <pre className="cc-tool-call__pre">{result}</pre>
                  </div>
                )}
                <ToolCallImages images={block.images} />
              </div>
            );
          })}
        </div>
      )}
    </section>
  );
}

function PeerToolGroup({ blocks }: { blocks: ConversationRichToolCallBlock[] }) {
  const { expanded, toggle } = useToolDisclosure(blocks, true);
  const displayLabels = useConversationDisplayLabels();
  const peers = new Map<string, ConversationRichToolCallBlock>();
  for (const block of blocks) {
    const identity = block.peerIdentity || block.peerTarget || "Unknown peer";
    if (!peers.has(identity)) peers.set(identity, block);
  }
  const targets = Array.from(peers.values(), (block) => peerDisplayLabel(block, displayLabels?.peers));
  const allSuccess = blocks.every((b) => b.status === "success");
  const anyError = blocks.some((b) => b.status === "error");
  const statusIcon = anyError ? "✗" : allSuccess ? "✓" : "⋯";
  const statusClass = anyError ? "cc-tool-call--error" : allSuccess ? "cc-tool-call--success" : "cc-tool-call--pending";
  const isIncoming = blocks[0]?.peerIncoming;
  const arrow = isIncoming ? "↙" : "↗";
  const label = isIncoming
    ? `Received from ${targets.join(", ")}`
    : `Sent to ${targets.join(", ")}`;

  return (
    <section data-quote-exclude className={clsx("cc-tool-call cc-tool-call--peer-group", isIncoming && "cc-tool-call--incoming", statusClass)}>
      <div
        className="cc-tool-call__header"
        role="button"
        tabIndex={0}
        onClick={toggle}
        onKeyDown={(event) => onToolHeaderKeyDown(event, toggle)}
        aria-expanded={expanded}
      >
        <span className="cc-tool-call__chevron">{expanded ? "▾" : "▸"}</span>
        <span className="cc-tool-call__icon">{arrow}</span>
        <span className="cc-tool-call__name" title={Array.from(peers.keys()).join(", ")}>{label}</span>
        <span className="cc-tool-call__status">{statusIcon}</span>
        <CopyBtn text={blocks.map((b) => toolBlockCopyText(b)).join("\n")} />
      </div>
      {expanded && (
        <div className="cc-tool-call__body">
          {blocks.map((block, i) => {
            const peerBody = conversationRichPeerBodyForDisplay(block.peerBody, block.peerBodyFormat ?? "legacy");
            const peerIntent = conversationRichPeerIntentForDisplay(block.peerIntent, peerBody);
            return (
              <div className="cc-tool-call__peer-row" key={block.toolCallId || i}>
                <span className="cc-tool-call__peer-target" title={block.peerIdentity || block.peerTarget}>{isIncoming ? "←" : "→"} {peerDisplayLabel(block, displayLabels?.peers)}</span>
                {peerIntent ? (
                  <span className="cc-tool-call__peer-intent">
                    {peerIntent}
                  </span>
                ) : null}
                {peerBody && (
                  <span className="cc-tool-call__peer-body">{peerBody}</span>
                )}
                {block.result && toolAttentionKey(block) && (
                  <div className="cc-tool-call__peer-result">
                    <div className="cc-tool-call__section-label">{toolCompletionLabel(block)}</div>
                    <pre className="cc-tool-call__pre">{block.result}</pre>
                  </div>
                )}
                <span className={`cc-tool-call__peer-status cc-tool-call__peer-status--${block.status}`}>
                  {block.status === "success" ? "✓" : block.status === "error" ? "✗" : "⋯"}
                </span>
              </div>
            );
          })}
        </div>
      )}
    </section>
  );
}

export function ConversationRichContent({
  blocks,
  markdownUrlPolicy,
  richStyle = "default",
  Icon,
  displayNormalization = true,
}: ConversationRichContentProps) {
  const insideDisclosure = useInsideCompletedToolDisclosure();
  if (!insideDisclosure && canFoldCompletedTools(blocks)) {
    return <CompletedToolDisclosure blocks={blocks}><ConversationRichContent blocks={blocks} markdownUrlPolicy={markdownUrlPolicy} richStyle={richStyle} Icon={Icon} displayNormalization={displayNormalization} /></CompletedToolDisclosure>;
  }
  // Render multi-block tool runs as a single collapsible group:
  // peer tools get the `Sent to a, b, c` blob, generic same-name
  // tool runs get the `<name> ×N` blob. The adapter (and ChatPane's
  // defensive merge) only puts blocks in the same array when they
  // share a `name` and, for peer tools, the same direction — so
  // detection here is just a "are they all tool-call and same-name"
  // check.
  if (blocks.length > 1 && blocks.every((b) => b.type === "tool-call") && (insideDisclosure || !groupRoutineToolRows(blocks, (block) => [block]).some((run) => run.tools.length >= 2))) {
    const tools = blocks as ConversationRichToolCallBlock[];
    const firstName = tools[0].name;
    if (tools.every((b) => b.name === firstName)) {
      const allPeer = tools.every((b) => PEER_TOOL_NAMES.has(b.name) || b.peerIncoming);
      if (allPeer) {
        return <PeerToolGroup blocks={tools} />;
      }
      return <ToolCallGroup blocks={tools} />;
    }
  }

  // Text siblings keep their element type and source key when nearby tools
  // become foldable. A recursive wrapper around every run would remount them.
  const body = groupRoutineToolRows(blocks, (block) => [block]).flatMap((run) => {
    if (!insideDisclosure && run.tools.length >= 2) return [
      <CompletedToolDisclosure key={`completed:${run.tools[0].toolCallId}`} blocks={run.tools}><ConversationRichContent blocks={run.rows} markdownUrlPolicy={markdownUrlPolicy} richStyle={richStyle} Icon={Icon} displayNormalization={displayNormalization} /></CompletedToolDisclosure>,
    ];
    return run.rows.map((block) => renderBlock(block, blocks.indexOf(block), Icon, displayNormalization, markdownUrlPolicy));
  }).filter((element): element is ReactElement<{ className?: string }> => element !== null);

  if (body.length === 0) {
    return null;
  }

  // Keep every rendered block at the same React depth when a streamed response
  // becomes final. A streaming-only wrapper forces React to replace the first
  // block (usually the live paragraph), while a permanent wrapper breaks host
  // selectors that intentionally target direct rich-message children.
  const renderedBody = richStyle === "streaming"
    ? body.map((element) => cloneElement(element, {
        className: clsx(element.props.className, "cc-rich-streaming"),
      }))
    : body;

  return <>{renderedBody}</>;
}
