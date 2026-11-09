import React from "react";
import {
  type ConsolePanelDefinition, type ConsolePanelContext, type ConsolePanelMount,
  consoleCustomPanelTarget, type ConsoleCustomPanelTarget, type ConsoleConversationTarget,
} from "@console-core";

type PanelState = { panels: readonly ConsolePanelDefinition[]; context: ConsolePanelContext };
const PanelContext = React.createContext<PanelState | null>(null);
export const ConsolePanelsProvider = PanelContext.Provider;

/** Contain lifecycle failures to one panel surface, including update/dispose. */
export function ConsolePanelSurface<T>({ mount, context, fallback }: {
  mount: ConsolePanelMount<T>; context: T; fallback: React.ReactNode;
}) {
  const container = React.useRef<HTMLDivElement>(null);
  const instance = React.useRef<ReturnType<ConsolePanelMount<T>>>(undefined);
  const abort = React.useRef<AbortController | null>(null);
  const [failed, setFailed] = React.useState(false);
  const latest = React.useRef(context);
  latest.current = context;
  const cleanup = React.useCallback(() => {
    abort.current?.abort();
    abort.current = null;
    const current = instance.current;
    instance.current = undefined;
    try { if (current) current.dispose(); } catch { /* Isolate panel disposal failures. */ }
    container.current?.replaceChildren();
  }, []);
  React.useLayoutEffect(() => cleanup, [cleanup]);
  const start = React.useCallback((value: T) => {
    abort.current = new AbortController();
    instance.current = mount(container.current!, value, abort.current.signal);
  }, [mount]);
  React.useEffect(() => {
    setFailed(false);
    try { start(latest.current); } catch { cleanup(); setFailed(true); }
    return cleanup;
  }, [start, cleanup]);
  const previous = React.useRef(context);
  React.useEffect(() => {
    if (previous.current === context) return;
    previous.current = context;
    if (failed) return;
    try {
      if (instance.current && instance.current.update) instance.current.update(context);
      else { cleanup(); start(context); }
    } catch { cleanup(); setFailed(true); }
  }, [context, failed, cleanup, start]);
  return <><div ref={container} hidden={failed} className="console-custom-panel-surface" />{failed ? fallback : null}</>;
}

function targetContext(base: ConsolePanelContext, conversation: ConsoleConversationTarget | null): ConsolePanelContext {
  return { ...base, conversation,
    request: (request, signal) => base.request(request, signal, conversation),
    openPanel: (id, options) => {
      const input = typeof options === "string" ? { intent: options } : options ?? {};
      base.openPanel(id, { ...input, conversation: input.conversation === undefined ? conversation : input.conversation });
    },
  };
}

export function ConsoleCustomPanel({ target, focused }: { target: ConsoleCustomPanelTarget; focused: boolean }) {
  const state = React.useContext(PanelContext);
  const payload = target.payload;
  const panel = state?.panels.find(panel => panel.id === payload?.panelId);
  const context = React.useMemo(() => {
    if (!panel || !state || target.payloadVersion !== 1 || typeof payload.instanceKey !== "string"
      || typeof payload.followSelection !== "boolean" || !("params" in payload) || !("conversation" in payload)) return null;
    try { consoleCustomPanelTarget(panel, payload); } catch { return null; }
    if (payload.scopeKey !== state.context.authority.key) return null;
    const conversation = payload.followSelection ? state.context.selection : payload.conversation;
    if (conversation && (conversation.scopeKey !== state.context.authority.key || !state.context.visibleIdentities.includes(conversation.identity))) return null;
    return { ...targetContext(state.context, conversation), panel: { instanceKey: payload.instanceKey, params: payload.params, focused } };
  }, [panel, state?.context, payload, target.payloadVersion, focused]);
  const fallback = <div role="status">Custom panel unavailable: {panel?.title ?? "Unknown panel"}</div>;
  // Replacing a target or following another conversation destroys private DOM
  // and aborts its work before mounting the replacement selection.
  const key = JSON.stringify([target.id, context?.authority.key, context?.conversation, context?.panel?.params]);
  return <div className="console-panel" data-custom-panel={payload?.panelId}>{panel && context
    ? <ConsolePanelSurface key={key} mount={panel.mount} context={context} fallback={fallback} />
    : fallback}</div>;
}
