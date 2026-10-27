// NOTE (2026-07-14 audit): this file is written against vitest globals and
// @testing-library/react, NEITHER of which is a dependency of this repo — it
// has never executed in any CI lane or local runner. Runnable coverage for
// these behaviors lives in console/src/lib/component-interaction.test.tsx
// (repo-standard esbuild + node --test) and, for the pure grouping/parsing
// logic, in packages/console-core/src/*.test.ts via the node:test-backed
// shim (test-support/vitest-shim.ts). If you add vitest + RTL as real
// dependencies, wire this file into CI before trusting it.
import { fireEvent, render, screen, waitFor } from "@testing-library/react";

import type { ConversationTimelineGroup } from "@console-core";

import { ConversationMessageGroup } from "./conversation-message-group";

function Icon({ name }: { name: string; className?: string }) {
  return <span>{name}</span>;
}

describe("ConversationMessageGroup", () => {
  test("passes the shared icon renderer through to user message copy affordances", () => {
    const group: ConversationTimelineGroup = {
      id: "user-group",
      identity: { id: "user", label: "You", role: "user" },
      copyText: "Create an arbitrary markdown table.",
      entries: [{
        id: "user-1",
        kind: "message",
        variant: "plain",
        identity: { id: "user", label: "You", role: "user" },
        text: "Create an arbitrary markdown table.",
      }],
    };

    render(<ConversationMessageGroup group={group} Icon={Icon} />);

    expect(screen.getByRole("button", { name: /copy message/i })).toHaveTextContent("i-copy");
    expect(screen.queryByText(/^Copy$/)).not.toBeInTheDocument();
  });

  test("hides the outer copy button when a single rich entry already has targeted copy controls", () => {
    const group: ConversationTimelineGroup = {
      id: "assistant-group",
      identity: { id: "assistant", label: "Assistant", role: "assistant" },
      copyText: "const ready = true;",
      entries: [{
        id: "assistant-code",
        kind: "message",
        variant: "rich",
        identity: { id: "assistant", label: "Assistant", role: "assistant" },
        text: "const ready = true;",
        blocks: [{
          type: "code",
          language: "ts",
          body: "const ready = true;",
        }],
      }],
    };

    render(<ConversationMessageGroup group={group} Icon={Icon} />);

    expect(screen.queryByRole("button", { name: /copy response/i })).not.toBeInTheDocument();
  });

  test("keeps response copy when targeted code is part of a larger answer", () => {
    const group: ConversationTimelineGroup = {
      id: "assistant-mixed-code-group",
      identity: { id: "assistant", label: "Assistant", role: "assistant" },
      copyText: "Use this example.\n\nconst ready = true;",
      entries: [{
        id: "assistant-mixed-code",
        kind: "message",
        variant: "rich",
        identity: { id: "assistant", label: "Assistant", role: "assistant" },
        text: "Use this example.\n\nconst ready = true;",
        blocks: [
          { type: "paragraph", text: "Use this example." },
          { type: "code", language: "ts", body: "const ready = true;" },
        ],
      }],
    };

    render(<ConversationMessageGroup group={group} Icon={Icon} />);

    expect(screen.getByRole("button", { name: /copy response/i })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /copy code/i })).toBeInTheDocument();
  });

  test("renders participant identity metadata and keeps a grouped copy affordance", () => {
    const group: ConversationTimelineGroup = {
      id: "builder-group",
      identity: {
        id: "builder",
        label: "Builder",
        role: "other",
        presentation: "participant",
        showLabel: true,
        meta: "implementation",
        avatarLabel: "BLD",
      },
      copyText: "I normalized the transcript.\n\nI removed the app-global diff stat classes.",
      entries: [
        {
          id: "builder-1",
          kind: "message",
          variant: "plain",
          identity: {
            id: "builder",
            label: "Builder",
            role: "other",
            presentation: "participant",
            showLabel: true,
          },
          text: "I normalized the transcript.",
        },
        {
          id: "builder-2",
          kind: "message",
          variant: "plain",
          identity: {
            id: "builder",
            label: "Builder",
            role: "other",
            presentation: "participant",
            showLabel: true,
          },
          text: "I removed the app-global diff stat classes.",
        },
      ],
    };

    render(<ConversationMessageGroup group={group} Icon={Icon} />);

    expect(screen.getByText("Builder")).toBeInTheDocument();
    expect(screen.getByText("implementation")).toBeInTheDocument();
    expect(screen.getByText("BLD")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /copy response/i })).toHaveTextContent("i-copy");
  });

  test("places response copy at the end of a long mixed response", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText },
    });
    const group: ConversationTimelineGroup = {
      id: "assistant-mixed-group",
      identity: { id: "assistant", label: "Assistant", role: "assistant" },
      copyText: "$ peers\n\nA long response follows the tool result.",
      entries: [
        {
          id: "assistant-tool",
          kind: "message",
          variant: "rich",
          identity: { id: "assistant", label: "Assistant", role: "assistant" },
          text: "$ peers",
          blocks: [{
            type: "tool-call",
            toolCallId: "peers-1",
            name: "peers",
            arguments: "{}",
            result: "[]",
            status: "success",
          }],
        },
        {
          id: "assistant-final",
          kind: "message",
          variant: "rich",
          identity: { id: "assistant", label: "Assistant", role: "assistant" },
          text: "A long response follows the tool result.",
          blocks: [{ type: "paragraph", text: "A long response follows the tool result." }],
        },
      ],
    };

    const { container } = render(<ConversationMessageGroup group={group} Icon={Icon} />);
    const body = container.querySelector(".cc-message-group__body");
    const actions = container.querySelector(".cc-message-group__actions");
    const copy = screen.getByRole("button", { name: /copy response/i });

    expect(actions).not.toBeNull();
    expect(actions).toContainElement(copy);
    expect(body?.nextElementSibling).toBe(actions);

    fireEvent.click(copy);
    await waitFor(() => {
      expect(writeText).toHaveBeenCalledWith("A long response follows the tool result.");
    });
    expect(writeText).not.toHaveBeenCalledWith(expect.stringContaining("$ peers"));
  });
});
