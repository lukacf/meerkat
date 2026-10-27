// NOTE (2026-07-14 audit): this file is written against vitest globals and
// @testing-library/react, NEITHER of which is a dependency of this repo — it
// has never executed in any CI lane or local runner. Runnable coverage for
// these behaviors lives in console/src/lib/component-interaction.test.tsx
// (repo-standard esbuild + node --test) and, for the pure grouping/parsing
// logic, in packages/console-core/src/*.test.ts via the node:test-backed
// shim (test-support/vitest-shim.ts). If you add vitest + RTL as real
// dependencies, wire this file into CI before trusting it.
import { fireEvent, render, screen } from "@testing-library/react";
import { vi } from "vitest";

import {
  groupConversationTimelineEntries,
  type ConversationFlowRunEntry,
  type ConversationTimelineEntry,
} from "@console-core";

import { FlowRunCard } from "./flow-run-card";

function memberTranscript() {
  const entries: ConversationTimelineEntry[] = [
    {
      id: "builder-message",
      kind: "message",
      variant: "plain",
      identity: { id: "builder", label: "Builder", role: "other" },
      text: "Implemented the shared flow-run presentation.",
    },
  ];
  return {
    conversationId: "builder-transcript",
    entries,
    groups: groupConversationTimelineEntries(entries),
    turnDiff: null,
    emptyState: null,
  };
}

function entryFixture(overrides: Partial<ConversationFlowRunEntry> = {}): ConversationFlowRunEntry {
  return {
    id: "flow-run:release-crew",
    kind: "flow_run",
    identity: { id: "coordinator", label: "Coordinator", role: "assistant" },
    helperId: "helper-1",
    flowName: "Release crew",
    status: "running",
    rows: [
      {
        memberKey: "builder",
        label: "Builder",
        caption: "Implementing the shared component",
        status: "running",
        subView: memberTranscript(),
      },
    ],
    ...overrides,
  };
}

