"""Bounded host shutdown budgets around the gateway's shutdown horizon.

The application's SDK waits the stdio_shutdown_horizon_ms its gateway
advertised in meta/init for native cleanup, then its own TERM and KILL graces.
The application reports that horizon at readiness. Each enclosing owner adds
time to close adapters, reap its exact children and publish cleanup. An
application that reports none is assumed to run this repository's gateway.
These are host deadlines, not evidence that native cleanup acknowledged success.
Recorded older service definitions keep their original 45-second or, from the
earlier pinned 347-second horizon, 397-second OS budget.
"""

# rpc_gateway.rs GATEWAY_SHUTDOWN_HORIZON_MS, pinned by a parity test: a change
# to the gateway's horizon moves this fallback in the same change.
UNREPORTED_GATEWAY_SHUTDOWN_HORIZON_MS = 432_000
# The Python SDK's parse bound; it waits its own default for a larger value.
MAX_REPORTED_GATEWAY_SHUTDOWN_HORIZON_MS = 2_147_483_647
EARLIER_PINNED_GATEWAY_SHUTDOWN_HORIZON_MS = 347_000
NATIVE_TRANSPORT_FORCE_REAP_TIMEOUT = 10  # Installed SDK TERM 5s, KILL/reap 5s.
APPLICATION_ADAPTER_STOP_MARGIN = 10
PROCESS_FORCE_REAP_TIMEOUT = 10
WORKER_STOP_MARGIN = 5
USER_SERVICE_STOP_MARGIN = 15
SERVICE_STOP_COMMAND_MARGIN = 15
LEGACY_USER_SERVICE_STOP_TIMEOUT = 45


def reported_gateway_horizon(payload):
    """The horizon an application reported at readiness; None when it reported none."""
    value = payload.get("gateway_shutdown_horizon_ms")
    if value is None:
        return None
    if type(value) is not int or not 0 < value <= MAX_REPORTED_GATEWAY_SHUTDOWN_HORIZON_MS:
        raise ValueError("The application reported an invalid gateway shutdown horizon.")
    return value


def shutdown_budget(horizon_ms=None):
    """Stop budgets, in seconds, that enclose one gateway shutdown horizon."""
    horizon = UNREPORTED_GATEWAY_SHUTDOWN_HORIZON_MS if horizon_ms is None else horizon_ms
    application = (-(-horizon // 1000) + NATIVE_TRANSPORT_FORCE_REAP_TIMEOUT
                   + APPLICATION_ADAPTER_STOP_MARGIN)
    worker = application + PROCESS_FORCE_REAP_TIMEOUT + WORKER_STOP_MARGIN
    service = worker + USER_SERVICE_STOP_MARGIN
    return {"application": application, "force_reap": PROCESS_FORCE_REAP_TIMEOUT, "worker": worker,
            "service": service, "command": service + SERVICE_STOP_COMMAND_MARGIN,
            "legacy_service": LEGACY_USER_SERVICE_STOP_TIMEOUT}


def service_stop_budgets(horizon_ms=None):
    """The OS stop budgets a recorded unit may carry for a recorded horizon."""
    return {LEGACY_USER_SERVICE_STOP_TIMEOUT,
            shutdown_budget(EARLIER_PINNED_GATEWAY_SHUTDOWN_HORIZON_MS)["service"],
            shutdown_budget()["service"], shutdown_budget(horizon_ms)["service"]}


APPLICATION_STOP_TIMEOUT = shutdown_budget()["application"]
WORKER_STOP_TIMEOUT = shutdown_budget()["worker"]
USER_SERVICE_STOP_TIMEOUT = shutdown_budget()["service"]
SERVICE_STOP_COMMAND_TIMEOUT = shutdown_budget()["command"]


def render_destination_shutdown_budget(template):
    """Export the shared budget inputs without requiring Forge on the destination."""
    import json

    marker = repr("__LAUNCH_SHUTDOWN_BUDGET__")
    if template.count(marker) != 1:
        raise ValueError("The destination shutdown budget template is invalid.")
    budget = {"unreported_horizon_ms": UNREPORTED_GATEWAY_SHUTDOWN_HORIZON_MS,
        "maximum_horizon_ms": MAX_REPORTED_GATEWAY_SHUTDOWN_HORIZON_MS,
        "earlier_pinned_horizon_ms": EARLIER_PINNED_GATEWAY_SHUTDOWN_HORIZON_MS,
        "application_margin": NATIVE_TRANSPORT_FORCE_REAP_TIMEOUT + APPLICATION_ADAPTER_STOP_MARGIN,
        "force_reap": PROCESS_FORCE_REAP_TIMEOUT, "worker_margin": WORKER_STOP_MARGIN,
        "service_margin": USER_SERVICE_STOP_MARGIN, "command_margin": SERVICE_STOP_COMMAND_MARGIN,
        "legacy_service": LEGACY_USER_SERVICE_STOP_TIMEOUT}
    return template.replace(marker, json.dumps(budget, ensure_ascii=True))
