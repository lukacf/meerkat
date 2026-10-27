import React from "react";
import { Icon } from "../icon";
import type { VoiceSessionSnapshot } from "../lib/voice-session";
import { voiceContextFailureMessage } from "../lib/voice-context";
import "./voice-bar.css";
import { countRender } from "../lib/render-counts";

type WaveformSource = "microphone" | "speaker";

/** Primary status once the microphone is open. Context arrival never gates it. */
export const TALK_READY_STATUS = "Listening, you can talk";
const CONTEXT_STAGE_TITLES = {
  capturing: "being read",
  generating: "being summarised",
  delivering: "being sent to the voice model",
} as const;
export type WaveformSampler = (source: WaveformSource, samples: Float32Array<ArrayBuffer>) => void;

interface VoiceBarProps {
  state: VoiceSessionSnapshot;
  sampleWaveform: WaveformSampler;
  onClose: () => void;
  onToggleMicrophone: () => void;
  onToggleSpeaker: () => void;
}

function Glyph({ name }: { name: string }): React.JSX.Element {
  return <span className="voice-glyph" aria-hidden="true"><Icon name={name} /></span>;
}

export function VoiceButton({
  agentLabel,
  active = false,
  disabled = false,
  checking = false,
  onClick,
}: {
  agentLabel: string;
  active?: boolean;
  disabled?: boolean;
  /** Readiness has no definite answer yet. The button stays usable: starting voice confirms. */
  checking?: boolean;
  onClick: () => void;
}): React.JSX.Element {
  const label = active
    ? `End voice with ${agentLabel}`
    : checking
      ? `Start voice with ${agentLabel} (checking availability)`
      : `Start voice with ${agentLabel}`;
  return (
    <button
      type="button"
      className={checking ? "composer__voice composer__voice--checking" : "composer__voice"}
      aria-label={label}
      aria-pressed={active}
      aria-busy={checking || undefined}
      title={label}
      disabled={disabled}
      onClick={onClick}
      data-testid="voice-start"
      data-readiness={checking ? "checking" : "confirmed"}
    >
      <Glyph name="i-voice" />
    </button>
  );
}

function AudioWaveform({
  source,
  sampleWaveform,
  active,
}: {
  source: WaveformSource;
  sampleWaveform: WaveformSampler;
  active: boolean;
}): React.JSX.Element {
  const canvasRef = React.useRef<HTMLCanvasElement>(null);
  React.useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const context = canvas.getContext("2d");
    if (!context) return;
    const samples = new Float32Array(512);
    const motionQuery = window.matchMedia("(prefers-reduced-motion: reduce)");
    let frame = 0;
    let lastDraw = -Infinity;
    let width = 0;
    let height = 0;

    const resize = () => {
      const rect = canvas.getBoundingClientRect();
      width = rect.width;
      height = rect.height;
      const scale = Math.min(window.devicePixelRatio || 1, 2);
      canvas.width = Math.round(width * scale);
      canvas.height = Math.round(height * scale);
      context.setTransform(scale, 0, 0, scale, 0, 0);
    };
    const draw = (now: number) => {
      frame = window.requestAnimationFrame(draw);
      if (document.hidden || now - lastDraw < (motionQuery.matches ? 125 : 33)) return;
      lastDraw = now;
      if (!width || !height) return;
      samples.fill(0);
      if (active) sampleWaveform(source, samples);
      const color = getComputedStyle(canvas).color;
      context.clearRect(0, 0, width, height);
      const middle = height / 2;
      context.strokeStyle = color;
      context.globalAlpha = 0.18;
      context.lineWidth = 1;
      context.beginPath();
      context.moveTo(0, middle);
      context.lineTo(width, middle);
      context.stroke();
      context.globalAlpha = 1;
      context.lineWidth = 1.7;
      context.lineJoin = "round";
      context.beginPath();
      for (let index = 0; index < samples.length; index += 1) {
        const x = index * width / (samples.length - 1);
        const amplitude = Math.max(-1, Math.min(1, samples[index] * 2.4));
        const y = middle - amplitude * (middle - 3);
        if (index === 0) context.moveTo(x, y);
        else context.lineTo(x, y);
      }
      context.stroke();
    };
    resize();
    const observer = new ResizeObserver(resize);
    observer.observe(canvas);
    frame = window.requestAnimationFrame(draw);
    return () => {
      window.cancelAnimationFrame(frame);
      observer.disconnect();
    };
  }, [active, sampleWaveform, source]);

  return <canvas className={`voice-waveform voice-waveform--${source}`} ref={canvasRef} aria-hidden="true" />;
}

