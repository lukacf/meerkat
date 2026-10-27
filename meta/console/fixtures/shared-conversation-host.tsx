import { conversationPresentationRows } from "../../packages/console-core/src/assistant-presentation";
import { useEffect, useMemo, useState } from "react";
import { createRoot } from "react-dom/client";
import {
  buildConversationViewState, createHttpConsoleTransport, createMetaConsoleController,
  mapFramesToTimelineEntries, resolvePanelResponsePhase, type ConsoleFrame, type ConsoleTransportState,
  createPendingApprovalResource, createConsoleContextRecord,
  serializeConsoleContextMessage, type ConsoleContextRecord, type PendingApprovalSnapshot,
} from "@console-core";
import { callConsoleRpc } from "../../packages/console-core/src/network";
import { editConsoleContextQuote } from "../../packages/console-core/src/context-edit";
import { ConversationPane, ConsoleTransportStatus, ApprovalCard, QuoteContextChips } from "@console-components";
import "@console-components/styles";
import "./shared-conversation-host.css";
import { acceptanceMarkdownUrlPolicy } from "./markdown-url-policy";

const markdownUrlPolicy = acceptanceMarkdownUrlPolicy();
const switchRuntimeAuthority = new URLSearchParams(location.search).get("runtime-scope-switch") === "1";

function createHistoryLoad() {
  let complete!: (available: boolean) => void;
  const ready = new Promise<boolean>(resolve => { complete = resolve; });
  return { ready, complete, frames: [] as ConsoleFrame[], loadOlder: null as (() => Promise<boolean>) | null };
}

function awaitHistory(work: Promise<boolean>, signal: AbortSignal): Promise<boolean> {
  if (signal.aborted) return Promise.resolve(false);
  return new Promise(resolve => {
    const finish = (available: boolean) => { signal.removeEventListener("abort", cancel); resolve(available); };
    const cancel = () => finish(false);
    signal.addEventListener("abort", cancel, { once: true });
    work.then(finish, () => finish(false));
  });
}