describe("FlowRunCard", () => {
  test.each(["idle", "queued", "running", "cancelling"] as const)(
    "keeps %s work expanded without a card-level disclosure",
    (status) => {
      const { container } = render(<FlowRunCard entry={entryFixture({ status })} />);

      expect(container.querySelector("[data-flow-run-card]")?.getAttribute("data-details-expanded"))
        .toBe("true");
      expect(screen.getByRole("region", { name: "Release crew details" })).toBeTruthy();
      expect(screen.queryByRole("button", { name: /details/i })).toBeNull();
      expect(screen.getByText("Builder")).toBeTruthy();
    },
  );

  // Audit fix: only COMPLETED runs compact by default — failed/stopped runs
  // keep details expanded (per-member failure detail matters most at the
  // moment of failure). See component-interaction.test.tsx for the runnable
  // pin of that behavior.
  test.each(["completed"] as const)(
    "defaults %s work to a compact accessible summary and reveals details on request",
    (status) => {
      const onMessageMember = vi.fn();
      const { container } = render(
        <FlowRunCard
          entry={entryFixture({
            status,
            outcome: "## Result\n\nThe crew left a concise durable outcome.",
          })}
          onMessageMember={onMessageMember}
        />,
      );
      const card = container.querySelector("[data-flow-run-card]");
      const disclosure = screen.getByRole("button", { name: "Show details" });
      const detailsId = disclosure.getAttribute("aria-controls");

      expect(card?.classList.contains("is-compact")).toBe(true);
      expect(card?.getAttribute("data-details-expanded")).toBe("false");
      expect(disclosure.getAttribute("aria-expanded")).toBe("false");
      expect(screen.queryByRole("region", { name: "Release crew details" })).toBeNull();
      expect(document.getElementById(detailsId || "")?.hidden).toBe(true);
      expect(screen.getByText("The crew left a concise durable outcome.")).toBeTruthy();

      fireEvent.click(disclosure);

      expect(card?.getAttribute("data-details-expanded")).toBe("true");
      expect(disclosure.textContent).toBe("Hide details");
      expect(disclosure.getAttribute("aria-expanded")).toBe("true");
      expect(screen.getByRole("region", { name: "Release crew details" }).id).toBe(detailsId);
      expect(screen.getByText("The crew left a concise durable outcome.")).toBeTruthy();

      fireEvent.click(screen.getByRole("button", { name: "Message Builder" }));
      expect(onMessageMember).toHaveBeenCalledWith("builder");
    },
  );

  test("collapses when active work completes and expands again when resumed", () => {
    const { container, rerender } = render(<FlowRunCard entry={entryFixture({ status: "running" })} />);

    expect(container.querySelector("[data-flow-run-card]")?.getAttribute("data-details-expanded"))
      .toBe("true");

    rerender(<FlowRunCard entry={entryFixture({ status: "completed" })} />);

    expect(container.querySelector("[data-flow-run-card]")?.getAttribute("data-details-expanded"))
      .toBe("false");
    expect(screen.getByRole("button", { name: "Show details" })).toBeTruthy();

    rerender(<FlowRunCard entry={entryFixture({ status: "running" })} />);

    expect(container.querySelector("[data-flow-run-card]")?.getAttribute("data-details-expanded"))
      .toBe("true");
    expect(screen.queryByRole("button", { name: /details/i })).toBeNull();
  });

  test("keeps a terminal outcome visible without adding an empty disclosure", () => {
    render(
      <FlowRunCard
        entry={entryFixture({
          status: "completed",
          rows: [],
          outcome: "The durable result stays in the transcript.",
        })}
      />,
    );

    expect(screen.getByText("The durable result stays in the transcript.")).toBeTruthy();
    expect(screen.queryByRole("button", { name: /details/i })).toBeNull();
  });

  test("keeps Resume available on an expanded restorable card without exposing Message", () => {
    // A legacy no-argument callback remains assignable; JavaScript safely
    // ignores the new helper and entry arguments supplied by FlowRunCard.
    const onRestore: () => void = vi.fn();
    const onMessageMember = vi.fn();
    render(
      <FlowRunCard
        entry={entryFixture({ status: "stopped", restorable: true })}
        onRestore={onRestore}
        onMessageMember={onMessageMember}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "Resume" }));
    expect(onRestore).toHaveBeenCalledTimes(1);

    expect(screen.getByRole("region", { name: "Release crew details" })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Hide details" }));
    expect(screen.queryByRole("region", { name: "Release crew details" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Message Builder" })).toBeNull();
    expect(onMessageMember).not.toHaveBeenCalled();
  });

  test("renders stopped state and non-expandable members as content with a unique message action", () => {
    const onMessageMember = vi.fn();
    const { container } = render(
      <FlowRunCard
        entry={entryFixture({
          status: "running",
          rows: [
            {
              memberKey: "reviewer",
              label: "Reviewer",
              caption: "Stopped by the operator",
              status: "stopped",
            },
          ],
        })}
        onMessageMember={onMessageMember}
      />,
    );

    expect(container.querySelector("[data-flow-run-card]")?.getAttribute("data-status"))
      .toBe("running");
    expect(screen.getByText("Stopped", { selector: ".cc-flow-run__member-status" })).toBeTruthy();
    expect(container.querySelector(".cc-flow-run__member-row")?.tagName).toBe("DIV");
    expect(screen.queryByRole("button", { name: /reviewer.*stopped/i })).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: "Message Reviewer" }));
    expect(onMessageMember).toHaveBeenCalledWith("reviewer");
  });

  test("distinguishes queued and cancelling agents from idle or completed work", () => {
    const { container } = render(
      <FlowRunCard
        entry={entryFixture({
          status: "cancelling",
          rows: [
            { memberKey: "builder", label: "Builder", caption: "Waiting for a safe stop", status: "cancelling" },
            { memberKey: "reviewer", label: "Reviewer", caption: "Waiting to start", status: "queued" },
            { memberKey: "observer", label: "Observer", caption: "Ready for work", status: "idle" },
          ],
        })}
      />,
    );

    expect(container.querySelector("[data-flow-run-card]")?.getAttribute("data-status"))
      .toBe("cancelling");
    expect(screen.getAllByText("Stopping")).toHaveLength(2);
    expect(screen.getByText("Queued", { selector: ".cc-flow-run__member-status" })).toBeTruthy();
    expect(screen.getByText("Idle", { selector: ".cc-flow-run__member-status" })).toBeTruthy();
  });

  test("keeps status in an expandable row's accessible name and labels its bounded transcript region", () => {
    const { container } = render(<FlowRunCard entry={entryFixture()} />);
    const rowButton = screen.getByRole("button", { name: /builder.*working/i });
    const detailId = rowButton.getAttribute("aria-controls");

    expect(rowButton.getAttribute("aria-expanded")).toBe("false");
    expect(screen.getByText("Working", { selector: ".cc-flow-run__member-status" }).textContent)
      .toBe("Working");

    fireEvent.click(rowButton);

    expect(rowButton.getAttribute("aria-expanded")).toBe("true");
    const region = screen.getByRole("region", { name: "Builder transcript" });
    expect(region.id).toBe(detailId);
    expect(region.getAttribute("tabindex")).toBe("0");
    expect(container.querySelector(".cc-flow-run__member-detail")?.textContent)
      .toContain("Implemented the shared flow-run presentation.");
  });
});
