/// Test-only render accounting. Components call `countRender(name)` at the
/// top of their render body; the call is a no-op unless a test has installed
/// the sink on `globalThis.__consoleRenderCounts`. Production bundles keep
/// the call (one property read per render) and never see a sink.
export type RenderCounts = Record<string, number>;

declare global {
  // eslint-disable-next-line no-var
  var __consoleRenderCounts: RenderCounts | undefined;
}

export function countRender(name: string, amount = 1): void {
  const sink = globalThis.__consoleRenderCounts;
  if (sink) sink[name] = (sink[name] ?? 0) + amount;
}

export function installRenderCounts(): RenderCounts {
  const sink: RenderCounts = {};
  globalThis.__consoleRenderCounts = sink;
  return sink;
}

export function resetRenderCounts(): void {
  const sink = globalThis.__consoleRenderCounts;
  if (!sink) return;
  for (const key of Object.keys(sink)) delete sink[key];
}

export function uninstallRenderCounts(): void {
  globalThis.__consoleRenderCounts = undefined;
}
