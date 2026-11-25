import React from "react";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ConsoleApp } from "../ConsoleApp";
import { createTwoRootGateway, ensureAbortSignalAny, HOSTED_MOB, type TwoRootGateway } from "./fixtures/two-root-gateway";

// An image on a message. The request door that takes a message with a file
// serves the console's own team (the launch mob) in this version. For an
// agent behind another team's door the composer takes no image and says so,
// and what the person writes still goes out as words; for an agent of the
// console's own team an image is taken and sent as before.

vi.mock("./use-voice-controller", () => ({
  useVoiceController: vi.fn(() => ({ voice: null, state: { phase: "idle", target: null, microphoneMuted: false, speakerMuted: false, error: null, notice: null } })),
}));
vi.mock("../mcp-apps/native-host", () => ({
  createNativeMcpAppsHost: vi.fn(() => ({ sandboxProxyUrl: "https://sandbox.test/sandbox.html", resolve: async () => null })),
}));

const LONG = 30_000;
const SHORT_FACT = "images not available for this team yet";
const sentenceFor = (agent: string) => `Images cannot be attached for ${agent} yet. For now they work with agents of Ops team only.`;

const settle = (ms = 300) => act(async () => { await new Promise((resolve) => window.setTimeout(resolve, ms)); });

async function open(identity: string) {
  await act(async () => { fireEvent.click(await screen.findByTestId(`sidebar-agent:${identity}`)); });
  await screen.findByTestId(`chat-composer:${identity}`);
  await settle();
  return screen.getByTestId(`chat-pane:${identity}`);
}

async function write(identity: string, text: string) {
  const textarea = await screen.findByTestId(`chat-composer:${identity}`) as HTMLTextAreaElement;
  await act(async () => {
    fireEvent.change(textarea, { target: { value: text } });
    fireEvent.keyDown(textarea, { key: "Enter" });
    await Promise.resolve();
  });
  await settle();
}

const image = (name: string) => new File([new Uint8Array([137, 80, 78, 71, 13, 10, 26, 10, 1, 2, 3])], name, { type: "image/png" });
/** What a drop or a paste of files hands the composer. */
const carrying = (files: File[]) => ({ files, items: [], types: ["Files"], getData: () => "" });
const shellOf = (identity: string) => screen.getByTestId(`chat-composer:${identity}`).closest(".composer__shell") as HTMLElement;

/** Drop `file` on the composer: whether the composer took the drop for its own (it is then not left to the browser, which would open the file). */
async function drop(identity: string, file: File): Promise<boolean> {
  let leftToTheBrowser = true;
  await act(async () => { leftToTheBrowser = fireEvent.drop(shellOf(identity), { dataTransfer: carrying([file]) }); await Promise.resolve(); });
  await settle();
  return !leftToTheBrowser;
}

/** Paste `file` into the composer: whether the composer took the paste for its own. */
async function paste(identity: string, file: File): Promise<boolean> {
  let leftToTheBrowser = true;
  await act(async () => { leftToTheBrowser = fireEvent.paste(shellOf(identity), { clipboardData: carrying([file]) }); await Promise.resolve(); });
  await settle();
  return !leftToTheBrowser;
}

const staged = (pane: HTMLElement) => pane.querySelectorAll(".composer__attachment").length;
const withFiles = (gateway: TwoRootGateway) => gateway.requests.filter((request) => request.route === "POST /console/rpc/multipart");

async function start(gateway: TwoRootGateway, firstChat = "planner") {
  vi.stubGlobal("fetch", gateway.fetch);
  render(<ConsoleApp baseUrl="" />);
  await screen.findByTestId(`chat-composer:${firstChat}`, undefined, { timeout: 6_000 });
  await settle(500);
}

let restoreAbortSignal = () => {};
let restoreObjectUrls = () => {};
beforeEach(() => {
  restoreAbortSignal = ensureAbortSignalAny();
  vi.stubGlobal("localStorage", window.localStorage);
  window.localStorage.clear();
  window.sessionStorage.clear();
  let pending = Promise.resolve();
  Object.defineProperty(navigator, "locks", { configurable: true, value: { request: (_key: string, callback: () => unknown) => {
    const next = pending.then(callback); pending = next.then(() => {}, () => {}); return next;
  } } });
  // jsdom makes no object URLs; a staged image's preview asks for one.
  const { createObjectURL, revokeObjectURL } = URL;
  let made = 0;
  URL.createObjectURL = () => `blob:test/${++made}`;
  URL.revokeObjectURL = () => {};
  restoreObjectUrls = () => { URL.createObjectURL = createObjectURL; URL.revokeObjectURL = revokeObjectURL; };
});
afterEach(async () => {
  await act(async () => { await new Promise((resolve) => window.setTimeout(resolve, 20)); });
  cleanup(); vi.unstubAllGlobals(); restoreAbortSignal(); restoreObjectUrls();
});