/// Memoised: the app root re-renders on every coalesced frame flush and on
/// each debounced draft publish, and the props here are stable references
/// (see ConsoleApp), so unchanged input skips the whole subtree.
export const VoiceBar = React.memo(function VoiceBar({
  state,
  sampleWaveform,
  onClose,
  onToggleMicrophone,
  onToggleSpeaker,
}: VoiceBarProps): React.JSX.Element | null {
  countRender("VoiceBar");
  if (!state.target && !state.error && !state.notice) return null;
  const active = state.phase === "active";
  const transitioning = state.phase === "requesting" || state.phase === "connecting";
  const status = state.phase === "requesting"
    ? "Allow microphone access"
    : state.phase === "connecting"
      ? state.connectionStage === "opening"
        ? "Starting voice"
        : state.connectionStage === "recovery"
          ? "Reconnecting"
          : "Connecting audio"
      : state.phase === "closing"
        ? "Ending voice"
        : active
          ? state.reconnecting
            ? "Reconnecting"
            : state.microphoneMuted ? "Microphone muted" : TALK_READY_STATUS
          : "Voice ended";
  // The microphone is open as soon as the phase is active; context
  // preparation is a secondary hint that never gates talking.
  const talkReady = active && !state.reconnecting && !state.microphoneMuted;
  const preparation = state.contextPreparation;
  const contextState = !active || preparation === undefined ? null
    : state.contextStatusError ? "status_unavailable"
      : preparation === null ? "checking"
        : preparation.phase;
  const contextLabel = contextState === null ? null
    : contextState === "status_unavailable" ? "Context status unavailable"
      : contextState === "checking" || contextState === "preparing" ? "Context arriving"
        : contextState === "provider_acknowledged" ? "Context supplied"
          : contextState === "not_requested" ? "No summary pending"
            : "Context unavailable";
  const contextTitle = contextState === "checking"
    ? "Checking whether the agent's context is on its way. You can talk now."
    : contextState === "preparing"
      ? `The agent's context is ${CONTEXT_STAGE_TITLES[(preparation as { stage: keyof typeof CONTEXT_STAGE_TITLES }).stage]}. You can talk now; the model receives it as it arrives.`
      : contextState === "provider_acknowledged"
        ? "The voice provider acknowledged the initial context. This does not confirm recall or speech completion."
        : contextState === "not_requested"
          ? "No concurrent context preparation was requested for this call."
          : undefined;
  const contextMessage = !active ? null : state.contextStatusError ??
    (preparation?.phase === "failed" ? voiceContextFailureMessage(preparation.reason) : null);

  return (
    <section
      className="voice-bar"
      aria-label={state.target ? `Voice with ${state.target.label}` : "Voice"}
      data-testid="voice-bar"
      data-phase={state.phase}
    >
      <div className="voice-bar__header">
        <div className="voice-bar__heading">
          <span className="voice-bar__indicator" aria-hidden="true" />
          <div className="voice-bar__titles">
            <span className="voice-bar__name" title={state.target?.label}>
              {state.target ? <>Voice with <strong>{state.target.label}</strong></> : "Voice"}
            </span>
            <span className="voice-bar__status" role="status" data-testid="voice-status" data-talk-ready={talkReady || undefined}>
              <span>{status}</span>
              {contextLabel && (
                <span
                  className="voice-bar__context"
                  data-context={contextState ?? undefined}
                  data-context-stage={preparation?.phase === "preparing" ? preparation.stage : undefined}
                  title={contextTitle}
                >{contextLabel}</span>
              )}
            </span>
          </div>
        </div>
        <div className="voice-bar__controls">
          <button
            className="voice-bar__control"
            type="button"
            aria-label={state.microphoneMuted ? "Unmute microphone" : "Mute microphone"}
            title={state.microphoneMuted ? "Unmute microphone" : "Mute microphone"}
            aria-pressed={state.microphoneMuted}
            disabled={!active}
            onClick={onToggleMicrophone}
          >
            <Glyph name={state.microphoneMuted ? "i-mic-off" : "i-mic"} />
          </button>
          <button
            className="voice-bar__control"
            type="button"
            aria-label={state.speakerMuted ? "Unmute speakers" : "Mute speakers"}
            title={state.speakerMuted ? "Unmute speakers" : "Mute speakers"}
            aria-pressed={state.speakerMuted}
            disabled={!active}
            onClick={onToggleSpeaker}
          >
            <Glyph name={state.speakerMuted ? "i-speaker-off" : "i-speaker"} />
          </button>
          <span className="voice-bar__separator" aria-hidden="true" />
          <button
            className="voice-bar__control voice-bar__control--close"
            type="button"
            aria-label={active || transitioning ? "End voice conversation" : "Dismiss voice"}
            title={active || transitioning ? "End voice conversation" : "Dismiss voice"}
            onClick={onClose}
            disabled={state.phase === "closing"}
          >
            <Glyph name="i-close" />
          </button>
        </div>
      </div>
      {(active || transitioning) && (
        <div className="voice-bar__channels">
          <div className="voice-bar__channel">
            <div className="voice-bar__channel-label"><span>You</span><span>{state.microphoneMuted ? "Muted" : "Microphone"}</span></div>
            <AudioWaveform source="microphone" sampleWaveform={sampleWaveform} active={active && !state.microphoneMuted} />
          </div>
          <div className="voice-bar__channel">
            <div className="voice-bar__channel-label"><span>Agent</span><span>{state.speakerMuted ? "Speakers muted" : "Live audio"}</span></div>
            <AudioWaveform source="speaker" sampleWaveform={sampleWaveform} active={active} />
          </div>
        </div>
      )}
      {state.error && <p className="voice-bar__message voice-bar__message--error" role="alert">{state.error}</p>}
      {contextMessage && <p className="voice-bar__message voice-bar__message--error" role="alert">{contextMessage}</p>}
      {state.notice && <p className="voice-bar__message" role="status">{state.notice}</p>}
    </section>
  );
});
