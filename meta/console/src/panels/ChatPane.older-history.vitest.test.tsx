import React from "react";
import { cleanup, fireEvent, render } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { AUTOMATIC } from "../../../packages/console-core/src/mob-door";
import type { ConsoleAgent } from "../types";
import { ChatPane } from "./ChatPane";

// Older history is read two ways: when the top of the chat comes into view
// (a scroll, which the console cannot tell from one of its own) and when a
// person presses "Load older history". The first is the console's own read,
// and says so, so that it is not made for an agent whose mob is asleep; the
// second is a person's act, and says nothing.

afterEach(() => {
  cleanup();
});

const agent: ConsoleAgent = {
  agent_id: "support/reviewer",
  member_id: "support/reviewer",
  identity: "support/reviewer",
  label: "Support reviewer",
  kind: "mob_agent",
  role: "analyst",
  mob_id: "support",
};
const noop = () => {};

function chat(onLoadOlder: (by?: unknown) => void) {
  return render(
    <ChatPane
      agent={agent}
      agentLabel="Support reviewer"
      identity="support/reviewer"
      entries={[]}
      phase={null}
      draft=""
      sending={false}
      staged={[]}
      onDraftChange={noop}
      onStagedChange={noop}
      onSend={() => false}
      hasOlderHistory
      onLoadOlder={onLoadOlder}
    />,
  );
}

describe("older history of a chat", () => {
  it("is asked for as the console's own read when the top of the chat comes into view", () => {
    const onLoadOlder = vi.fn();
    const view = chat(onLoadOlder);
    const body = view.container.querySelector(".conv__body") as HTMLElement;
    fireEvent.scroll(body, { target: { scrollTop: 0 } });
    expect(onLoadOlder.mock.calls).toEqual([[AUTOMATIC]]);
  });

  it("is asked for as a person's act when the person presses the button", () => {
    const onLoadOlder = vi.fn();
    const view = chat(onLoadOlder);
    fireEvent.click(view.getByText("Load older history"));
    expect(onLoadOlder.mock.calls).toEqual([[undefined]]);
  });
});
