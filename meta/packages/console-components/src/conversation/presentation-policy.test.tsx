import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, test, vi } from "vitest";
import type { ConversationRichToolCallBlock } from "@console-core";
import { readFileSync } from "node:fs";
const stylePath = "../styles/conversation.css";
const conversationStyles = readFileSync(new URL(stylePath, import.meta.url), "utf8");
import { ConversationRichContent } from "./conversation-rich-content";
import { canFoldCompletedTools, ConversationHeader, ConversationPresentationProvider, explicitDisplayLabel, groupRoutineToolRows } from "./presentation-policy";

const tool = (id: string, overrides = {}): ConversationRichToolCallBlock => ({ type: "tool-call", toolCallId: id, name: "read_file", arguments: '{"path":"notes.txt"}', result: "original output", status: "success", completionEvidence: { outcome: "success", source: "session-history", toolCallId: id }, ...overrides });
describe("conservative presentation", () => {
  test("fold eligibility requires allowlisted generic tools and matched owner success", () => {
    expect(canFoldCompletedTools([tool("a"), tool("b")])).toBe(true);
    for (const outcome of ["unknown", "running", "error", "cancelled", "interrupted"]) {
      expect(canFoldCompletedTools([tool("a"), tool("b", { completionEvidence: { outcome, source: "runtime-result", toolCallId: "b" } })])).toBe(false);
    }
    for (const override of [{ name: "send_message" }, { name: "workgraph_update" }, { peerTarget: "peer" }, { peerDisplayLabel: "Delivery" }, { completionEvidence: undefined }, { completionEvidence: { outcome: "success", source: "session-history", toolCallId: "wrong" } }]) {
      expect(canFoldCompletedTools([tool("a"), tool("b", override)])).toBe(false);
    }
  });
  test("a completed disclosure preserves all detail and new work is exposed", () => {
    const view = render(<ConversationPresentationProvider><ConversationRichContent blocks={[tool("a"), tool("b")]} /></ConversationPresentationProvider>);
    const summary = screen.getByText("2 completed tool calls");
    expect(summary.closest("details")).not.toHaveAttribute("open");
    fireEvent.click(summary);
    expect(screen.getAllByText("read_file")).toHaveLength(1);
    expect(screen.getAllByText("original output")).toHaveLength(2);
    view.rerender(<ConversationPresentationProvider><ConversationRichContent blocks={[tool("a"), tool("b"), tool("c", { status: "pending", completionEvidence: undefined })]} /></ConversationPresentationProvider>);
    expect(screen.getByText("2 completed tool calls")).toBeInTheDocument();
    expect(screen.getByText("⋯ Running")).toBeInTheDocument();
  });
  test("adjacent mixed routine tools fold but prose and unknown results split runs", () => {
    const rows = [tool("a"), tool("b", { name: "list_files" }), { type: "paragraph" as const, text: "Visible prose" }, tool("c"), tool("d", { completionEvidence: undefined }), tool("e")];
    expect(groupRoutineToolRows(rows, (row) => [row]).map((run) => run.tools.length)).toEqual([2, 0, 1, 0, 1]);
    render(<ConversationRichContent blocks={rows} />);
    expect(screen.getAllByText("2 completed tool calls")).toHaveLength(1);
    expect(screen.getByText("Visible prose")).toBeInTheDocument();
  });
  test("new completion while reading starts expanded and disclosure choices stay scoped", () => {
    const key = { authority: "presentation-test", identity: "one", conversation: "one", pane: "one" };
    const view = render(<ConversationPresentationProvider viewportKey={key} autoFold={false}><ConversationRichContent blocks={[tool("a"), tool("b")]} /></ConversationPresentationProvider>);
    expect(screen.getByText("2 completed tool calls").closest("details")).toHaveAttribute("open");
    view.rerender(<ConversationPresentationProvider viewportKey={{ ...key, identity: "two" }}><ConversationRichContent blocks={[tool("a"), tool("b")]} /></ConversationPresentationProvider>);
    expect(screen.getByText("2 completed tool calls").closest("details")).not.toHaveAttribute("open");
  });
  test("completing tool siblings preserves a Markdown document DOM node", () => {
    const pending = tool("b", { status: "pending", completionEvidence: { outcome: "running", source: "runtime-start", toolCallId: "b" } });
    const document = { type: "markdown" as const, id: "stable-text", source: "Selected reply", streaming: true };
    const view = render(<ConversationRichContent blocks={[tool("a"), pending, document]} />);
    const node = screen.getByText("Selected reply");
    view.rerender(<ConversationRichContent blocks={[tool("a"), tool("b"), document]} />);
    expect(screen.getByText("Selected reply")).toBe(node);
  });
  test("raw tool copy preserves input and output bytes after disclosure", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });
    const source = tool("raw", { arguments: "  {\"path\":\"a\"}\n", result: "  exact output\n" });
    render(<ConversationRichContent blocks={[source]} />);
    fireEvent.click(screen.getByRole("button", { name: "Copy" }));
    await waitFor(() => expect(writeText).toHaveBeenCalledWith('$ read_file\nInput:   {"path":"a"}\n\nResult:   exact output\n'));
  });
  test("labels use exact authorized map and raw identifiers remain inspectable", () => {
    expect(explicitDisplayLabel("peer:full:id", new Map([["id", "Wrong"]]))).toBe("peer:full:id");
    render(<ConversationPresentationProvider labels={{ tools: new Map([["read_file", "Read file"]]) }}><ConversationRichContent blocks={[tool("a")]} /></ConversationPresentationProvider>);
    expect(screen.getByText("Read file")).toHaveAttribute("title", "read_file");
  });
  test.each([true, false])("canonical peer labels render for incoming=%s and preserve the exact identity", (peerIncoming) => {
    const peer = tool("canonical", { name: "send_message", peerIncoming, peerIdentity: "peer:full:id", peerDisplayLabel: "Delivery", peerTarget: "Untrusted argument", peerBody: "Please review this." });
    render(<ConversationRichContent blocks={[peer]} />);
    expect(screen.getByText(peerIncoming ? "Received from Delivery" : "Delivery")).toHaveAttribute("title", "peer:full:id");
    expect(screen.queryByText("Untrusted argument")).not.toBeInTheDocument();
  });
  test("an exact host peer label overrides canonical display metadata", () => {
    const peer = tool("override", { name: "send_message", peerIncoming: true, peerIdentity: "peer:full:id", peerDisplayLabel: "Delivery", peerTarget: "Untrusted argument" });
    render(<ConversationPresentationProvider labels={{ peers: new Map([["peer:full:id", "Household delivery"]]) }}><ConversationRichContent blocks={[peer]} /></ConversationPresentationProvider>);
    expect(screen.getByText("Received from Household delivery")).toHaveAttribute("title", "peer:full:id");
    expect(screen.queryByText("Received from Delivery")).not.toBeInTheDocument();
  });
  test.each([
    { peerIdentity: "peer:full:id", peerTarget: undefined, expected: "peer:full:id" },
    { peerIdentity: "peer:full:id", peerTarget: "Untrusted argument", expected: "peer:full:id" },
    { peerIdentity: undefined, peerTarget: "legacy-peer", expected: "legacy-peer" },
  ])("missing canonical labels fall back to the exact identity or legacy target: $expected", ({ expected, ...identity }) => {
    const peer = tool("fallback", { name: "send_message", peerIncoming: true, ...identity });
    render(<ConversationPresentationProvider labels={{ peers: new Map([["id", "Wrong short ID"], ["Untrusted argument", "Wrong argument label"]]) }}><ConversationRichContent blocks={[peer]} /></ConversationPresentationProvider>);
    expect(screen.getByText(`Received from ${expected}`)).toHaveAttribute("title", expected);
    expect(screen.queryByText(/Wrong short ID|Wrong argument label/)).not.toBeInTheDocument();
  });
  test("grouped peer names preserve distinct identities even when canonical labels match", () => {
    const peer = (id: string, peerIdentity: string) => tool(id, { name: "send_message", peerIncoming: true, peerIdentity, peerDisplayLabel: "Reviewer", peerBody: `Message ${id}` });
    const view = render(<ConversationRichContent blocks={[peer("a", "peer:first"), peer("b", "peer:second"), peer("c", "peer:first")]} />);
    expect(screen.getByText("Received from Reviewer, Reviewer")).toHaveAttribute("title", "peer:first, peer:second");
    const targets = Array.from(view.container.querySelectorAll(".cc-tool-call__peer-target"));
    expect(targets.map((target) => target.textContent)).toEqual(["← Reviewer", "← Reviewer", "← Reviewer"]);
    expect(targets.map((target) => target.getAttribute("title"))).toEqual(["peer:first", "peer:second", "peer:first"]);
  });
  test("grouped peer headers and detail rows use the same exact host override", () => {
    const peer = (id: string) => tool(id, { name: "send_message", peerIdentity: `peer:${id}`, peerDisplayLabel: "Reviewer", peerBody: `Message ${id}` });
    const view = render(<ConversationPresentationProvider labels={{ peers: new Map([["peer:a", "Lead reviewer"]]) }}><ConversationRichContent blocks={[peer("a"), peer("b")]} /></ConversationPresentationProvider>);
    expect(screen.getByText("Sent to Lead reviewer, Reviewer")).toHaveAttribute("title", "peer:a, peer:b");
    const targets = Array.from(view.container.querySelectorAll(".cc-tool-call__peer-target"));
    expect(targets.map((target) => target.textContent)).toEqual(["→ Lead reviewer", "→ Reviewer"]);
    expect(targets.map((target) => target.getAttribute("title"))).toEqual(["peer:a", "peer:b"]);
  });
  test.each(["error", "cancelled", "interrupted", "unknown"] as const)("a late %s outcome exposes a same-ID tool result", (outcome) => {
    const pending = tool("late", { status: "pending", result: undefined, completionEvidence: { outcome: "running", source: "runtime-start", toolCallId: "late" } });
    const view = render(<ConversationRichContent blocks={[pending]} />);
    const header = view.container.querySelector(".cc-tool-call__header")!;
    expect(header).toHaveAttribute("aria-expanded", "true");
    fireEvent.click(header);
    expect(header).toHaveAttribute("aria-expanded", "false");
    view.rerender(<ConversationRichContent blocks={[{ ...pending, status: outcome === "error" ? "error" : "pending", result: "Actionable detail", completionEvidence: { outcome, source: "runtime-result", toolCallId: "late" } }]} />);
    expect(header).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByText("Actionable detail")).toBeVisible();
    fireEvent.click(header);
    expect(header).toHaveAttribute("aria-expanded", "false");
  });
  test("a collapsed group reopens for failed or newly active work", () => {
    const pending = (id: string) => tool(id, { name: "write_file", status: "pending", result: undefined, completionEvidence: { outcome: "running", source: "runtime-start", toolCallId: id } });
    const view = render(<ConversationRichContent blocks={[pending("a"), pending("b")]} />);
    const header = view.container.querySelector(".cc-tool-call__header")!;
    fireEvent.click(header);
    view.rerender(<ConversationRichContent blocks={[pending("a"), { ...pending("b"), status: "error", result: "Cannot write file", completionEvidence: { outcome: "error", source: "runtime-result", toolCallId: "b" } }]} />);
    expect(screen.getByText("Cannot write file")).toBeVisible();
    fireEvent.click(header);
    view.rerender(<ConversationRichContent blocks={[pending("a"), pending("b"), pending("c")]} />);
    expect(header).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByText("#3")).toBeVisible();
  });
  test.each([
    ["unknown", "Completion unknown"], ["interrupted", "Interrupted"], ["cancelled", "Cancelled"],
  ] as const)("a mixed failed group exposes its %s child without inventing a result", async (outcome, label) => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });
    const failed = tool("missing", { status: "error", result: "File is missing", completionEvidence: { outcome: "error", source: "runtime-result", toolCallId: "missing" } });
    const unresolved = tool("late", { status: "pending", result: undefined, arguments: '{"path":"late-review.txt"}', completionEvidence: { outcome, source: "runtime-start", toolCallId: "late" } });
    const view = render(<ConversationRichContent blocks={[failed, unresolved]} />);
    expect(screen.getByText("✗ Failed")).toBeVisible();
    const children = view.container.querySelectorAll<HTMLElement>(".cc-tool-call__sub");
    expect(children).toHaveLength(2);
    const child = within(children[1]);
    const status = child.getByRole("status", { name: label });
    expect(status).toHaveTextContent(label);
    expect(status).toBeVisible();
    expect(status).toHaveAttribute("title", label);
    expect(child.queryByText("Result", { exact: true })).toBeNull();
    expect(view.container.querySelector(".cc-completed-tools")).toBeNull();
    expect(canFoldCompletedTools([failed, unresolved])).toBe(false);
    fireEvent.click(screen.getByRole("button", { name: "Copy" }));
    await waitFor(() => expect(writeText).toHaveBeenCalledWith('$ read_file\nInput: {"path":"notes.txt"}\nResult: File is missing\n$ read_file\nInput: {"path":"late-review.txt"}'));
  });
  test("ordinary grouped outcomes retain compact icons with accessible labels", () => {
    const view = render(<ConversationRichContent blocks={[
      tool("done"),
      tool("failed", { status: "error", completionEvidence: { outcome: "error", source: "runtime-result", toolCallId: "failed" } }),
      tool("working", { status: "pending", result: undefined, completionEvidence: { outcome: "running", source: "runtime-start", toolCallId: "working" } }),
    ]} />);
    const children = view.container.querySelectorAll<HTMLElement>(".cc-tool-call__sub");
    for (const [index, label, icon] of [[0, "Success", "✓"], [1, "Failed", "✗"], [2, "Running", "⋯"]] as const) {
      const status = within(children[index]).getByRole("status", { name: label });
      expect(status.textContent).toBe(icon);
      expect(status).toHaveAttribute("title", label);
    }
  });
  test("a collapsed peer group reopens with a late delivery failure result", () => {
    const peer = (id: string) => tool(id, { name: "send_message", status: "pending", result: undefined, peerIdentity: `peer:${id}`, peerBody: "Please review this.", completionEvidence: { outcome: "running", source: "runtime-start", toolCallId: id } });
    const view = render(<ConversationRichContent blocks={[peer("a"), peer("b")]} />);
    const header = view.container.querySelector(".cc-tool-call__header")!;
    fireEvent.click(header);
    view.rerender(<ConversationRichContent blocks={[peer("a"), { ...peer("b"), status: "error", result: "Peer delivery denied", completionEvidence: { outcome: "error", source: "runtime-result", toolCallId: "b" } }]} />);
    expect(header).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByText("Peer delivery denied")).toBeVisible();
  });
  test("single and grouped peer messages expose full wrapping multiline bodies", () => {
    const body = "A long request to review the uploaded diagram and report every discrepancy.\nKeep this second line intact. 🌳";
    const peer = (id: string) => tool(id, { name: "send_message", peerIdentity: `peer:${id}`, peerBody: body });
    const view = render(<><style>{conversationStyles}</style><ConversationRichContent blocks={[peer("a")]} /></>);
    const single = view.container.querySelector(".cc-tool-call__peer-body")!;
    expect(single.textContent).toBe(body);
    expect(single).toBeVisible();
    expect(getComputedStyle(single).whiteSpace).toBe("pre-wrap");
    expect(getComputedStyle(single).overflow).not.toBe("hidden");
    view.rerender(<><style>{conversationStyles}</style><ConversationRichContent blocks={[peer("a"), peer("b")]} /></>);
    const bodies = view.container.querySelectorAll(".cc-tool-call__peer-body");
    expect(bodies).toHaveLength(2);
    for (const node of bodies) {
      expect(node.textContent).toBe(body);
      expect(node).toBeVisible();
      expect(getComputedStyle(node).whiteSpace).toBe("pre-wrap");
      expect(getComputedStyle(node).overflow).not.toBe("hidden");
    }
  });
  test("compact header keeps target and actions while full header includes identity", () => {
    const view = render(<ConversationHeader title="Agent" identity="canonical:agent" detail="worker" actions={<button>Details</button>} />);
    expect(screen.getByText("canonical:agent · worker")).toBeInTheDocument();
    view.rerender(<ConversationHeader title="Agent" identity="canonical:agent" variant="compact" actions={<button>Details</button>} />);
    expect(screen.getByText("Agent").parentElement).toHaveAttribute("title", "canonical:agent");
    expect(screen.getByRole("button", { name: "Details" })).toBeInTheDocument();
    expect(screen.queryByText("canonical:agent · worker")).toBeNull();
  });
});
