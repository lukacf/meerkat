import React from "react";
import { createVoiceSession, type VoiceSession, type VoiceSessionSnapshot } from "./voice-session";

const IDLE: VoiceSessionSnapshot = {
  phase: "idle",
  target: null,
  microphoneMuted: false,
  speakerMuted: false,
  error: null,
  notice: null,
};
const idleSnapshot = () => IDLE;
const idleSubscribe = () => () => {};

export function useVoiceController(baseUrl: string) {
  const [voice, setVoice] = React.useState<VoiceSession | null>(null);
  React.useEffect(() => {
    const owned = createVoiceSession(baseUrl);
    setVoice(owned);
    return () => owned.dispose();
  }, [baseUrl]);
  const state = React.useSyncExternalStore(
    voice?.subscribe ?? idleSubscribe,
    voice?.getSnapshot ?? idleSnapshot,
    idleSnapshot,
  );
  return { voice, state };
}
