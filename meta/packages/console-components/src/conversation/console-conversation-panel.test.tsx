import { fireEvent, render, screen } from "@testing-library/react";
import { vi } from "vitest";

import type { ConversationTimelineEntry } from "@console-core";

import { ConsoleConversationPanel } from "./console-conversation-panel";

function Icon({ name, className }: { name: string; className?: string }) {
  return <span className={className}>{name}</span>;
}

describe("ConsoleConversationPanel", () => {
  test("renders the shared conversation and composer shell", () => {
    const entries: ConversationTimelineEntry[] = [{
      id: "message-1",
      kind: "message",
      variant: "plain",
      identity: { id: "user", label: "You", role: "user", presentation: "user" },
      text: "Please do the work.",
    }];
    const onDraftChange = vi.fn();
    const onSend = vi.fn();

    render(
      <ConsoleConversationPanel
        Icon={Icon}
        agent={null}
        agentLabel="Thread agent"
        identity="agent-1"
        entries={entries}
        draft="Ship it"
        inputId="conversation-composer:agent-1"
        submitButtonId="conversation-send:agent-1"
        modelLabel="GPT-5.5"
        reasoningLabel="Medium"
        executionModeLabel="Local project"
        branchLabel="main"
        permissionsLabel="Full access"
        onDraftChange={onDraftChange}
        onSend={onSend}
      />,
    );

    expect(screen.getByTestId("conversation-pane:agent-1")).toHaveClass("cc-conversation-panel");
    expect(screen.getByText("Please do the work.")).toBeInTheDocument();
    expect(screen.getByRole("textbox")).toHaveValue("Ship it");

    fireEvent.click(screen.getByTitle("Send to Thread agent"));

    expect(onSend).toHaveBeenCalledWith("Ship it");
  });
});
