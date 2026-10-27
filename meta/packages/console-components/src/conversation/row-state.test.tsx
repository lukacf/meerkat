import { fireEvent, render, screen } from "@testing-library/react";
import type { ReactNode } from "react";
import { describe, expect, it } from "vitest";

import type { ConversationFlowRunEntry, ConversationRichThinkingBlock, ConversationRichToolCallBlock } from "@console-core";
import { ConversationRichContent } from "./conversation-rich-content";
import { FlowRunCard } from "./flow-run-card";
import { ConversationPresentationProvider, ConversationRowStateScope, RowDetails } from "./presentation-policy";

/** A pane whose rows can be unmounted and mounted again, as a windowed transcript does. */
function Pane({ mounted, pane = "left", children }: { mounted: boolean; pane?: string; children: ReactNode }) {
  return (
    <ConversationPresentationProvider viewportKey={{ authority: "row-state-test", identity: "agent", conversation: "c", pane }}>
      {mounted ? <ConversationRowStateScope rowId="row-1">{children}</ConversationRowStateScope> : null}
    </ConversationPresentationProvider>
  );
}

function toggle(details: HTMLDetailsElement, open: boolean) {
  details.open = open;
  fireEvent(details, new Event("toggle"));
}

describe("row state survives a row unmounting and mounting again", () => {
  it("keeps a disclosure the reader opened", () => {
    const row = <RowDetails part="details" data-testid="details"><summary>More</summary>body</RowDetails>;
    const view = render(<Pane mounted>{row}</Pane>);
    const details = screen.getByTestId("details") as HTMLDetailsElement;
    expect(details.open).toBe(false);
    toggle(details, true);
    view.rerender(<Pane mounted={false}>{row}</Pane>);
    view.rerender(<Pane mounted>{row}</Pane>);
    expect((screen.getByTestId("details") as HTMLDetailsElement).open).toBe(true);
    // Another pane keeps its own state.
    view.rerender(<Pane mounted pane="right">{row}</Pane>);
    expect((screen.getByTestId("details") as HTMLDetailsElement).open).toBe(false);
  });

  it("is component-local outside a row scope, as before", () => {
    const details = () => <RowDetails part="details" data-testid="details"><summary>More</summary>body</RowDetails>;
    const view = render(<ConversationPresentationProvider>{details()}</ConversationPresentationProvider>);
    toggle(screen.getByTestId("details") as HTMLDetailsElement, true);
    view.rerender(<ConversationPresentationProvider>{null}</ConversationPresentationProvider>);
    view.rerender(<ConversationPresentationProvider>{details()}</ConversationPresentationProvider>);
    expect((screen.getByTestId("details") as HTMLDetailsElement).open).toBe(false);
  });

  it("keeps a thinking block as it was, open while it streamed or closed by the reader", () => {
    const streaming: ConversationRichThinkingBlock = { type: "thinking", label: "Thinking", text: "step one" };
    const persisted: ConversationRichThinkingBlock = { ...streaming, final: true, persisted: true };
    const thinking = (block: ConversationRichThinkingBlock) => <ConversationRichContent blocks={[block]} />;
    const view = render(<Pane mounted>{thinking(streaming)}</Pane>);
    const open = () => (document.querySelector(".cc-rich-thinking") as HTMLDetailsElement).open;
    expect(open()).toBe(true);
    // Hydration makes it persisted; remounting must not fold what the reader saw open.
    view.rerender(<Pane mounted={false}>{thinking(persisted)}</Pane>);
    view.rerender(<Pane mounted>{thinking(persisted)}</Pane>);
    expect(open()).toBe(true);
    toggle(document.querySelector(".cc-rich-thinking") as HTMLDetailsElement, false);
    view.rerender(<Pane mounted={false}>{thinking(persisted)}</Pane>);
    view.rerender(<Pane mounted>{thinking(persisted)}</Pane>);
    expect(open()).toBe(false);
  });

  it("keeps a tool call the reader expanded", () => {
    const tool: ConversationRichToolCallBlock = { type: "tool-call", toolCallId: "call-1", name: "read_file", arguments: "{}", result: "ok", status: "success" };
    const content = <ConversationRichContent blocks={[tool]} />;
    const view = render(<Pane mounted>{content}</Pane>);
    const header = () => document.querySelector(".cc-tool-call__header") as HTMLElement;
    expect(header().getAttribute("aria-expanded")).toBe("false");
    fireEvent.click(header());
    expect(header().getAttribute("aria-expanded")).toBe("true");
    view.rerender(<Pane mounted={false}>{content}</Pane>);
    view.rerender(<Pane mounted>{content}</Pane>);
    expect(header().getAttribute("aria-expanded")).toBe("true");
  });

  it("keeps a completed-tools fold as it first rendered, whatever the scroll mode when it mounts again", () => {
    const tools: ConversationRichToolCallBlock[] = [0, 1].map((i) => ({
      type: "tool-call", toolCallId: `fold-${i}`, name: "read_file", arguments: "{}", result: "ok", status: "success",
      completionEvidence: { outcome: "success", toolCallId: `fold-${i}`, source: "runtime-result" } as ConversationRichToolCallBlock["completionEvidence"],
    }));
    const pane = (mounted: boolean, autoFold: boolean) => (
      <ConversationPresentationProvider autoFold={autoFold} viewportKey={{ authority: "row-state-test", identity: "agent", conversation: "fold", pane: "left" }}>
        {mounted ? <ConversationRowStateScope rowId="row-fold"><ConversationRichContent blocks={tools} /></ConversationRowStateScope> : null}
      </ConversationPresentationProvider>
    );
    // First rendered while following the live edge: folded.
    const view = render(pane(true, true));
    const open = () => (document.querySelector(".cc-completed-tools") as HTMLDetailsElement).open;
    expect(open()).toBe(false);
    // Mounted again while reading history (where a new fold would start open).
    view.rerender(pane(false, false));
    view.rerender(pane(true, false));
    expect(open()).toBe(false);
  });

  it("keeps a flow run's details choice, and still folds the card when its run completes", () => {
    const entry = (status: ConversationFlowRunEntry["status"]): ConversationFlowRunEntry => ({
      kind: "flow_run", id: "flow-1", identity: { role: "assistant", label: "Router" } as ConversationFlowRunEntry["identity"],
      helperId: "helper-1", flowName: "Crew", status, rows: [{ memberKey: "m-1", label: "Analyst", caption: "", status }],
    });
    const card = (status: ConversationFlowRunEntry["status"]) => <FlowRunCard entry={entry(status)} />;
    const expanded = () => document.querySelector("[data-flow-run-card]")!.getAttribute("data-details-expanded");
    // A failed run keeps its details open; the reader closes them.
    const view = render(<Pane mounted>{card("failed")}</Pane>);
    expect(expanded()).toBe("true");
    fireEvent.click(document.querySelector(".cc-flow-run__disclosure") as HTMLElement);
    expect(expanded()).toBe("false");
    view.rerender(<Pane mounted={false}>{card("failed")}</Pane>);
    view.rerender(<Pane mounted>{card("failed")}</Pane>);
    expect(expanded()).toBe("false");
    // A status change applies that status's default, as before.
    view.rerender(<Pane mounted>{card("running")}</Pane>);
    expect(expanded()).toBe("true");
    view.rerender(<Pane mounted>{card("completed")}</Pane>);
    expect(expanded()).toBe("false");
  });
});
