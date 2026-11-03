export type FailureKind = "invalid_configuration" | "invalid_arguments" | "cancelled" |
  "deadline_exceeded" | "transport_error" | "http_error" | "redirect_refused" |
  "invalid_response" | "response_too_large" | "upstream_error";

/** Safe metadata. Neither provider bodies nor credentials are retained. */
export class ActivepiecesError extends Error {
  readonly kind: FailureKind;
  readonly entered: boolean;
  readonly dispatched: boolean;
  readonly status: number | undefined;
  readonly writeOutcome: "not_applicable" | "not_attempted" | "uncertain";
  readonly effect: "read" | "write";

  constructor(kind: FailureKind, entered = false, dispatched = false, status?: number, effect: "read" | "write" = "read") {
    super(`Activepieces ${effect === "write" ? "writer" : "reader"} ${kind}`);
    this.name = "ActivepiecesError";
    this.kind = kind;
    this.effect = effect;
    this.writeOutcome = effect === "read" ? "not_applicable" : dispatched ? "uncertain" : "not_attempted";
    this.entered = entered;
    this.dispatched = dispatched;
    this.status = status;
  }
}
