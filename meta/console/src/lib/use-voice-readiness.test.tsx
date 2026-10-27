import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { queryVoiceAvailability, type VoiceAvailability } from "./voice-session";
import {
  nextVoiceReadiness,
  READINESS_REFRESH_INTERVAL_MS,
  useVoiceReadiness,
  voiceReadinessDenied,
  voiceReadinessOffersVoice,
  voiceReadinessPending,
  voiceReadinessRetryDelayMs,
} from "./use-voice-readiness";

vi.mock("./voice-session", () => ({ queryVoiceAvailability: vi.fn() }));
const query = vi.mocked(queryVoiceAvailability);

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((yes) => { resolve = yes; });
  return { promise, resolve };
}

beforeEach(() => {
  vi.useFakeTimers();
  query.mockReset();
  query.mockResolvedValue("available");
});
afterEach(() => vi.useRealTimers());

it("shows a typed checking state from the first render, before any answer", async () => {
  const pending = deferred<VoiceAvailability>();
  query.mockReturnValue(pending.promise);
  const view = renderHook(() => useVoiceReadiness("/gateway", "alpha", null, true));
  expect(view.result.current).toEqual({ alpha: "checking" });
  expect(voiceReadinessOffersVoice(view.result.current.alpha)).toBe(true);
  expect(voiceReadinessPending(view.result.current.alpha)).toBe(true);
  expect(voiceReadinessDenied(view.result.current, "alpha")).toBe(false);
  await act(async () => pending.resolve("available"));
  expect(view.result.current).toEqual({ alpha: "available" });
  expect(voiceReadinessPending(view.result.current.alpha)).toBe(false);
});

it("requires positive per-target readiness and separately retains the voice target", async () => {
  const view = renderHook(
    ({ focused, voice }) => useVoiceReadiness("/gateway", focused, voice, true),
    { initialProps: { focused: "alpha", voice: "alpha" } },
  );
  await act(async () => {});
  expect(view.result.current).toEqual({ alpha: "available" });
  expect(query.mock.calls).toEqual([["/gateway", "alpha", expect.any(AbortSignal)]]);
  view.rerender({ focused: "beta", voice: "alpha" });
  expect(view.result.current).toEqual({ alpha: "available", beta: "checking" });
  await act(async () => {});
  expect(view.result.current).toEqual({ alpha: "available", beta: "available" });
});

it("refreshes auth for the ongoing voice target even when no chat is selected", async () => {
  const view = renderHook(() => useVoiceReadiness("/gateway", null, "alpha", true));
  await act(async () => {});
  expect(view.result.current.alpha).toBe("available");
  query.mockResolvedValue("unavailable");
  await act(() => vi.advanceTimersByTimeAsync(READINESS_REFRESH_INTERVAL_MS));
  expect(view.result.current.alpha).toBe("unavailable");
  expect(voiceReadinessOffersVoice(view.result.current.alpha)).toBe(false);
});

it("retries a failed check promptly with backoff instead of waiting for the refresh cadence", async () => {
  query.mockResolvedValue("unknown");
  const view = renderHook(() => useVoiceReadiness("/gateway", "alpha", null, true));
  await act(async () => {});
  expect(query).toHaveBeenCalledTimes(1);
  expect(view.result.current).toEqual({ alpha: "retrying" });
  expect(voiceReadinessOffersVoice(view.result.current.alpha)).toBe(true);
  expect(voiceReadinessDenied(view.result.current, "alpha")).toBe(false);
  await act(() => vi.advanceTimersByTimeAsync(999));
  expect(query).toHaveBeenCalledTimes(1);
  await act(() => vi.advanceTimersByTimeAsync(1));
  expect(query).toHaveBeenCalledTimes(2);
  await act(() => vi.advanceTimersByTimeAsync(2_000));
  expect(query).toHaveBeenCalledTimes(3);
  await act(() => vi.advanceTimersByTimeAsync(4_000));
  expect(query).toHaveBeenCalledTimes(4);
  query.mockResolvedValue("available");
  await act(() => vi.advanceTimersByTimeAsync(8_000));
  expect(query).toHaveBeenCalledTimes(5);
  expect(view.result.current).toEqual({ alpha: "available" });
  // A definite answer resets the backoff to the steady cadence.
  await act(() => vi.advanceTimersByTimeAsync(READINESS_REFRESH_INTERVAL_MS - 1));
  expect(query).toHaveBeenCalledTimes(5);
  await act(() => vi.advanceTimersByTimeAsync(1));
  expect(query).toHaveBeenCalledTimes(6);
});

it("never overlaps checks for one identity: the next starts only after the previous settled", async () => {
  const first = deferred<VoiceAvailability>();
  query.mockReturnValueOnce(first.promise);
  const view = renderHook(() => useVoiceReadiness("/gateway", "alpha", null, true));
  await act(() => vi.advanceTimersByTimeAsync(60_000));
  expect(query).toHaveBeenCalledTimes(1);
  expect(view.result.current).toEqual({ alpha: "checking" });
  await act(async () => first.resolve("unknown"));
  expect(view.result.current).toEqual({ alpha: "retrying" });
  await act(() => vi.advanceTimersByTimeAsync(1_000));
  expect(query).toHaveBeenCalledTimes(2);
});

