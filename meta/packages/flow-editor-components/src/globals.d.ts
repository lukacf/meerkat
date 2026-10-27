// The shell provides React through window globals (react-globals.js); the
// classic JSX transform emits React.createElement calls that resolve to it.
import type * as ReactNS from "react";

declare global {
  const React: typeof ReactNS;

  // The controller facade global stays the runtime contract for view
  // components until S23; components call it at render time, not import time.
  interface Window {
    MetaFlowController: any;
  }
}

export {};
