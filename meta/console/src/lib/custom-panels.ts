import React from "react";
import { consolePanelModuleUrl, validateConsolePanels, type ConsolePanelDefinition, type ConsolePanelService } from "@console-core";

const EMPTY_PANELS: readonly ConsolePanelDefinition[] = [];

/** Developer modules contribute panels only. MCP Apps use their own host. */
export function useConsolePanels(
  supplied: readonly ConsolePanelDefinition[] = EMPTY_PANELS,
  modules: readonly string[] = [],
  baseUrl = "",
  load: (url: string) => Promise<{ default: readonly ConsolePanelDefinition[] }> = loadModule,
) {
  const moduleKey = JSON.stringify(modules);
  const [loaded, setLoaded] = React.useState<{
    supplied: readonly ConsolePanelDefinition[]; moduleKey: string; baseUrl: string;
    panels: readonly ConsolePanelDefinition[]; errors: string[];
  } | null>(null);
  const initial = React.useMemo(() => {
    try { validateConsolePanels(supplied); return { panels: supplied, errors: [] as string[] }; }
    catch (error) { return { panels: EMPTY_PANELS, errors: [String(error)] }; }
  }, [supplied]);
  React.useEffect(() => {
    let active = true;
    const paths: string[] = JSON.parse(moduleKey);
    void Promise.all(paths.map(async path => {
      try { return { panels: (await load(consolePanelModuleUrl(path, baseUrl))).default }; }
      catch (error) { return { error: `${path}: ${String(error)}` }; }
    })).then(results => {
      const panels = [...initial.panels], errors = [...initial.errors];
      for (const result of results) {
        if (result.error) { errors.push(result.error); continue; }
        try { validateConsolePanels([...panels, ...result.panels!]); panels.push(...result.panels!); }
        catch (error) { errors.push(String(error)); }
      }
      if (active) setLoaded({ supplied, moduleKey, baseUrl, panels, errors });
    });
    return () => { active = false; };
  }, [supplied, moduleKey, baseUrl, load, initial]);
  return loaded?.supplied === supplied && loaded.moduleKey === moduleKey && loaded.baseUrl === baseUrl ? loaded : initial;
}

function loadModule(url: string): Promise<{ default: readonly ConsolePanelDefinition[] }> {
  return import(/* @vite-ignore */ url);
}

/** No credentials are handed to plugins, and requests are never replayed. */
export function createConsolePanelService(baseUrl: string): ConsolePanelService {
  return async (request, scope, signal) => {
    const method = request.method ?? "GET";
    if (scope.readOnly && method !== "GET") throw new Error("This console is view only");
    const url = consolePanelModuleUrl(request.path, baseUrl);
    // Locators only. The server derives the viewer from its authenticated session.
    const headers: Record<string, string> = {
      "X-Console-Authority": encodeURIComponent(JSON.stringify(scope.authority)),
      "X-Console-Conversation": encodeURIComponent(JSON.stringify(scope.conversation)),
    };
    if (request.body !== undefined) headers["Content-Type"] = "application/json";
    const response = await fetch(url, { method, signal, headers, credentials: "same-origin", redirect: "error",
      ...(request.body === undefined ? {} : { body: JSON.stringify(request.body) }),
    });
    if (!response.ok) throw new Error(`Panel request failed (${response.status})`);
    return response.status === 204 ? null : response.json();
  };
}

export function bindConsolePanelService(service: ConsolePanelService, authoritySignal: AbortSignal): ConsolePanelService {
  return async (request, scope, signal) => {
    if (scope.readOnly && (request.method ?? "GET") !== "GET") throw new Error("This console is view only");
    const combined = new AbortController();
    const abort = () => combined.abort();
    authoritySignal.addEventListener("abort", abort, { once: true });
    signal.addEventListener("abort", abort, { once: true });
    try {
      if (authoritySignal.aborted || signal.aborted) combined.abort();
      combined.signal.throwIfAborted();
      const result = await service(request, scope, combined.signal);
      combined.signal.throwIfAborted();
      return result;
    } finally {
      authoritySignal.removeEventListener("abort", abort);
      signal.removeEventListener("abort", abort);
    }
  };
}
