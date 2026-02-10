"""Capability checking and method gating for the Meerkat SDK."""

from .errors import CapabilityUnavailableError
from .generated.types import CapabilitiesResponse


class CapabilityChecker:
    """Gates method calls by checking runtime capabilities.

    Usage:
        checker = CapabilityChecker(capabilities_response)
        checker.require("comms")  # raises if comms not available
    """

    def __init__(self, response: CapabilitiesResponse):
        self._capabilities = {
            c.id: c.status for c in response.capabilities
        }

    def has(self, capability_id: str) -> bool:
        """Check if a capability is available."""
        return self._capabilities.get(capability_id) == "available"

    def require(self, capability_id: str) -> None:
        """Raise CapabilityUnavailableError if capability is not available."""
        if not self.has(capability_id):
            raise CapabilityUnavailableError(
                "CAPABILITY_UNAVAILABLE",
                f"Capability '{capability_id}' is not available in this runtime",
            )

    @property
    def available(self) -> list[str]:
        """List all available capability IDs."""
        return [k for k, v in self._capabilities.items() if v == "available"]
