import { act, cleanup } from "@testing-library/react";
import { afterEach, beforeEach, describe, it, vi } from "vitest";

import { SWEEP_PANES, sweepOpenWhileTheRootGoesToSleep } from "./fixtures/asleep-sweep";
import { ensureAbortSignalAny } from "./fixtures/two-root-gateway";

// THE SWEEP, second half: every pane the console has, left open while a
// loaded hosted root goes to sleep. Whatever the pane, nothing the console
// does on its own from then on names the sleeping root.
// (First half: hosted-mob-asleep-sweep-restored.)

vi.mock("./use-voice-controller", () => ({
  useVoiceController: vi.fn(() => ({ voice: null, state: { phase: "idle", target: null, microphoneMuted: false, speakerMuted: false, error: null, notice: null } })),
}));
vi.mock("../mcp-apps/native-host", () => ({
  createNativeMcpAppsHost: vi.fn(() => ({ sandboxProxyUrl: "https://sandbox.test/sandbox.html", resolve: async () => null })),
}));

let restoreAbortSignal = () => {};
beforeEach(() => {
  restoreAbortSignal = ensureAbortSignalAny();
  vi.stubGlobal("localStorage", window.localStorage);
  window.localStorage.clear();
  window.sessionStorage.clear();
  let pending = Promise.resolve();
  Object.defineProperty(navigator, "locks", { configurable: true, value: { request: (_key: string, callback: () => unknown) => {
    const next = pending.then(callback); pending = next.then(() => {}, () => {}); return next;
  } } });
});
afterEach(async () => {
  await act(async () => { await new Promise((resolve) => window.setTimeout(resolve, 20)); });
  cleanup(); vi.unstubAllGlobals(); restoreAbortSignal();
});

describe.each(SWEEP_PANES)("with $name open, a root that goes to sleep", ({ target }) => {
  it("is named in no request the console makes on its own", async () => {
    await sweepOpenWhileTheRootGoesToSleep(target);
  }, 30_000);
});
