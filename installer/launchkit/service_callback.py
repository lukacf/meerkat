"""Fixed browser return page for the host's protected service login.

This page transports the original query to the same authenticated workspace.
It neither selects a native target nor interprets an OAuth outcome. The native
ceremony owner must be composed before callback POSTs can be accepted.
"""

import base64
import hashlib

from starlette.responses import Response


MCP_CALLBACK_PATH = "/services/mcp/callback"

# No request data is interpolated into the document, script, CSP or messages.
_SCRIPT = """(async () => {
  let query = window.location.search.slice(1);
  const message = document.getElementById('callback-status');
  const unavailable = () => {
    message.textContent = 'The connection could not be confirmed. Return to Forge to check its status.';
  };
  try {
    window.history.replaceState(null, '', window.location.pathname);
    if (!query || query.length > 8192 || /[^\\x20-\\x7e]/.test(query)) {
      unavailable();
      return;
    }
    const identityResponse = await fetch('/__workspace/identity', {
      credentials: 'same-origin', cache: 'no-store', redirect: 'error',
      referrerPolicy: 'no-referrer', headers: {Accept: 'application/json'}
    });
    if (!identityResponse.ok ||
        identityResponse.headers.get('content-type')?.split(';')[0].trim() !== 'application/json') {
      unavailable();
      return;
    }
    const identity = await identityResponse.json();
    if (!identity || typeof identity.workspace !== 'string' ||
        !/^[0-9a-f]{64}$/.test(identity.workspace)) {
      unavailable();
      return;
    }
    const response = await fetch('/services/mcp/callback', {
      method: 'POST', credentials: 'same-origin', cache: 'no-store', redirect: 'error',
      referrerPolicy: 'no-referrer',
      headers: {'Content-Type': 'application/json', 'X-Forge-Workspace': identity.workspace},
      body: JSON.stringify({query})
    });
    if (response.status === 202) {
      message.textContent = 'Callback received. Return to Forge to check whether the connection completed.';
    } else {
      unavailable();
    }
  } catch {
    unavailable();
  } finally {
    query = '';
  }
})();"""

_STYLE = """body { font: 18px/1.5 system-ui, sans-serif; color: #23332c; background: #f7f8f5; }
main { max-width: 36rem; margin: 15vh auto; padding: 1.5rem; }
h1 { font-size: 1.6rem; } a { color: #17613f; }"""


def _hash(value: str) -> str:
    return base64.b64encode(hashlib.sha256(value.encode("utf-8")).digest()).decode("ascii")


_CSP = ("default-src 'none'; script-src 'sha256-" + _hash(_SCRIPT)
        + "'; style-src 'sha256-" + _hash(_STYLE)
        + "'; connect-src 'self'; base-uri 'none'; frame-ancestors 'none'; form-action 'none'")
_HEADERS = {"Cache-Control": "no-store", "Referrer-Policy": "no-referrer",
            "X-Content-Type-Options": "nosniff", "Content-Security-Policy": _CSP}
_PAGE = ("<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">"
         "<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">"
         "<title>Return to Forge</title><style>" + _STYLE + "</style></head><body><main>"
         "<h1>Return to Forge</h1><p id=\"callback-status\" role=\"status\">"
         "Returning your connection response...</p><p><a href=\"/\">Return to Forge</a></p>"
         "<noscript>JavaScript is required to return this response. Return to Forge to check the connection.</noscript>"
         "</main><script>" + _SCRIPT + "</script></body></html>").encode("utf-8")


def callback_landing() -> Response:
    return Response(_PAGE, media_type="text/html", headers=_HEADERS)


def callback_unavailable() -> Response:
    # No native login owner is composed yet. Never accept, queue, persist or
    # replay a code on behalf of a guessed target, realm or authenticated login.
    return Response("Connection setup is not available in this Forge session.",
                    status_code=503, media_type="text/plain", headers=_HEADERS)
