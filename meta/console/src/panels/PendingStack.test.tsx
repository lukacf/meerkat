import { act, render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ACCEPTANCE_NOTICE_GRACE_MS, PendingStack, type PendingItem } from "./PendingStack";
import {
  beginConsoleSendAttempt,
  createConsoleSendAttempt,
  finishConsoleSendAttempt,
} from "../../../packages/console-core/src/send-attempt";

const noop = () => {};
const handlers = {
  onSteer: noop, onRetry: noop, onReconcile: noop, onRemoveContext: noop, onReorderContext: noop,
  onTrash: noop, onEdit: noop, onCommitEdit: noop, onCancelEdit: noop, onReorder: noop,
  onClearAll: noop, onToggleExpand: noop,
};

const draft = (id = "pmsg-1"): PendingItem => createConsoleSendAttempt({
  id, scope: "scope", destination: "agent", origin: "console:panel", idempotencyKey: `key-${id}`,
  text: "hello there", now: Date.now(),
});
const attempting = (item: PendingItem, retryRejected = false): PendingItem =>
  beginConsoleSendAttempt(item, { owner: "tab", now: Date.now(), handlingMode: "queue", retryRejected });
const accepted = (item: PendingItem): PendingItem =>
  finishConsoleSendAttempt(item, { state: "accepted", interactionId: "interaction-1" });
const rejected = (item: PendingItem): PendingItem =>
  finishConsoleSendAttempt(item, { state: "definitely-rejected", error: "The request was refused.", kind: "refused" });

function mount(items: PendingItem[], directSendIds?: ReadonlySet<string>) {
  const view = render(<PendingStack items={items} agentLabel="Queue agent" agentBusy={false} directSendIds={directSendIds} {...handlers} />);
  return {
    view,
    update: (next: PendingItem[]) =>
      view.rerender(<PendingStack items={next} agentLabel="Queue agent" agentBusy={false} directSendIds={directSendIds} {...handlers} />),
  };
}
const advance = (ms: number) => act(() => { vi.advanceTimersByTime(ms); });
const notice = () => document.querySelector(".needs-acceptance");

beforeEach(() => { vi.useFakeTimers({ now: 1_000_000 }); });
afterEach(() => { vi.useRealTimers(); });

describe("PendingStack acceptance notice grace", () => {
  it("never renders a normal send that is accepted inside the grace period", () => {
    const row = draft();
    const { view, update } = mount([row], new Set([row.id]));
    expect(view.queryByTestId("pending-stack")).toBeNull();
    const inFlight = attempting(row);
    update([inFlight]);
    expect(view.queryByTestId("pending-stack")).toBeNull();
    advance(500);
    update([accepted(inFlight)]);
    expect(view.queryByTestId("pending-stack")).toBeNull();
    update([]);
    advance(ACCEPTANCE_NOTICE_GRACE_MS * 2);
    expect(view.queryByTestId("pending-stack")).toBeNull();
    expect(view.queryByText("Sending")).toBeNull();
  });

  it("shows the unchanged needs-acceptance UI once a send is still attempting after the grace period", () => {
    const inFlight = attempting(draft());
    const { view } = mount([inFlight]);
    expect(view.queryByTestId("pending-stack")).toBeNull();
    advance(1_900);
    expect(view.queryByText("Sending")).toBeNull();
    advance(200);
    expect(view.getByText("Sending")).toBeTruthy();
    expect(view.getByText("Waiting for the server to confirm.")).toBeTruthy();
    expect(notice()).not.toBeNull();
  });

  it("shows a settled failure immediately", () => {
    const row = draft();
    const { view, update } = mount([attempting(row)], new Set([row.id]));
    advance(300);
    update([rejected(attempting(row))]);
    expect(view.getByTestId("pending-stack")).toBeTruthy();
    expect(view.getByText("Not sent: this message never reached Queue agent.")).toBeTruthy();
  });

  it("renders a draft queued behind a busy agent at once", () => {
    const { view } = mount([draft()]);
    expect(view.getByText("Queued")).toBeTruthy();
  });

  it("keeps a retried row visible instead of hiding it again", () => {
    const row = draft();
    const failed = rejected(attempting(row));
    const { view, update } = mount([attempting(row)]);
    advance(ACCEPTANCE_NOTICE_GRACE_MS + 100);
    update([failed]);
    update([attempting(failed, true)]);
    expect(view.getByText("Sending")).toBeTruthy();
  });

  it("leaves no grace timer behind on unmount", () => {
    const { view } = mount([attempting(draft())]);
    expect(vi.getTimerCount()).toBeGreaterThan(0);
    view.unmount();
    expect(vi.getTimerCount()).toBe(0);
  });
});
