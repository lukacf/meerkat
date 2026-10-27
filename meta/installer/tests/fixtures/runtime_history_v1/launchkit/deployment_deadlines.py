"""Bounded host shutdown budgets for the pinned MobKit 0.8.42 gateway.

rpc_gateway.rs advertises stdio_shutdown_horizon_ms=347000: native cleanup
may need callback transport for its entire horizon. Each enclosing owner gets
additional time to close adapters, reap its exact children and publish cleanup.
These are host deadlines, not evidence that native cleanup acknowledged success.
Recorded older service definitions retain their original 45-second OS budget.
"""

NATIVE_GATEWAY_SHUTDOWN_TIMEOUT = 347
NATIVE_TRANSPORT_FORCE_REAP_TIMEOUT = 10  # Installed SDK TERM 5s, KILL/reap 5s.
APPLICATION_ADAPTER_STOP_MARGIN = 10
APPLICATION_STOP_TIMEOUT = (NATIVE_GATEWAY_SHUTDOWN_TIMEOUT
                            + NATIVE_TRANSPORT_FORCE_REAP_TIMEOUT
                            + APPLICATION_ADAPTER_STOP_MARGIN)
PROCESS_FORCE_REAP_TIMEOUT = 10
WORKER_STOP_TIMEOUT = APPLICATION_STOP_TIMEOUT + PROCESS_FORCE_REAP_TIMEOUT + 5
USER_SERVICE_STOP_TIMEOUT = WORKER_STOP_TIMEOUT + 15
SERVICE_STOP_COMMAND_TIMEOUT = USER_SERVICE_STOP_TIMEOUT + 15
LEGACY_USER_SERVICE_STOP_TIMEOUT = 45


def render_destination_shutdown_budget(template):
    """Export shared literal budgets without requiring Launch on the destination."""
    import json

    marker = repr("__LAUNCH_SHUTDOWN_BUDGET__")
    if template.count(marker) != 1:
        raise ValueError("The destination shutdown budget template is invalid.")
    budget = {"application": APPLICATION_STOP_TIMEOUT,
        "force_reap": PROCESS_FORCE_REAP_TIMEOUT, "worker": WORKER_STOP_TIMEOUT,
        "service": USER_SERVICE_STOP_TIMEOUT, "command": SERVICE_STOP_COMMAND_TIMEOUT,
        "legacy_service": LEGACY_USER_SERVICE_STOP_TIMEOUT}
    return template.replace(marker, json.dumps(budget, ensure_ascii=True))
