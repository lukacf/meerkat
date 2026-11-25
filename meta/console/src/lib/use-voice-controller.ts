import React from "react";
import { createVoiceSession, type VoiceCallerScope, type VoiceMemberDoor, type VoiceSession, type VoiceSessionSnapshot } from "./voice-session";

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
 * refused, so an ended login or another caller reaches the console.
 * `memberDoor` names a request's member by its door (see mob-door). */
export function useVoiceController(
  baseUrl: string,
  callerScope?: VoiceCallerScope,
  reportFailure?: (error: unknown) => void,
  memberDoor?: VoiceMemberDoor,
) {
  const [voice, setVoice] = React.useState<VoiceSession | null>(null);
  const callerScopeRef = React.useRef(callerScope);
  callerScopeRef.current = callerScope;
  const reportFailureRef = React.useRef(reportFailure);
  reportFailureRef.current = reportFailure;
  const memberDoorRef = React.useRef(memberDoor);
  memberDoorRef.current = memberDoor;
  React.useEffect(() => {
    const owned = createVoiceSession(
      baseUrl,
      undefined,
      () => callerScopeRef.current?.(),
      (error) => reportFailureRef.current?.(error),
      (identity) => memberDoorRef.current?.(identity) ?? { mobId: null, identity },
    );
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
