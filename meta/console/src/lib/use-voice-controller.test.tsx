import { renderHook } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
import { createVoiceSession, type VoiceSession, type VoiceSessionSnapshot } from "./voice-session";
import { useVoiceController } from "./use-voice-controller";

vi.mock("./voice-session", () => ({ createVoiceSession: vi.fn() }));
const create = vi.mocked(createVoiceSession);
const sessions: VoiceSession[] = [];
const idle: VoiceSessionSnapshot = {
  phase: "idle", target: null, microphoneMuted: false,
  speakerMuted: false, error: null, notice: null,
};

beforeEach(() => {
  sessions.length = 0;
  create.mockReset();
  create.mockImplementation(() => {
    const session: VoiceSession = {
      subscribe: () => () => {},
      getSnapshot: () => idle,
      start: vi.fn(async () => {}),
      close: vi.fn(async () => {}),
      toggleMicrophone: vi.fn(),
      toggleSpeaker: vi.fn(),
      dispose: vi.fn(),
      sampleWaveform: vi.fn(),
    };
    sessions.push(session);
    return session;
  });
});

it("creates a live controller after StrictMode effect replay instead of reusing a disposed one", () => {
  const view = renderHook(() => useVoiceController("/gateway"), {
    reactStrictMode: true,
  });
  expect(sessions).toHaveLength(2);
  expect(sessions[0].dispose).toHaveBeenCalledOnce();
  expect(view.result.current.voice).toBe(sessions[1]);
  expect(sessions[1].dispose).not.toHaveBeenCalled();
  view.unmount();
  expect(sessions[1].dispose).toHaveBeenCalledOnce();
});

it("keeps the controller on rerender and disposes it when the gateway changes", () => {
  const view = renderHook(({ url }) => useVoiceController(url), {
    initialProps: { url: "/first" },
  });
  view.rerender({ url: "/first" });
  expect(sessions).toHaveLength(1);
  view.rerender({ url: "/second" });
  expect(sessions).toHaveLength(2);
  expect(sessions[0].dispose).toHaveBeenCalledOnce();
  expect(view.result.current.voice).toBe(sessions[1]);
  expect(create).toHaveBeenLastCalledWith("/second");
});
