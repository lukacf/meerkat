import React from "react";
import { act, fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { VoiceSessionSnapshot } from "../lib/voice-session";
import { ChatPane, TRANSCRIPT_WINDOW_STEP, TRANSCRIPT_WINDOW_TURNS } from "./ChatPane";
import { TALK_READY_STATUS, VoiceBar, VoiceButton } from "./VoiceBar";

const activeState: VoiceSessionSnapshot = {
  phase: "active",
  target: { identity: "identity:alpha", label: "Alpha" },
  microphoneMuted: false,
  speakerMuted: false,
  error: null,
  notice: null,
};
const sampleWaveform = vi.fn();
const controls = {
  sampleWaveform,
  onClose: vi.fn(),
  onToggleMicrophone: vi.fn(),
  onToggleSpeaker: vi.fn(),
};

describe("voice controls", () => {
  it("labels the persistent voice agent and exposes independent mute controls", () => {
    const view = render(<VoiceBar state={activeState} {...controls} />);
    expect(screen.getByRole("region", { name: "Voice with Alpha" })).toBeInTheDocument();
    expect(view.container.querySelectorAll("canvas")).toHaveLength(2);
    fireEvent.click(screen.getByRole("button", { name: "Mute microphone" }));
    fireEvent.click(screen.getByRole("button", { name: "Mute speakers" }));
    expect(controls.onToggleMicrophone).toHaveBeenCalledOnce();
    expect(controls.onToggleSpeaker).toHaveBeenCalledOnce();

    view.rerender(<VoiceBar state={{ ...activeState, microphoneMuted: true, speakerMuted: true }} {...controls} />);
    expect(screen.getByRole("button", { name: "Unmute microphone" })).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByRole("button", { name: "Unmute speakers" })).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByRole("status")).toHaveTextContent("Microphone muted");
    fireEvent.click(screen.getByRole("button", { name: "End voice conversation" }));
    expect(controls.onClose).toHaveBeenCalledOnce();
  });

  it("can cancel microphone permission or connection without enabling media controls", () => {
    const view = render(<VoiceBar state={{ ...activeState, phase: "requesting" }} {...controls} />);
    expect(screen.getByRole("status")).toHaveTextContent("Allow microphone access");
    expect(screen.getByRole("button", { name: "Mute microphone" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Mute speakers" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "End voice conversation" })).toBeEnabled();
    view.rerender(<VoiceBar state={{ ...activeState, phase: "connecting" }} {...controls} />);
    expect(screen.getByRole("status")).toHaveTextContent("Connecting");
    view.rerender(<VoiceBar state={{ ...activeState, phase: "connecting", connectionStage: "opening" }} {...controls} />);
    expect(screen.getByRole("status")).toHaveTextContent("Starting voice");
    view.rerender(<VoiceBar state={{ ...activeState, phase: "connecting", connectionStage: "recovery" }} {...controls} />);
    expect(screen.getByRole("status")).toHaveTextContent("Reconnecting");
  });

  it("shows a reconnect grace on an active call as Reconnecting without an error and with controls enabled", () => {
    render(<VoiceBar state={{ ...activeState, reconnecting: true }} {...controls} />);
    expect(screen.getByRole("status")).toHaveTextContent("Reconnecting");
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(screen.getByTestId("voice-bar")).toHaveAttribute("data-phase", "active");
    expect(screen.getByRole("button", { name: "Mute microphone" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "End voice conversation" })).toBeEnabled();
  });

  it.each([
    ["capturing", "being read"],
    ["generating", "being summarised"],
    ["delivering", "being sent to the voice model"],
  ] as const)("says you can talk while context is %s, with the stage as a secondary hint", (stage, detail) => {
    const view = render(<VoiceBar state={{
      ...activeState, contextPreparation: { phase: "preparing", stage },
    }} {...controls} />);
    const status = screen.getByRole("status");
    expect(status).toHaveTextContent(TALK_READY_STATUS);
    expect(status).toHaveAttribute("data-talk-ready", "true");
    expect(status).toHaveTextContent("Context arriving");
    const hint = screen.getByText("Context arriving");
    expect(hint).toHaveAttribute("data-context", "preparing");
    expect(hint).toHaveAttribute("data-context-stage", stage);
    expect(hint).toHaveAttribute("title", expect.stringContaining(detail));
    expect(hint).toHaveAttribute("title", expect.stringContaining("You can talk now"));
    expect(screen.getByTestId("voice-bar")).toHaveAttribute("data-phase", "active");
    expect(screen.getByRole("button", { name: "Mute microphone" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "Mute speakers" })).toBeEnabled();
    expect(view.container.querySelectorAll("canvas")).toHaveLength(2);
  });

  it("marks talk readiness only for an unmuted, connected active call", () => {
    const view = render(<VoiceBar state={activeState} {...controls} />);
    expect(screen.getByRole("status")).toHaveTextContent(TALK_READY_STATUS);
    expect(screen.getByRole("status")).toHaveAttribute("data-talk-ready", "true");
    view.rerender(<VoiceBar state={{ ...activeState, microphoneMuted: true }} {...controls} />);
    expect(screen.getByRole("status")).toHaveTextContent("Microphone muted");
    expect(screen.getByRole("status")).not.toHaveAttribute("data-talk-ready");
    view.rerender(<VoiceBar state={{ ...activeState, reconnecting: true }} {...controls} />);
    expect(screen.getByRole("status")).not.toHaveAttribute("data-talk-ready");
    view.rerender(<VoiceBar state={{ ...activeState, phase: "connecting", connectionStage: "transport" }} {...controls} />);
    expect(screen.getByRole("status")).toHaveTextContent("Connecting audio");
    expect(screen.getByRole("status")).not.toHaveAttribute("data-talk-ready");
  });

  it("describes acknowledged context without claiming model recall or speech completion", () => {
    render(<VoiceBar state={{
      ...activeState, contextPreparation: { phase: "provider_acknowledged" },
    }} {...controls} />);
    expect(screen.getByRole("status")).toHaveTextContent(TALK_READY_STATUS);
    expect(screen.getByRole("status")).toHaveTextContent("Context supplied");
    expect(screen.getByText("Context supplied")).toHaveAttribute("data-context", "provider_acknowledged");
    expect(screen.getByText("Context supplied")).toHaveAttribute(
      "title", expect.stringContaining("does not confirm recall or speech completion"),
    );
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("exposes context failures without hiding usable audio and clears them after voice ends", () => {
    const view = render(<VoiceBar state={{
      ...activeState, contextPreparation: { phase: "failed", reason: "timed_out" },
    }} {...controls} />);
    expect(screen.getByRole("status")).toHaveTextContent(TALK_READY_STATUS);
    expect(screen.getByRole("status")).toHaveTextContent("Context unavailable");
    expect(screen.getByText("Context unavailable")).toHaveAttribute("data-context", "failed");
    expect(screen.getByRole("alert")).toHaveTextContent("Context preparation timed out.");
    expect(screen.getByRole("alert")).toHaveTextContent("End voice and start again to retry.");
    expect(screen.getByRole("button", { name: "Mute microphone" })).toBeEnabled();
    view.rerender(<VoiceBar state={{
      ...activeState, phase: "closing", contextPreparation: { phase: "failed", reason: "timed_out" },
    }} {...controls} />);
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("distinguishes an unknown status from missing or failed context", () => {
    const view = render(<VoiceBar state={{ ...activeState, contextPreparation: null }} {...controls} />);
    expect(screen.getByRole("status")).toHaveTextContent(TALK_READY_STATUS);
    expect(screen.getByRole("status")).toHaveTextContent("Context arriving");
    expect(screen.getByText("Context arriving")).toHaveAttribute("data-context", "checking");
    expect(screen.getByText("Context arriving")).not.toHaveAttribute("data-context-stage");
    view.rerender(<VoiceBar state={{
      ...activeState, contextPreparation: null, contextStatusError: "Agent context status is unavailable. Checking again automatically.",
    }} {...controls} />);
    expect(screen.getByRole("status")).toHaveTextContent("Context status unavailable");
    expect(screen.getByText("Context status unavailable")).toHaveAttribute("data-context", "status_unavailable");
    expect(screen.getByRole("alert")).toHaveTextContent("Checking again automatically");
    view.rerender(<VoiceBar state={{
      ...activeState, contextPreparation: { phase: "not_requested" },
    }} {...controls} />);
    expect(screen.getByRole("status")).toHaveTextContent("No summary pending");
    expect(screen.getByText("No summary pending")).toHaveAttribute(
      "title", "No concurrent context preparation was requested for this call.",
    );
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("shows startup errors and silence termination as dismissible non-modal messages", () => {
    const view = render(<VoiceBar state={{ ...activeState, phase: "error", error: "Microphone access was denied." }} {...controls} />);
    expect(screen.getByRole("alert")).toHaveTextContent("Microphone access was denied.");
    expect(view.container.querySelectorAll("canvas")).toHaveLength(0);
    expect(screen.getByRole("button", { name: "Dismiss voice" })).toBeEnabled();
    view.rerender(<VoiceBar state={{ ...activeState, phase: "idle", notice: "Voice ended after 15 minutes of silence." }} {...controls} />);
    expect(screen.getByText("Voice ended after 15 minutes of silence.")).toHaveAttribute("role", "status");
  });

  it("names the icon action and exposes its active state", () => {
    const onClick = vi.fn();
    const view = render(<VoiceButton agentLabel="Alpha" onClick={onClick} />);
    fireEvent.click(screen.getByRole("button", { name: "Start voice with Alpha" }));
    expect(onClick).toHaveBeenCalledOnce();
    view.rerender(<VoiceButton agentLabel="Alpha" active onClick={onClick} />);
    expect(screen.getByRole("button", { name: "End voice with Alpha" })).toHaveAttribute("aria-pressed", "true");
  });

  it("shows a busy, still-usable checking state until readiness is confirmed", () => {
    const onClick = vi.fn();
    const view = render(<VoiceButton agentLabel="Alpha" checking onClick={onClick} />);
    const checking = screen.getByRole("button", { name: "Start voice with Alpha (checking availability)" });
    expect(checking).toHaveAttribute("aria-busy", "true");
    expect(checking).toHaveAttribute("data-readiness", "checking");
    expect(checking).toHaveClass("composer__voice--checking");
    expect(checking).toBeEnabled();
    fireEvent.click(checking);
    expect(onClick).toHaveBeenCalledOnce();
    view.rerender(<VoiceButton agentLabel="Alpha" onClick={onClick} />);
    const confirmed = screen.getByRole("button", { name: "Start voice with Alpha" });
    expect(confirmed).not.toHaveAttribute("aria-busy");
    expect(confirmed).toHaveAttribute("data-readiness", "confirmed");
    expect(confirmed).not.toHaveClass("composer__voice--checking");
  });
});

describe("voice and text composer", () => {
  function props(label: string) {
    return {
      agent: { agent_id: label, member_id: label, label, kind: "identity", affordances: { can_send_message: true } },
      agentLabel: label,
      identity: `identity:${label.toLowerCase()}`,
      entries: [],
      phase: null,
      draft: "Keep working on the text task",
      sending: false,
      staged: [],
      onDraftChange: vi.fn(),
      onStagedChange: vi.fn(),
      onSend: vi.fn().mockResolvedValue(true),
      voiceSlot: <VoiceBar state={activeState} {...controls} />,
    };
  }

  it("keeps Alpha's voice controls above Beta's usable text input after navigation", () => {
    const alpha = props("Alpha");
    const view = render(<ChatPane {...alpha} onVoiceToggle={vi.fn()} voiceActive />);
    expect(screen.getByRole("textbox")).toHaveAttribute("placeholder", "Message Alpha (background agent)…");
    expect(screen.getByText("· text to background agent")).toBeInTheDocument();
    const beta = props("Beta");
    view.rerender(<ChatPane {...beta} onVoiceToggle={vi.fn()} />);
    expect(screen.getByRole("region", { name: "Voice with Alpha" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Start voice with Beta" })).toBeInTheDocument();
    const input = screen.getByRole("textbox");
    expect(input).toBeEnabled();
    expect(input).toHaveAttribute("placeholder", "Message Beta…");
    expect(screen.queryByText("· text to background agent")).not.toBeInTheDocument();
    expect(screen.getByTestId("voice-bar").compareDocumentPosition(input) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    fireEvent.click(screen.getByTestId("chat-send:identity:beta"));
    expect(beta.onSend).toHaveBeenCalledWith([], "Keep working on the text task");
  });

  it("owns the live draft locally, publishes it debounced, and adopts parent resets", async () => {
    vi.useFakeTimers();
    try {
      const pane = props("Alpha");
      const view = render(<ChatPane {...pane} />);
      const input = screen.getByRole("textbox") as HTMLTextAreaElement;
      expect(input).toHaveValue("Keep working on the text task");
      fireEvent.change(input, { target: { value: "Keep working on the text task now" } });
      // Keystrokes stay local until the debounce elapses.
      expect(input).toHaveValue("Keep working on the text task now");
      expect(pane.onDraftChange).not.toHaveBeenCalled();
      vi.advanceTimersByTime(400);
      expect(pane.onDraftChange).toHaveBeenCalledWith("Keep working on the text task now");
      // The exact live text travels with the send, not the persisted copy.
      fireEvent.click(screen.getByTestId("chat-send:identity:alpha"));
      expect(pane.onSend).toHaveBeenCalledWith([], "Keep working on the text task now");
      await act(async () => {
        await Promise.resolve();
      });
      expect(input).toHaveValue("");
      // The parent clearing the persisted draft after the send empties the box.
      view.rerender(<ChatPane {...pane} draft="" />);
      expect(input).toHaveValue("");
      // A parent-provided draft (panel navigation) replaces the local value.
      view.rerender(<ChatPane {...pane} draft="Restored draft" />);
      expect(input).toHaveValue("Restored draft");
      // Blur publishes immediately without waiting for the debounce.
      fireEvent.change(input, { target: { value: "Restored draft edited" } });
      fireEvent.blur(input);
      expect(pane.onDraftChange).toHaveBeenLastCalledWith("Restored draft edited");
      // Sending before the debounce fires must not re-publish the sent
      // text as a draft afterwards, and the box empties even though the
      // parent's persisted copy never held it.
      fireEvent.change(input, { target: { value: "Sent in a hurry" } });
      fireEvent.click(screen.getByTestId("chat-send:identity:alpha"));
      await act(async () => {
        await Promise.resolve();
      });
      expect(pane.onSend).toHaveBeenLastCalledWith([], "Sent in a hurry");
      expect(input).toHaveValue("");
      vi.advanceTimersByTime(400);
      expect(pane.onDraftChange).not.toHaveBeenCalledWith("Sent in a hurry");
    } finally {
      vi.useRealTimers();
    }
  });

  it("mounts only the newest transcript window and reveals earlier turns on demand", () => {
    const USER = { id: "user", label: "You", role: "user" as const };
    const AGENT = { id: "agent", label: "Alpha", role: "assistant" as const };
    const entries = [];
    for (let i = 0; i < 300; i += 1) {
      const createdAt = new Date(1_700_000_000_000 + i * 60_000).toISOString();
      entries.push({ id: `u-${i}`, kind: "message", variant: "plain", identity: USER, createdAt, text: `question ${i}` });
      entries.push({ id: `a-${i}`, kind: "message", variant: "plain", identity: AGENT, createdAt, text: `answer ${i}` });
    }
    const pane = props("Alpha");
    const view = render(<ChatPane {...pane} entries={entries as never} />);
    const mounted = () => view.container.querySelectorAll("[data-chat-turn-index]");
    expect(mounted()).toHaveLength(TRANSCRIPT_WINDOW_TURNS);
    // Absolute indexes survive windowing: the newest turn is still turn 300.
    expect(view.container.querySelector('[data-chat-turn-index="299"]')).not.toBeNull();
    expect(view.container.querySelector('[data-chat-turn-index="0"]')).toBeNull();
    const bodyText = () => view.container.querySelector(".conv__body")?.textContent ?? "";
    expect(bodyText()).toContain("answer 299");
    expect(bodyText()).not.toContain("question 0");
    fireEvent.click(screen.getByTestId("chat-reveal-earlier:identity:alpha"));
    expect(mounted()).toHaveLength(TRANSCRIPT_WINDOW_TURNS + TRANSCRIPT_WINDOW_STEP);
    // The per-identity log trimmed past the anchored turn (the first 70
    // turns are gone): the window keeps its size at the oldest retained
    // turns instead of snapping back to the 120-turn tail.
    view.rerender(<ChatPane {...pane} entries={entries.slice(70 * 2) as never} />);
    expect(mounted()).toHaveLength(230);
    expect(view.container.querySelector('[data-chat-turn-index="0"]')).not.toBeNull();
    expect(bodyText()).toContain("question 70");
    // Reaching the first turn removes the reveal affordance and shows all.
    view.rerender(<ChatPane {...pane} entries={entries as never} />);
    fireEvent.click(screen.getByTestId("chat-reveal-earlier:identity:alpha"));
    expect(mounted()).toHaveLength(300);
    expect(screen.queryByTestId("chat-reveal-earlier:identity:alpha")).not.toBeInTheDocument();
    expect(bodyText()).toContain("question 0");
  });

  it("does not offer voice without the authenticated capability callback", () => {
    render(<ChatPane {...props("Alpha")} voiceSlot={null} />);
    expect(screen.queryByTestId("voice-start")).not.toBeInTheDocument();
    expect(screen.getByRole("textbox")).toBeEnabled();
  });

  it.each(["read-only", "send-denied"])("does not offer voice in a %s pane", (restriction) => {
    const pane = props("Alpha");
    render(
      <ChatPane
        {...pane}
        agent={{ ...pane.agent, affordances: { can_send_message: restriction !== "send-denied" } }}
        readOnly={restriction === "read-only"}
        accessEnforcing
        onVoiceToggle={vi.fn()}
      />,
    );
    expect(screen.queryByTestId("voice-start")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "End voice conversation" })).toBeEnabled();
  });
});
