import { QuoteSelectionAction } from "./quote-selection-action";
import { JumpToLatest } from "./jump-to-latest";
import { ConversationApprovals, type ConversationApprovalProps } from "./conversation-approvals";
import type { ConsoleQuoteSelection } from "./context-selection";
import { ConversationHeader, ConversationPresentationProvider, type ConversationDisplayLabels, type ConversationHeaderProps } from "./presentation-policy";
import clsx from "clsx";
import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
  type CSSProperties,
} from "react";

import type { ConversationViewState } from "@console-core";

import { useConversationScrollController, type ConversationScrollControllerOptions, type ConversationViewportKey } from "./scroll-controller";
import { ConversationEmptyState } from "./conversation-empty-state";
import { ConversationTranscript } from "./conversation-transcript";
import { ConversationBlobReaderProvider, type ConversationBlobReader } from "./blob-image";
import type { MarkdownUrlPolicy } from "./conversation-markdown";
import type { FlowRunRestoreHandler } from "./flow-run-card";
import type { WorkGraphCardActions } from "./work-graph-card";
import {
  conversationTurnPreview,
  groupConversationTranscriptTurns,
} from "./conversation-turns";
import type { IconRenderer } from "../shared";

export type ConversationPaneProps = ConversationApprovalProps & {
  viewState: ConversationViewState;
  viewportKey?: ConversationViewportKey;
  submittedRowId?: string | null;
  onRevealAnchor?: ConversationScrollControllerOptions["revealAnchor"];
  markdownUrlPolicy?: MarkdownUrlPolicy;
  // Reads stored images with the host transport's authority (its `readBlob`).
  // Without it, stored images keep their direct URL. Pass a stable function:
  // a new reader restarts every stored image's read.
  blobReader?: ConversationBlobReader;
  displayLabels?: ConversationDisplayLabels;
  header?: ConversationHeaderProps;
  Icon?: IconRenderer | null;
  footer?: ReactNode;
  contextSlot?: ReactNode;
  onQuoteSelection?: (quote: ConsoleQuoteSelection) => void;
  isWorking?: boolean;
  scrollTail?: ReactNode;
  className?: string;
  scrollClassName?: string;
  bodyClassName?: string;
  compact?: boolean;
  maxGroups?: number | null;
  showTurnDiff?: boolean;
  expandedDiffFile?: string | null;
  onApplySuggestion?: (value: string) => void;
  onToggleDiffFile?: ((filePath: string) => void) | null;
  onFlowRunMessageMember?: ((memberKey: string) => void) | null;
  onFlowRunRestore?: FlowRunRestoreHandler | null;
  workGraphActions?: WorkGraphCardActions | null;
  // The jump-to-turn rail on the pane's edge. Default true (the Meta
  // console ships it); meerkat-studio opts out until it adopts the rail
  // deliberately.
  showTurnRail?: boolean;
};

function visibleTranscriptGroups(viewState: ConversationViewState, maxGroups: number | null) {
  return typeof maxGroups === "number" && maxGroups > 0
    ? viewState.groups.slice(-maxGroups)
    : viewState.groups;
}