function SharedHost() {
  const [identity, setIdentity] = useState("router:main");
  const [scope, setScope] = useState("fixture-principal-a");
  // Opt-in acceptance proxy prefixes select isolated real runtime owners.
  // Ordinary shared-host tests retain their existing same-runtime scope switch.
  const baseUrl = switchRuntimeAuthority
    ? `${location.origin}/${scope === "fixture-principal-a" ? "scope-a" : "scope-b"}`
    : location.origin;
  const [history, setHistory] = useState({ available: false, loading: false });
  const [draft, setDraft] = useState("");
  const [error, setError] = useState("");
  const [submitted, setSubmitted] = useState<string | null>(null);
  const [revision, setRevision] = useState(0);
  const historyLoad = useMemo(createHistoryLoad, [identity, scope, revision]);
  const [timeline, setTimeline] = useState({ load: historyLoad, frames: historyLoad.frames });
  // A changed viewport identity must never inspect the previous load's rows,
  // including the render before its subscription effect has started.
  const frames = timeline.load === historyLoad ? timeline.frames : historyLoad.frames;
  const [twoPanes, setTwoPanes] = useState(false);
  const [showInbox, setShowInbox] = useState(false);
  const [contexts, setContexts] = useState<ConsoleContextRecord[]>([]);
  const [approvals, setApprovals] = useState<PendingApprovalSnapshot>();
  const [transportState, setTransportState] = useState<ConsoleTransportState>({ phase: "connecting", stale: true, freshness: "unknown" });
  const controller = useMemo(() => createMetaConsoleController({ transport: createHttpConsoleTransport({ baseUrl }) }), [baseUrl]);
  const approvalResource = useMemo(() => createPendingApprovalResource({
    scopeKey: scope,
    load: signal => callConsoleRpc(baseUrl, "meta/gating/pending", {}, undefined, signal),
    decide: (pendingId, decision, signal) => callConsoleRpc(baseUrl, "meta/gating/decide", {
      pending_id: pendingId, decision, approver_id: "acceptance-operator", reason: "Reviewed evidence",
    }, undefined, signal),
  }), [scope, baseUrl]);
  useEffect(() => {
    const publish = () => setApprovals(approvalResource.getSnapshot());
    const unsubscribe = approvalResource.subscribe(publish); publish();
    return () => { unsubscribe(); approvalResource.dispose(); };
  }, [approvalResource]);
  const activeApprovals = approvals?.scopeKey === scope ? approvals : undefined;

  useEffect(() => {
    const abort = new AbortController(); let unsubscribe: (() => void) | undefined;
    let oldestCursor: string | undefined;
    let exhausted = true;
    let loading: Promise<boolean> | null = null;
    let seeded = false;
    const publishFrames = (update: (current: ConsoleFrame[]) => ConsoleFrame[]) => {
      if (abort.signal.aborted) return;
      historyLoad.frames = update(historyLoad.frames);
      setTimeline({ load: historyLoad, frames: historyLoad.frames });
    };
    // Keep page metadata from the same authorized query used by the live
    // controller, without issuing a duplicate initial history request.
    const scopedController = createMetaConsoleController({ transport: {
      ...controller.transport,
      async queryTimeline(input) {
        const page = await controller.transport.queryTimeline(input);
        if (!abort.signal.aborted && page.available !== false && (!oldestCursor || input.before)) {
          oldestCursor = page.frames[0]?.cursor ?? oldestCursor;
          exhausted = page.exhausted === true;
          if (!input.before) seeded = true;
          setHistory({ available: !!oldestCursor && !exhausted, loading: loading !== null });
        }
        return page;
      },
    } });
    historyLoad.loadOlder = () => {
      if (loading) return loading;
      if (exhausted || !oldestCursor || abort.signal.aborted) return Promise.resolve(false);
      const before = oldestCursor;
      setHistory({ available: true, loading: true });
      loading = (async () => {
        try {
          const { value: page } = await scopedController.timeline.query({ identity,
            mode: "recent", before, limit: 200, signal: abort.signal });
          if (abort.signal.aborted || page.available === false) return false;
          // The retained live version wins if a concurrent update overlaps this
          // older page. Paging only adds owner frames absent from the transcript.
          publishFrames(current => {
            const retained = new Set(current.map(frame => frame.id));
            return [...page.frames.filter(frame => !retained.has(frame.id)), ...current];
          });
          return oldestCursor !== before;
        } catch (reason) {
          if (!abort.signal.aborted) setError(String(reason));
          return false;
        } finally {
          loading = null;
          if (!abort.signal.aborted) setHistory({ available: !!oldestCursor && !exhausted, loading: false });
        }
      })();
      return loading;
    };
    publishFrames(() => []); setHistory({ available: false, loading: false });
    setDraft(""); setContexts([]); setSubmitted(null); setError("");
    void scopedController.timeline.subscribeWithBackfill({ identity, limit: 200, signal: abort.signal,
      onTransportState: state => { if (!abort.signal.aborted) setTransportState(state); },
    }, fact => {
      const frame = fact.value;
      if (frame.event === "snapshot_started" || frame.event === "snapshot_complete") return;
      publishFrames(current => {
        const update = frame.event === "frame_updated" && frame.data && typeof frame.data === "object" && "frame" in frame.data
          ? (frame.data as { frame: ConsoleFrame }).frame : frame;
        const index = current.findIndex(item => item.id === update.id);
        return index < 0 ? [...current, update] : current.map((item, i) => i === index ? update : item);
      });
    }).then(value => {
      if (abort.signal.aborted) value();
      else { unsubscribe = value; historyLoad.complete(seeded); }
    }).catch(reason => {
      historyLoad.complete(false);
      if (!abort.signal.aborted) setError(String(reason));
    });
    return () => { abort.abort(); historyLoad.complete(false); unsubscribe?.(); historyLoad.loadOlder = null; };
  }, [controller, identity, historyLoad]);

  const entries = useMemo(() => mapFramesToTimelineEntries(null, frames, {
    textMode: "markdown", renderTextDeltas: true, renderInteractionStartsAsUser: true, blobBaseUrl: baseUrl,
  }), [frames, baseUrl]);
  const viewState = useMemo(() => buildConversationViewState({ memberId: identity, agentLabel: identity, entries }), [entries, identity]);

  async function revealAnchor(rowId: string, signal: AbortSignal) {
    if (!await awaitHistory(historyLoad.ready, signal)) return false;
    const hasRow = () => conversationPresentationRows(mapFramesToTimelineEntries(null, historyLoad.frames, {
      textMode: "markdown", renderTextDeltas: true, renderInteractionStartsAsUser: true, blobBaseUrl: baseUrl,
    })).some(entry => (entry.renderKey ?? entry.id) === rowId);
    while (!signal.aborted) {
      if (hasRow()) return true;
      if (!historyLoad.loadOlder) return false;
      if (!await awaitHistory(historyLoad.loadOlder(), signal)) return !signal.aborted && hasRow();
    }
    return false;
  }

  async function send(event: React.FormEvent) {
    event.preventDefault();
    try {
      const accepted = await controller.transport.send({ identity, content: contexts.length ? serializeConsoleContextMessage(draft, contexts) : draft,
        origin: `console:shared:${scope}`, idempotencyKey: crypto.randomUUID() });
      setSubmitted(accepted.input_frame_id ?? null); setDraft(""); setContexts([]); setError("");
    } catch (reason) { setError(String(reason)); }
  }

  return <main className="acceptance-host cc-theme-scope">
    <header>
      <strong>Reusable Meta conversation</strong>
      <label>Agent <select aria-label="Agent" value={identity} onChange={event => setIdentity(event.target.value)}>
        <option>router:main</option><option>domain:delivery</option>
      </select></label>
      <button type="button" onClick={() => setTwoPanes(value => !value)}>Toggle second pane</button>
      <button type="button" disabled={!history.available || history.loading} onClick={() => void historyLoad.loadOlder?.()}>Load older history</button>
      <button type="button" onClick={() => { setShowInbox(value => !value); void approvalResource.refresh(); }}>Needs you ({activeApprovals?.status === "ready" ? activeApprovals.requests.length : "?"})</button>
      <button type="button" onClick={() => setScope(value => value === "fixture-principal-a" ? "fixture-principal-b" : "fixture-principal-a")}>Change host scope</button>
      <span data-testid="host-scope">{scope}</span>
    </header>
    <ConsoleTransportStatus state={transportState} onRetry={() => setRevision(value => value + 1)} />
    {error ? <div role="alert">{error}</div> : null}
    {showInbox && activeApprovals ? <aside aria-label="Approval inbox">
      {activeApprovals.requests.map(request => <ApprovalCard key={request.pendingId} request={request}
        resourceStatus={activeApprovals.status} readOnly={activeApprovals.readOnly}
        decision={activeApprovals.decisions[request.pendingId]}
        onDecide={(id, action) => void approvalResource.decide(id, action)} />)}
    </aside> : null}
    <div className="acceptance-panes">
      {[0, ...(twoPanes ? [1] : [])].map(pane => <section key={pane} data-testid={`shared-pane-${pane}`}>
        <ConversationPane viewState={viewState}
          markdownUrlPolicy={markdownUrlPolicy}
          isWorking={resolvePanelResponsePhase({ frames }) !== null}
          approvalSnapshot={activeApprovals}
          onApprovalDecision={(id, action) => void approvalResource.decide(id, action)}
          contextSlot={pane === 0 ? <QuoteContextChips records={contexts} destinationLabel={identity}
            onEdit={(id, quote) => setContexts(editConsoleContextQuote(contexts, id, quote))}
            onRemove={id => setContexts(current => current.filter(item => item.id !== id))} /> : undefined}
          onQuoteSelection={quote => {
            try {
              const record = createConsoleContextRecord({ id: crypto.randomUUID(), sourceScope: scope,
                sourceIdentity: identity, messageId: quote.messageId, quote: quote.text, sourceText: quote.sourceText, label: identity });
              setContexts(current => [...current, record]);
              setError("");
            }
            catch (reason) { setError(String(reason)); }
          }}
          viewportKey={{ authority: `${location.origin}/${scope}`, identity, conversation: identity, pane: String(pane) }}
          onRevealAnchor={revealAnchor}
          submittedRowId={submitted}
          footer={pane === 0 ? <form onSubmit={send}>
            <label>Message to {identity}<textarea aria-label="Message" value={draft} onChange={event => setDraft(event.target.value)} /></label>
            <button type="submit" disabled={!draft.trim()}>Send</button>
          </form> : <p>Second pane reads the same authorized conversation.</p>}
        />
      </section>)}
    </div>
  </main>;
}

createRoot(document.getElementById("root")!).render(<SharedHost />);
