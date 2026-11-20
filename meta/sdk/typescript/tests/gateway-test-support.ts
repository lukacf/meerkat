/** Private child state for tests that launch a real rpc_gateway. */
import { mkdirSync } from "node:fs";
import { join } from "node:path";

export function gatewayEnvironment(directory: string): Record<string, string> {
  const stateHome = join(directory, "state-home");
  mkdirSync(stateHome, { recursive: true });
  // Linux resolves comms identities from XDG_STATE_HOME; preserve HOME.
  return { XDG_STATE_HOME: stateHome };
}
