import { fireEvent, render, screen } from "@testing-library/react";
import type React from "react";

import { PendingStack } from "./pending-stack";
import type { PendingItem } from "./pending-stack";

const baseItem: PendingItem = {
  id: "queued-1",
  text: "Ask the peer agent to keep the conversation going.",
  addedAt: Date.now(),
};

function renderPendingStack(overrides: Partial<React.ComponentProps<typeof PendingStack>> = {}) {
  const props: React.ComponentProps<typeof PendingStack> = {
    items: [baseItem],
    agentBusy: true,
    onSteer: vi.fn(),
    onRetry: vi.fn(),
    onTrash: vi.fn(),
    onEdit: vi.fn(),
    onCommitEdit: vi.fn(),
    onCancelEdit: vi.fn(),
    onReorder: vi.fn(),
    onClearAll: vi.fn(),
    onToggleExpand: vi.fn(),
    ...overrides,
  };

  return {
    props,
    ...render(<PendingStack {...props} />),
  };
}

describe("PendingStack", () => {
  test("renders the shared pending-message queue affordance", () => {
    renderPendingStack();

    expect(screen.getByTestId("pending-stack")).toBeInTheDocument();
    expect(screen.getByText("Queue")).toBeInTheDocument();
    expect(screen.getByText("01")).toBeInTheDocument();
    expect(screen.getByText("Busy")).toBeInTheDocument();
    expect(screen.getByTestId("pending-item:queued-1")).toHaveTextContent(baseItem.text);
  });

  test("routes queue actions through host callbacks", () => {
    const { props } = renderPendingStack();

    fireEvent.click(screen.getByTestId("pending-steer:queued-1"));
    fireEvent.click(screen.getByTestId("pending-edit:queued-1"));
    fireEvent.click(screen.getByTestId("pending-trash:queued-1"));

    expect(props.onSteer).toHaveBeenCalledWith("queued-1");
    expect(props.onEdit).toHaveBeenCalledWith("queued-1");
    expect(props.onTrash).toHaveBeenCalledWith("queued-1");
  });

  test("surfaces a durable delivery failure as an explicit retry", () => {
    const { props } = renderPendingStack({
      agentBusy: false,
      items: [{ ...baseItem, manualRetryRequired: true }],
    });

    expect(screen.getAllByText("Needs retry")).toHaveLength(2);
    const retry = screen.getByRole("button", { name: "Retry delivery" });
    expect(retry).toHaveTextContent("Retry");
    fireEvent.click(retry);
    expect(props.onRetry).toHaveBeenCalledWith("queued-1");
    expect(props.onSteer).not.toHaveBeenCalled();
  });

  test("returns null when there is no queued work", () => {
    const { container } = renderPendingStack({ items: [] });

    expect(container).toBeEmptyDOMElement();
  });
});
