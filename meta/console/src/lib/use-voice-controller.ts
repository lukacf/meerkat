import React from "react";
import { createVoiceSession, type VoiceCallerScope, type VoiceSession, type VoiceSessionSnapshot } from "./voice-session";

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
 * the console acts as on every voice request (`expected_caller_scope`); a
 * request waits for it. `reportFailure` hears the requests the gateway
 * refused, so an ended login or another caller reaches the console. */
export function useVoiceController(baseUrl: string, callerScope?: VoiceCallerScope, reportFailure?: (error: unknown) => void) {
  const [voice, setVoice] = React.useState<VoiceSession | null>(null);
  const callerScopeRef = React.useRef(callerScope);
  callerScopeRef.current = callerScope;
  const reportFailureRef = React.useRef(reportFailure);
  reportFailureRef.current = reportFailure;
  React.useEffect(() => {
    const owned = createVoiceSession(baseUrl, undefined, () => callerScopeRef.current?.(), (error) => reportFailureRef.current?.(error));
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
