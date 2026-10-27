"""Auth configuration for Meta runtime."""
from __future__ import annotations

from dataclasses import dataclass
from typing import Any


@dataclass(frozen=True)
class GoogleAuthConfig:
    client_id: str
    discovery_url: str = "https://accounts.google.com/.well-known/openid-configuration"
    audience: str | None = None
    leeway_seconds: int = 60

    def to_dict(self) -> dict[str, Any]:
        return {
            "provider": "google",
            "client_id": self.client_id,
            "discovery_url": self.discovery_url,
            "audience": self.audience or self.client_id,
            "leeway_seconds": self.leeway_seconds,
        }


@dataclass(frozen=True)
class JwtAuthConfig:
    shared_secret: str
    issuer: str | None = None
    audience: str | None = None
    leeway_seconds: int = 60

    def to_dict(self) -> dict[str, Any]:
        return {
            "provider": "jwt",
            "shared_secret": self.shared_secret,
            "issuer": self.issuer,
            "audience": self.audience,
            "leeway_seconds": self.leeway_seconds,
        }


def google(client_id: str, **kwargs: Any) -> GoogleAuthConfig:
    return GoogleAuthConfig(client_id=client_id, **kwargs)


def jwt(shared_secret: str, **kwargs: Any) -> JwtAuthConfig:
    return JwtAuthConfig(shared_secret=shared_secret, **kwargs)
