// Generated error types for Meerkat SDK

export class MeerkatError extends Error {
  constructor(
    public readonly code: string,
    message: string,
    public readonly details?: unknown,
    public readonly capabilityHint?: { capability_id: string; message: string },
  ) {
    super(message);
    this.name = 'MeerkatError';
  }
}

export class CapabilityUnavailableError extends MeerkatError {
  constructor(code: string, message: string, details?: unknown, capabilityHint?: { capability_id: string; message: string }) {
    super(code, message, details, capabilityHint);
    this.name = 'CapabilityUnavailableError';
  }
}

export class SessionNotFoundError extends MeerkatError {
  constructor(code: string, message: string) {
    super(code, message);
    this.name = 'SessionNotFoundError';
  }
}

export class SkillNotFoundError extends MeerkatError {
  constructor(code: string, message: string) {
    super(code, message);
    this.name = 'SkillNotFoundError';
  }
}
