import React, { useEffect, useRef, useState } from "react";
import { hostSignInPath } from "./sign-in-recovery.mjs";

export function SignInRecovery({ onCheck, className = "" }) {
  const [checking, setChecking] = useState(false);
  const [unconfirmed, setUnconfirmed] = useState(false);
  const mounted = useRef(true);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  const path = hostSignInPath();
  async function check() {
    if (checking || !onCheck) return;
    setChecking(true);
    setUnconfirmed(false);
    let confirmed = false;
    try { confirmed = await onCheck() === true; } catch {}
    if (mounted.current) { setChecking(false); setUnconfirmed(!confirmed); }
  }
  return <div className={`setup-error ${className}`} role="alert">
    <strong>Sign in to continue</strong>
    <p>Your sign-in needs renewal. Your conversation and draft are kept. Nothing will be sent again automatically.</p>
    {path ? <a className="setup-action" href={path} target="_blank" rel="noopener noreferrer">Sign in</a>
      : <p>Use this host's sign-in page, then return here to check your sign-in.</p>}
    <button type="button" disabled={checking || !onCheck} onClick={check}>
      {checking ? "Checking sign-in..." : "Check sign-in"}</button>
    {unconfirmed && <p>Sign-in could not be confirmed. Your draft is still kept; you can check again.</p>}
  </div>;
}
