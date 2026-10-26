"""Use the native console's durable send/recover contract for a frozen request."""
from __future__ import annotations

import asyncio

from .projects import DraftConflict


async def send_command(client, base, command, request, runtime_root):
    # This is product retry intent, never a substitute admission receipt.
    await asyncio.to_thread(command.native_request, request, runtime_root)
    response = await client.get(base + "/console/experience")
    response.raise_for_status()
    experience = response.json()
    if experience.get("send_dedupe", {}).get("durable") is not True:
        raise DraftConflict("The native console has not confirmed durable command recovery.")
    retained = await asyncio.to_thread(command.load)
    if not retained.get("dispatch_attempted"):
        agents = experience.get("agent_sidebar", {}).get("live_snapshot", {}).get("agents", [])
        matches = [agent for agent in agents if isinstance(agent, dict) and agent.get("identity") == "architect:main"]
        if (len(matches) != 1 or not isinstance(matches[0].get("session_id"), str)
                or not matches[0]["session_id"] or matches[0].get("state") != "active"):
            raise DraftConflict("The native guide session is not ready. Retry this original command after startup completes.")
    attempted = await asyncio.to_thread(command.begin_dispatch)
    if attempted:
        response = await client.post(base + "/console/send/recover", json={
            key: request[key] for key in ("identity", "origin", "idempotency_key")})
        # Even typed send_not_found cannot distinguish a cleared store from a
        # request lost before admission. It never authorizes a fresh dispatch.
        if response.status_code == 404:
            raise DraftConflict("The original native receipt is unavailable. The command was not resent.")
        response.raise_for_status()
        recovered = response.json()
        await _retain_acceptance(command, recovered)
        status = recovered["status"]
        if status == "delivery_failed":
            await asyncio.to_thread(command.fail, "native_delivery_failed")
            raise DraftConflict("The original native command failed delivery.")
        if status == "delivered" or recovered.get("native", {}).get("state") == "admitted":
            return recovered
        if status != "accepted" or recovered.get("native", {}).get("state") != "absent":
            raise DraftConflict("Native admission is uncertain. Recover this original command before continuing.")
        # Native owns reconciliation against the frame's recorded session. This
        # exact replay can settle or dispatch that frame, never retarget it.
    response = await client.post(base + "/console/send", json=request)
    response.raise_for_status()
    accepted = response.json()
    await _retain_acceptance(command, accepted)
    if accepted["status"] == "delivery_failed":
        await asyncio.to_thread(command.fail, "native_delivery_failed")
        raise DraftConflict("The original native command failed delivery.")
    return accepted


async def _retain_acceptance(command, accepted):
    if (not isinstance(accepted, dict) or accepted.get("identity") != "architect:main"
            or accepted.get("status") not in {"accepted", "delivered", "delivery_failed"}
            or any(not isinstance(accepted.get(key), str) or not accepted[key]
                   for key in ("interaction_id", "input_frame_id", "cursor", "session_id"))):
        raise DraftConflict("The native console did not return the original command's complete receipt.")
    await asyncio.to_thread(command.acceptance, accepted)
