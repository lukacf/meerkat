/**
 * Capability checking and method gating for the Meerkat SDK.
 */

import type { CapabilitiesResponse } from "./generated/types.js";
import { CapabilityUnavailableError } from "./generated/errors.js";

/**
 * Gates method calls by checking runtime capabilities.
 *
 * @example
 * ```ts
 * const checker = new CapabilityChecker(capabilitiesResponse);
 * checker.require("comms"); // throws if comms not available
 * ```
 */
export class CapabilityChecker {
  private capabilities: Map<string, string>;

  constructor(response: CapabilitiesResponse) {
    this.capabilities = new Map(
      response.capabilities.map((c) => [c.id, c.status]),
    );
  }

  /** Check if a capability is available. */
  has(capabilityId: string): boolean {
    return this.capabilities.get(capabilityId) === "Available";
  }

  /** Throw CapabilityUnavailableError if capability is not available. */
  require(capabilityId: string): void {
    if (!this.has(capabilityId)) {
      throw new CapabilityUnavailableError(
        "CAPABILITY_UNAVAILABLE",
        `Capability '${capabilityId}' is not available in this runtime`,
      );
    }
  }

  /** List all available capability IDs. */
  get available(): string[] {
    return [...this.capabilities.entries()]
      .filter(([, status]) => status === "Available")
      .map(([id]) => id);
  }
}
