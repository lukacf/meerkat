"""Generated error types for Meerkat SDK."""


class MeerkatError(Exception):
    """Base error for Meerkat SDK."""

    def __init__(self, code: str, message: str, details=None, capability_hint=None):
        super().__init__(message)
        self.code = code
        self.message = message
        self.details = details
        self.capability_hint = capability_hint


class CapabilityUnavailableError(MeerkatError):
    """Raised when a capability is not available."""
    pass


class SessionNotFoundError(MeerkatError):
    """Raised when a session is not found."""
    pass


class SkillNotFoundError(MeerkatError):
    """Raised when a skill reference cannot be resolved."""
    pass