describe("an image for an agent behind another team's door", () => {
  it("is not taken when dropped: the composer says why, sends no file, and still sends the person's words to that agent", async () => {
    const gateway = createTwoRootGateway({ imageInput: true });
    await start(gateway);
    const pane = await open("support/reviewer");
    // Before anything is dropped the footer says that images are not offered here, and not that the model cannot see them.
    expect(pane.textContent).toContain(SHORT_FACT);
    expect(pane.textContent).not.toContain("model cannot see images");
    gateway.clear();

    // The dropped file is the composer's to refuse: the browser is not left to open it in the console's place.
    // A browser sends the drop only where the drag over the place was taken, so that is taken too, without a mark.
    let dragLeftToTheBrowser = true;
    await act(async () => { dragLeftToTheBrowser = fireEvent.dragOver(shellOf("support/reviewer"), { dataTransfer: carrying([image("a-picture.png")]) }); });
    expect(dragLeftToTheBrowser).toBe(false);
    expect(shellOf("support/reviewer").className).not.toContain("is-drag-active");
    expect(await drop("support/reviewer", image("a-picture.png"))).toBe(true);
    expect(staged(pane)).toBe(0);
    expect(pane.textContent).toContain(sentenceFor("Support reviewer"));

    await write("support/reviewer", "plain words");
    expect(withFiles(gateway)).toEqual([]);
    expect(gateway.calls("meta/console/send").map((request) => [request.route, request.params?.mob_id, request.params?.identity, request.params?.content]))
      .toEqual([["POST /console/rpc", HOSTED_MOB, "reviewer", "plain words"]]);
  }, LONG);

  it("is not taken when pasted", async () => {
    const gateway = createTwoRootGateway({ imageInput: true });
    await start(gateway);
    const pane = await open("support/reviewer");
    gateway.clear();
    expect(await paste("support/reviewer", image("pasted.png"))).toBe(true);
    expect(staged(pane)).toBe(0);
    expect(pane.textContent).toContain(sentenceFor("Support reviewer"));
    expect(withFiles(gateway)).toEqual([]);
    // Words that are pasted are the composer's text as ever: the paste is not taken away and nothing is said.
    const words = { files: [], items: [], types: ["text/plain"], getData: (kind: string) => (kind === "text/plain" ? "pasted words" : "") };
    let leftToTheBrowser = false;
    await act(async () => { leftToTheBrowser = fireEvent.paste(shellOf("support/reviewer"), { clipboardData: words }); });
    expect(leftToTheBrowser).toBe(true);
  }, LONG);

  it("says nothing of images in a console that is view only", async () => {
    const gateway = createTwoRootGateway({ imageInput: true, experience: { console_policy: { read_only: true } } });
    await start(gateway);
    const pane = await open("support/reviewer");
    expect(pane.textContent).toContain("view only");
    expect(pane.textContent).not.toContain(SHORT_FACT);
    expect(await drop("support/reviewer", image("a-picture.png"))).toBe(false);
    expect(staged(pane)).toBe(0);
    expect(pane.textContent).not.toContain(sentenceFor("Support reviewer"));
  }, LONG);

  it("is not taken for an agent of a team nested under that door either", async () => {
    // The roster draws the rows in view; the nested team's agent is reached by a chat the browser kept open.
    const member = "support/support-lab/tester";
    window.localStorage.setItem('meta-console-dock-state:["","console-storage:v1:two-root"]', JSON.stringify({
      tabs: [{ id: "tab-1", presetId: "single", layout: { kind: "panel", panelId: "panel-chat" } }],
      panels: [{ id: "panel-chat", target: { id: member, kind: "agent-chat", addressingMode: "identity", memberId: member, identity: member, title: "Tester" }, mode: "console" }],
      activeTabId: "tab-1",
      focusedPanelId: "panel-chat",
    }));
    const gateway = createTwoRootGateway({ imageInput: true });
    await start(gateway, member);
    const nested = screen.getByTestId(`chat-pane:${member}`);
    expect(nested.textContent).toContain(SHORT_FACT);
    gateway.clear();
    await drop(member, image("for-the-tester.png"));
    expect(staged(nested)).toBe(0);
    expect(nested.textContent).toContain(sentenceFor("Tester"));
    expect(withFiles(gateway)).toEqual([]);
  }, LONG);

  it("says of an agent whose model cannot see images that, as for every agent, and nothing of its team", async () => {
    const gateway = createTwoRootGateway();
    await start(gateway);
    const pane = await open("support/reviewer");
    expect(pane.textContent).toContain("model cannot see images");
    expect(pane.textContent).not.toContain(SHORT_FACT);
    await drop("support/reviewer", image("a-picture.png"));
    expect(staged(pane)).toBe(0);
    expect(pane.textContent).not.toContain(sentenceFor("Support reviewer"));
  }, LONG);
});

describe("an image for an agent of the console's own team", () => {
  it("is taken when dropped and goes out with the message as before, naming no other team", async () => {
    const gateway = createTwoRootGateway({ imageInput: true });
    await start(gateway);
    const pane = await open("reviewer");
    expect(pane.textContent).not.toContain(SHORT_FACT);
    expect(pane.textContent).not.toContain("model cannot see images");
    gateway.clear();

    await drop("reviewer", image("a-picture.png"));
    expect(staged(pane)).toBe(1);
    // A drag over the composer of such an agent is marked, as before.
    await act(async () => { fireEvent.dragOver(shellOf("reviewer"), { dataTransfer: carrying([image("another.png")]) }); });
    expect(shellOf("reviewer").className).toContain("is-drag-active");
    await act(async () => { fireEvent.dragLeave(shellOf("reviewer")); });
    await write("reviewer", "with a picture");
    const sent = withFiles(gateway);
    expect(sent.map((request) => [request.method, request.params?.identity, "mob_id" in (request.params ?? {}), request.files?.length]))
      .toEqual([["meta/console/send", "reviewer", false, 1]]);
    const content = sent[0].params?.content as Array<{ type: string; alt?: string }>;
    expect(content.map((block) => [block.type, block.alt ?? null])).toEqual([["text", null], ["image_upload", "a-picture.png"]]);

    // An agent of a team nested under the console's own team is behind the same door: an image is taken for it too.
    const scout = await open("research/scout");
    expect(scout.textContent).not.toContain(SHORT_FACT);
    await drop("research/scout", image("for-the-scout.png"));
    expect(staged(scout)).toBe(1);
  }, LONG);
});
