"""Cross-language projection fixture. No native client or protected presenter."""
import asyncio
import json

from meerkat_toolkit.connector_auth import ConnectorSlot, ConnectorTarget, Discover, Known
from meerkat_toolkit.connector_auth_host import ConnectorSignInBinding, HostStepReference


async def fixture(enabled, selection=Discover()):
    async def fixture_admission(context, operation, request, target):
        assert context == "fixture-person"

    async def fixture_presenter(url):
        raise AssertionError("Projection must never present or start auth")

    reference = HostStepReference("fixture-host", "fixture-step", "fixture-revision")
    control = {"scope": "fixture-person", "viewRevision": 1, "control": {
        "offer": "fixture-offer", "kind": "open_authorization", "target": reference.to_view()}}
    binding = ConnectorSignInBinding(object(), ConnectorTarget(
        ConnectorSlot("fixture-realm", "fixture-slot"), "https://issuer.example.test",
        "fixture-client", "fixture-resource", ("read",), "fixture-strategy", selection),
        reference=reference, revalidate=fixture_admission,
        protected_presenter=fixture_presenter if enabled else None)
    action = await binding.project_action("fixture-person", action_id="fixture-action",
        integration="Fixture connector", state="action_required",
        instructions="Review the host-supplied sign-in request.", control_request=control)
    return {"scope": "fixture-person", "revision": 1, "observedAt": "Synthetic fixture",
        "announcement": "Sign-in is pending; no account has been connected.", "tasks": [{
            "id": "fixture-task", "title": "Connect a service", "application": "Fixture host",
            "requester": "Fixture person", "reason": "Exercise the projection contract",
            "phase": {"tone": "pending", "text": "Waiting for sign-in"},
            "nextAction": "Review sign-in", "roles": [], "userActions": [action], "controls": [],
        }], "connections": [], "frontends": [], "packages": []}


async def main():
    print(json.dumps({"enabled": await fixture(True), "disabled": await fixture(False),
                      "known": await fixture(True, Known("known-subject"))}))


asyncio.run(main())
