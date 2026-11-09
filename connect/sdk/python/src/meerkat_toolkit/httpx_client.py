"""Host composition for HTTPX clients borrowed by Toolkit connectors.

Call ``prepare_httpx_client(client)`` before sharing a client with operations.
It installs one first response hook so cleanup remains owned before HTTPX
invokes host response hooks or reads an auth-flow response. It preserves the
host's client, credentials, endpoint, hook order and client lifetime.

Hosts must keep hook ordering and response-stream ownership stable while
operations borrow the client. Operations validate preparation and never
reconfigure a shared client themselves.
"""

from .connectors._httpx_stream import (
    HttpxClientNotPrepared,
    prepare_httpx_client,
    require_prepared_httpx_client,
)

__all__ = ["HttpxClientNotPrepared", "prepare_httpx_client", "require_prepared_httpx_client"]
