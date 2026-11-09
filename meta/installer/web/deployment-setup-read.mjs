import { request } from "./state.mjs";

// These reads verify the native package, which can exceed the ordinary GET
// deadline. Share pending reads across component lifetimes, never their results.
// Port changes must wait for the previous check instead of starting another
// package verification or receiving a response for the wrong port.
export function createDeploymentSetupReader(send = request) {
  const pendingSources = new Map(), pendingReads = new Map();
  return function readDeploymentSetup(url) {
    const parsed = new URL(url, "http://forge.local");
    if (!/^\/api\/guide\/[^/]+\/deployment\/(options|settings)$/.test(parsed.pathname))
      throw new Error("Expected a deployment setup read.");
    parsed.searchParams.sort();
    const source = JSON.stringify([parsed.pathname, parsed.searchParams.get("build_id"),
      parsed.searchParams.get("deployment_id"), parsed.searchParams.get("source_kind"),
      parsed.searchParams.get("version")]);
    const key = parsed.pathname + parsed.search;
    if (pendingReads.has(key)) return pendingReads.get(key);
    const previous = pendingSources.get(source);
    const start = () => send(url, { timeoutMs: 120000 });
    const pending = (previous ? previous.catch(() => {}).then(start) : Promise.resolve().then(start))
      .finally(() => {
        if (pendingReads.get(key) === pending) pendingReads.delete(key);
        if (pendingSources.get(source) === pending) pendingSources.delete(source);
      });
    pendingReads.set(key, pending);
    pendingSources.set(source, pending);
    return pending;
  };
}

export const readDeploymentSetup = createDeploymentSetupReader();