export function ConversationPane({
  viewState,
  approvalSnapshot,
  approvalIdentity,
  onApprovalDecision,
  contextSlot,
  onQuoteSelection,
  isWorking,
  viewportKey,
  submittedRowId,
  onRevealAnchor,
  markdownUrlPolicy,
  blobReader,
  displayLabels,
  header,
  Icon,
  footer = null,
  scrollTail = null,
  className,
  scrollClassName,
  bodyClassName,
  compact = false,
  maxGroups = null,
  showTurnDiff = true,
  expandedDiffFile = null,
  onApplySuggestion,
  onToggleDiffFile = null,
  onFlowRunMessageMember = null,
  onFlowRunRestore = null,
  workGraphActions = null,
  showTurnRail: showTurnRailProp = true,
}: ConversationPaneProps) {
  const [quoteError, setQuoteError] = useState<string | null>(null);
  useEffect(() => { setQuoteError(null); }, [submittedRowId, viewState.conversationId, viewportKey?.authority, viewportKey?.identity, viewportKey?.conversation, viewportKey?.pane]);
  const scrollRef = useRef<HTMLElement | null>(null);
  const contentRef = useRef<HTMLDivElement | null>(null);
  const headerRef = useRef<HTMLDivElement | null>(null);
  const footerRef = useRef<HTMLDivElement | null>(null);
  const railRef = useRef<HTMLElement | null>(null);
  const [railHeight, setRailHeight] = useState<number | null>(null);
  const [railPage, setRailPage] = useState<{ conversationId: string; firstTurnId: string } | null>(null);
  const [railInsets, setRailInsets] = useState({ top: 0, bottom: 0 });
  const scroll = useConversationScrollController({
    viewportRef: scrollRef, contentRef, viewportKey,
    conversationId: viewState.conversationId, contentVersion: viewState, submittedRowId,
    resolveSubmittedRowId: sourceId => {
      const entry = viewState.entries.find(entry => entry.id === sourceId || entry.renderKey === sourceId);
      return entry ? entry.renderKey ?? entry.id : null;
    },
    revealAnchor: onRevealAnchor,
  });
  const [visibleTurnIndexes, setVisibleTurnIndexes] = useState<number[]>([]);
  const canRenderTurnDiff = Boolean(showTurnDiff && viewState.turnDiff && onToggleDiffFile);
  const showEmptyState = Boolean(viewState.emptyState && viewState.entries.length === 0 && !canRenderTurnDiff);
  const visibleTurns = useMemo(
    () => groupConversationTranscriptTurns(visibleTranscriptGroups(viewState, maxGroups)),
    [maxGroups, viewState],
  );
  const railTurns = visibleTurns.length
    ? visibleTurns
    : canRenderTurnDiff
      ? [{ id: "turn-diff", groups: [] }]
      : [];
  const showTurnRail = showTurnRailProp && !showEmptyState && railTurns.length > 1;
  // Bound the rail to its measured band. Page controls keep every source turn
  // individually reachable, while stable turn IDs survive history prepends.
  const railSlots = railHeight === null || railHeight <= 0
    ? 48 : Math.max(3, Math.min(48, Math.floor((railHeight - 8) / 10) - 1));
  const railCapacity = Math.max(1, railSlots - 2);
  const requestedRailStart = railPage?.conversationId === viewState.conversationId
    ? railTurns.findIndex(turn => turn.id === railPage.firstTurnId) : -1;
  const railStart = railTurns.length <= railSlots ? 0
    : Math.min(Math.max(0, railTurns.length - railCapacity), requestedRailStart < 0 ? railTurns.length - railCapacity : requestedRailStart);
  const railEnd = railTurns.length <= railSlots ? railTurns.length : Math.min(railTurns.length, railStart + railCapacity);
  const pageRail = (start: number) => {
    const turn = railTurns[Math.max(0, Math.min(start, railTurns.length - railCapacity))];
    if (turn) setRailPage({ conversationId: viewState.conversationId, firstTurnId: turn.id });
  };

  useEffect(() => { setRailPage(null); }, [viewState.conversationId, viewportKey?.authority, viewportKey?.identity, viewportKey?.conversation, viewportKey?.pane]);
  useEffect(() => {
    const rail = railRef.current;
    if (!rail || !showTurnRail) return;
    const measure = () => setRailHeight(rail.getBoundingClientRect().height);
    measure();
    const observer = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(entries => {
      const entry = entries.find(item => item.target === rail);
      if (entry) setRailHeight(entry.contentRect.height);
    });
    observer?.observe(rail);
    window.addEventListener("resize", measure);
    return () => { observer?.disconnect(); window.removeEventListener("resize", measure); };
  }, [showTurnRail]);

  useEffect(() => {
    const scrollNode = scrollRef.current;
    if (!scrollNode || railTurns.length <= 1) {
      setVisibleTurnIndexes([]);
      return;
    }

    let frame = 0;
    const updateActiveTurn = () => {
      frame = 0;
      const turnNodes = Array.from(
        scrollNode.querySelectorAll<HTMLElement>("[data-cc-conversation-turn-index]"),
      );
      if (!turnNodes.length) {
        setVisibleTurnIndexes([]);
        return;
      }

      const scrollRect = scrollNode.getBoundingClientRect();
      const visibleTop = scrollRect.top;
      const visibleBottom = scrollRect.bottom;
      const targetY = scrollRect.top + Math.min(128, Math.max(48, scrollRect.height * 0.24));
      let nextIndex = 0;
      const nextVisibleIndexes: number[] = [];

      for (const turnNode of turnNodes) {
        const rawIndex = Number(turnNode.dataset.ccConversationTurnIndex);
        if (!Number.isFinite(rawIndex)) {
          continue;
        }
        const turnRect = turnNode.getBoundingClientRect();
        if (turnRect.bottom >= visibleTop && turnRect.top <= visibleBottom) {
          nextVisibleIndexes.push(rawIndex);
        }
        if (turnRect.top <= targetY) {
          nextIndex = rawIndex;
        }
      }

      const nextIndexes = nextVisibleIndexes.length > 0 ? nextVisibleIndexes : [nextIndex];
      setVisibleTurnIndexes((current) => {
        if (current.length === nextIndexes.length && current.every((value, index) => value === nextIndexes[index])) {
          return current;
        }
        return nextIndexes;
      });
    };

    const scheduleUpdate = () => {
      if (frame) {
        return;
      }
      frame = window.requestAnimationFrame(updateActiveTurn);
    };

    updateActiveTurn();
    scrollNode.addEventListener("scroll", scheduleUpdate, { passive: true });
    window.addEventListener("resize", scheduleUpdate);

    const Observer = window.ResizeObserver;
    const resizeObserver = Observer ? new Observer(scheduleUpdate) : null;
    resizeObserver?.observe(scrollNode);

    return () => {
      if (frame) {
        window.cancelAnimationFrame(frame);
      }
      scrollNode.removeEventListener("scroll", scheduleUpdate);
      window.removeEventListener("resize", scheduleUpdate);
      resizeObserver?.disconnect();
    };
  }, [railTurns.length]);

  useEffect(() => {
    const measure = () => setRailInsets((previous) => {
      const next = { top: headerRef.current?.getBoundingClientRect().height ?? 0, bottom: footerRef.current?.getBoundingClientRect().height ?? 0 };
      return next.top === previous.top && next.bottom === previous.bottom ? previous : next;
    });
    measure();
    const observer = typeof ResizeObserver !== "undefined" ? new ResizeObserver(measure) : null;
    if (headerRef.current) observer?.observe(headerRef.current);
    if (footerRef.current) observer?.observe(footerRef.current);
    return () => observer?.disconnect();
  }, [header, footer, contextSlot, onQuoteSelection, scroll.awayFromEnd, scroll.missingAnchor, scroll.revealingAnchor]);

  function scrollToTurn(turnIndex: number) {
    const entry = visibleTurns[turnIndex]?.groups[0]?.entries[0];
    const rowId = entry?.renderKey ?? entry?.id;
    if (rowId) scroll.jumpToRow(rowId);
  }

  return (
    <ConversationPresentationProvider labels={displayLabels} viewportKey={viewportKey} autoFold={scroll.mode === "following-end"}>
    <ConversationBlobReaderProvider reader={blobReader}>
    <div className={clsx("cc-theme-scope", "cc-conversation-pane", header && "cc-conversation-pane--header", className)} style={{ "--cc-conversation-header-height": `${railInsets.top}px`, "--cc-conversation-footer-height": `${railInsets.bottom}px` } as CSSProperties}>
      {header ? <div ref={headerRef}><ConversationHeader {...header} /></div> : null}
      {showTurnRail ? (
        <nav ref={railRef} className="cc-conversation-turn-rail" aria-label="Conversation turns">
          <ol className="cc-conversation-turn-rail__list">
            {railStart > 0 ? <li className="cc-conversation-turn-rail__item">
              <button type="button" className="cc-conversation-turn-rail__button cc-conversation-turn-rail__page"
                aria-label="Show earlier turns" title="Show earlier turns" onClick={() => pageRail(railStart - railCapacity)}>
                <svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true"><path d="m4 10 4-4 4 4" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" /></svg>
              </button>
            </li> : null}
            {railTurns.slice(railStart, railEnd).map((turn, railIndex) => {
              const turnIndex = railStart + railIndex;
              const isLastVisibleTurn = turnIndex === visibleTurns.length - 1;
              const isVisibleTurn = visibleTurnIndexes.includes(turnIndex);
              const preview = visibleTurns[turnIndex]
                ? conversationTurnPreview(
                    visibleTurns[turnIndex],
                    isLastVisibleTurn && showTurnDiff ? viewState.turnDiff : null,
                  )
                : null;
              return (
                <li className="cc-conversation-turn-rail__item" key={turn.id || `turn-${turnIndex}`}>
                  <button
                    aria-current={isVisibleTurn ? "true" : undefined}
                    aria-label={preview ? `Jump to turn ${turnIndex + 1}: ${preview.title}` : `Jump to turn ${turnIndex + 1}`}
                    className={clsx(
                      "cc-conversation-turn-rail__button",
                      isVisibleTurn && "is-active",
                    )}
                    data-testid={`conversation-turn-rail:${turnIndex}`}
                    onClick={(event) => {
                      scrollToTurn(turnIndex);
                      if (event.detail > 0) {
                        event.currentTarget.blur();
                      }
                    }}
                    type="button"
                  >
                    <span className="cc-conversation-turn-rail__tick" aria-hidden="true" />
                  </button>
                  {preview ? (
                    <div className="cc-conversation-turn-preview" role="presentation">
                      <div className="cc-conversation-turn-preview__title">{preview.title}</div>
                      <div className="cc-conversation-turn-preview__body">{preview.body}</div>
                      {preview.files.length || preview.hiddenFileCount ? (
                        <div className="cc-conversation-turn-preview__files">
                          {preview.files.map((file) => (
                            <span className="cc-conversation-turn-preview__file" key={file.name}>
                              {file.iconName && Icon ? (
                                <Icon className="cc-conversation-turn-preview__file-icon" name={file.iconName} />
                              ) : (
                                <span className="cc-conversation-turn-preview__file-icon" aria-hidden="true" />
                              )}
                              <span className="cc-conversation-turn-preview__file-name">{file.name}</span>
                            </span>
                          ))}
                          {preview.hiddenFileCount ? (
                            <span className="cc-conversation-turn-preview__file cc-conversation-turn-preview__file--more">
                              +{preview.hiddenFileCount}
                            </span>
                          ) : null}
                        </div>
                      ) : null}
                    </div>
                  ) : null}
                </li>
              );
            })}
            {railEnd < railTurns.length ? <li className="cc-conversation-turn-rail__item">
              <button type="button" className="cc-conversation-turn-rail__button cc-conversation-turn-rail__page"
                aria-label="Show later turns" title="Show later turns" onClick={() => pageRail(railStart + railCapacity)}>
                <svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true"><path d="m4 6 4 4 4-4" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" /></svg>
              </button>
            </li> : null}
          </ol>
        </nav>
      ) : null}
      <section ref={scrollRef} tabIndex={0} aria-label="Conversation transcript" className={clsx("cc-conversation-pane__scroll", scrollClassName)}>
        <div ref={contentRef} className={clsx("cc-conversation-pane__body", bodyClassName)}>
          {showEmptyState && viewState.emptyState ? (
            <ConversationEmptyState Icon={Icon} onApplySuggestion={onApplySuggestion} state={viewState.emptyState} />
          ) : (
            <ConversationTranscript
              Icon={Icon}
              markdownUrlPolicy={markdownUrlPolicy}
              compact={compact}
              expandedDiffFile={expandedDiffFile}
              maxGroups={maxGroups}
              onFlowRunMessageMember={onFlowRunMessageMember}
              onFlowRunRestore={onFlowRunRestore}
              workGraphActions={workGraphActions}
              onToggleDiffFile={onToggleDiffFile}
              showTurnDiff={showTurnDiff}
              viewState={viewState}
              approvalSnapshot={approvalSnapshot}
              approvalIdentity={approvalIdentity ?? viewportKey?.identity}
              onApprovalDecision={onApprovalDecision}
            />
          )}
          {showEmptyState ? <ConversationApprovals approvalSnapshot={approvalSnapshot} approvalIdentity={approvalIdentity ?? viewportKey?.identity} onApprovalDecision={onApprovalDecision} conversationId={viewState.conversationId} /> : null}
          {scrollTail}
        </div>
      </section>
      {onQuoteSelection ? <QuoteSelectionAction key={`${viewState.conversationId}:${viewportKey?.authority ?? ""}:${viewportKey?.identity ?? ""}`} viewportRef={scrollRef} onQuote={onQuoteSelection} onError={setQuoteError} /> : null}
      {scroll.awayFromEnd ? <JumpToLatest onClick={scroll.jumpToLatest} working={isWorking ?? viewState.entries.some((entry) => entry.kind === "message" && entry.richStyle === "streaming")} /> : null}
      {scroll.missingAnchor || scroll.revealingAnchor || footer || contextSlot || quoteError ? (
        <div ref={footerRef} className="cc-conversation-pane__footer" onInputCapture={() => { if (quoteError) setQuoteError(null); }}>
          {scroll.revealingAnchor ? <div role="status">Restoring earlier position...</div> : null}
          {scroll.missingAnchor ? <div role="status">Earlier position is unavailable. Load older history to see more.</div> : null}
          {quoteError ? <p role="alert">{quoteError}</p> : null}
          {contextSlot}
          {footer}
        </div>
      ) : null}
    </div>
    </ConversationBlobReaderProvider>
    </ConversationPresentationProvider>
  );
}
