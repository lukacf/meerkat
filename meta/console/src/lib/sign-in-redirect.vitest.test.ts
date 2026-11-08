import { afterEach, describe, expect, it, vi } from "vitest";
import * as consoleNetwork from "./network";
import * as coreNetwork from "../../../packages/console-core/src/network";
import { classifyConsoleSendFailure } from "../../../packages/console-core/src/send-attempt";

// A front that wants the browser to sign in answers a console request with a
// redirect to its login page. The console asks for redirects to stay
// unfollowed (`redirect: "manual"`), so the answer is an opaque redirect:
// the request never reached the gateway, so a send was not sent, and the
// console needs a sign-in, exactly as for a 401.

function opaqueRedirect() {
  return {
    type: "opaqueredirect", status: 0, ok: false, redirected: false, statusText: "", url: "",
    headers: new Headers(), body: null,
    text: async () => "", json: async () => { throw new SyntaxError("Unexpected end of JSON input"); },
  } as unknown as Response;
}

afterEach(() => { vi.unstubAllGlobals(); });

describe.each([
  ["console", consoleNetwork],
  ["console-core", coreNetwork],
] as const)("login_redirect_is_unauthenticated (%s network)", (_name, network) => {
  it("asks for unfollowed redirects and refuses a send that met one as unauthenticated", async () => {
    const fetch = vi.fn(async () => opaqueRedirect());
    vi.stubGlobal("fetch", fetch);
    const failure = await network.sendConsole("", "member", "Hello", "console:pane", "key-1").then(() => null, (error: unknown) => error);
    expect(fetch).toHaveBeenCalledTimes(1);
    expect((fetch.mock.calls[0] as unknown as [string, RequestInit])[1].redirect).toBe("manual");
    expect(failure).toBeTruthy();
    expect(classifyConsoleSendFailure(failure)).toMatchObject({ state: "definitely-rejected", kind: "unauthenticated" });
  });

  it("types a redirected read as a sign-in redirect", async () => {
    const fetch = vi.fn(async () => opaqueRedirect());
    vi.stubGlobal("fetch", fetch);
    const failure = await network.fetchJson("", "/console/experience").then(() => null, (error: unknown) => error) as { transportFailure?: string } | null;
    expect((fetch.mock.calls[0] as unknown as [string, RequestInit])[1].redirect).toBe("manual");
    expect(failure?.transportFailure).toBe("sign_in_redirect");
  });

  it("stops the timeline stream as authentication-required", async () => {
    const fetch = vi.fn(async () => opaqueRedirect());
    vi.stubGlobal("fetch", fetch);
    const phases: string[] = [];
    const stop = network.subscribeTimelineEvents("", { identity: "member" }, () => {}, {
      onTransportState: (state) => { phases.push(state.phase); },
    });
    try {
      await vi.waitFor(() => expect(phases).toContain("authentication-required"), { timeout: 1_000 });
      expect((fetch.mock.calls[0] as unknown as [string, RequestInit])[1].redirect).toBe("manual");
      expect(phases).not.toContain("retrying");
    } finally { stop(); }
  });
});
