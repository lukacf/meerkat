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

/** The console's voice session for `baseUrl`. `callerScope` names the caller
 * the console acts as on every voice request (`expected_caller_scope`). */
export function useVoiceController(baseUrl: string, callerScope?: () => string | undefined) {
  const [voice, setVoice] = React.useState<VoiceSession | null>(null);
  const callerScopeRef = React.useRef(callerScope);
  callerScopeRef.current = callerScope;
  React.useEffect(() => {
    const owned = createVoiceSession(baseUrl, undefined, () => callerScopeRef.current?.());
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