it("aborts the in-flight check when the scope changes or unmounts", async () => {
  const signals: AbortSignal[] = [];
  query.mockImplementation((_baseUrl, _identity, signal) => {
    if (signal) signals.push(signal);
    return new Promise<VoiceAvailability>(() => {});
  });
  const view = renderHook(
    ({ focused }) => useVoiceReadiness("/gateway", focused, null, true),
    { initialProps: { focused: "alpha" } },
  );
  expect(signals).toHaveLength(1);
  view.rerender({ focused: "beta" });
  expect(signals[0].aborted).toBe(true);
  expect(signals).toHaveLength(2);
  view.unmount();
  expect(signals[1].aborted).toBe(true);
});

it("keeps the last known readiness when a check fails and flips only on a definite gateway answer", async () => {
  const view = renderHook(() => useVoiceReadiness("/gateway", null, "alpha", true));
  await act(async () => {});
  expect(view.result.current).toEqual({ alpha: "available" });
  query.mockResolvedValue("unknown");
  await act(() => vi.advanceTimersByTimeAsync(READINESS_REFRESH_INTERVAL_MS));
  expect(view.result.current).toEqual({ alpha: "available" });
  expect(voiceReadinessDenied(view.result.current, "alpha")).toBe(false);
  await act(() => vi.advanceTimersByTimeAsync(1_000));
  expect(view.result.current).toEqual({ alpha: "available" });
  expect(query).toHaveBeenCalledTimes(3);
  query.mockResolvedValue("unavailable");
  await act(() => vi.advanceTimersByTimeAsync(2_000));
  expect(view.result.current).toEqual({ alpha: "unavailable" });
  expect(voiceReadinessDenied(view.result.current, "alpha")).toBe(true);
});

it("does not carry a known value across gateways when the new gateway's check fails", async () => {
  const view = renderHook(
    ({ baseUrl }) => useVoiceReadiness(baseUrl, "alpha", null, true),
    { initialProps: { baseUrl: "/first" } },
  );
  await act(async () => {});
  expect(view.result.current).toEqual({ alpha: "available" });
  query.mockResolvedValue("unknown");
  view.rerender({ baseUrl: "/second" });
  expect(view.result.current).toEqual({ alpha: "checking" });
  await act(async () => {});
  expect(view.result.current).toEqual({ alpha: "retrying" });
});

it("does not transfer readiness across gateways or accept stale navigation responses", async () => {
  let finishAlpha: (available: VoiceAvailability) => void = () => {};
  query.mockImplementation((_baseUrl, identity) => identity === "alpha"
    ? new Promise((resolve) => { finishAlpha = resolve; })
    : Promise.resolve("unavailable"));
  const view = renderHook(
    ({ baseUrl, focused }) => useVoiceReadiness(baseUrl, focused, null, true),
    { initialProps: { baseUrl: "/first", focused: "alpha" } },
  );
  view.rerender({ baseUrl: "/second", focused: "beta" });
  await act(async () => {});
  await act(async () => finishAlpha("available"));
  expect(view.result.current).toEqual({ beta: "unavailable" });
});

it("does not probe a read-only console and stops refreshing on unmount", async () => {
  const view = renderHook(
    ({ enabled }) => useVoiceReadiness("/gateway", "alpha", null, enabled),
    { initialProps: { enabled: false } },
  );
  await act(async () => {});
  expect(view.result.current).toEqual({});
  expect(query).not.toHaveBeenCalled();
  view.rerender({ enabled: true });
  await act(async () => {});
  expect(query).toHaveBeenCalledOnce();
  view.unmount();
  await act(() => vi.advanceTimersByTimeAsync(30_000));
  expect(query).toHaveBeenCalledOnce();
});

it("folds check results so only definite answers replace known values and denial needs a definite no", () => {
  expect(nextVoiceReadiness(undefined, "available")).toBe("available");
  expect(nextVoiceReadiness(undefined, "unavailable")).toBe("unavailable");
  expect(nextVoiceReadiness(undefined, "unknown")).toBe("retrying");
  expect(nextVoiceReadiness("checking", "unknown")).toBe("retrying");
  expect(nextVoiceReadiness("retrying", "unknown")).toBe("retrying");
  expect(nextVoiceReadiness("available", "unknown")).toBe("available");
  expect(nextVoiceReadiness("unavailable", "unknown")).toBe("unavailable");
  expect(nextVoiceReadiness("available", "unavailable")).toBe("unavailable");
  expect(voiceReadinessDenied({ alpha: "unavailable" }, "alpha")).toBe(true);
  for (const state of ["checking", "retrying", "available"] as const) {
    expect(voiceReadinessDenied({ alpha: state }, "alpha")).toBe(false);
    expect(voiceReadinessOffersVoice(state)).toBe(true);
  }
  expect(voiceReadinessDenied({}, "alpha")).toBe(false);
  expect(voiceReadinessOffersVoice(undefined)).toBe(false);
  expect(voiceReadinessOffersVoice("unavailable")).toBe(false);
});

it("backs off 1 s, 2 s, 4 s, 8 s and caps at the refresh cadence", () => {
  expect([0, 1, 2, 3, 4, 5, 6, 50].map(voiceReadinessRetryDelayMs))
    .toEqual([15_000, 1_000, 2_000, 4_000, 8_000, 15_000, 15_000, 15_000]);
});
