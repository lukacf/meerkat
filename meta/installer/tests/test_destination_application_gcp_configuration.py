"""Original pre-stage GCP package with real guest Configure ownership.

The source/artifact intake, actual factory byte Install/default retainer,
canonical receipt, private settings and GCP physical-target readers are real.
Linux guest observations and venv/pip/check effects are inert. No VM, native
setup, grants, qualification, login or running cloud application is claimed.
"""
import asyncio
from copy import deepcopy
import json
import threading

import pytest

from launchkit import deployment
from launchkit.projects import DraftConflict
from launchkit.templates.storage_setup import project_lock
from test_destination_application_configuration import guest, configure, arguments, retained, assert_stopped
from test_destination_application_inputs import inputs_destination
from test_destination_application_factory import destination
from test_product_destination_install import installation
from test_product_destination_owner import destination_source, gcp_destination_source, no_destination_execution
from test_product_artifact_transfer import custody as artifact_custody
from test_meta_toolkit_runtime_selection import product, no_product_execution


@pytest.mark.parametrize("installation", ["gcp"], indirect=True)
async def test_original_gcp_guest_status_and_configure_forward_actual_source_for_every_installed_read(
        inputs_destination, monkeypatch, installation):
    # inputs_destination reserves its actual access source first; the required
    # installation parameter then selects the original pre-stage GCP package.
    f = inputs_destination
    owner = await guest(f)
    assert f is installation and f.service.guide is None and f.service.implementation is None
    before = retained(f), deepcopy(f.selected.context), f.acquisition.read(), f.viewer.read()
    assert f.spec["target"] == "gcp" and f.selected.context["schema"] == 2
    assert json.loads((f.root / "blueprint.json").read_bytes())["target"] == "gcp"
    assert json.loads((f.root / "deployment.json").read_bytes())["target"] == "gcp"
    original = f.service._installed_configuration
    reads = []
    phase = {"name": "initial-status"}

    def read(gid, did, **kwargs):
        assert threading.get_ident() != f.thread
        with pytest.raises(RuntimeError, match="no running event loop"):
            asyncio.get_running_loop()
        source = kwargs.get("source_custody")
        assert source is f.factory._source and source is owner.host._product_source_custody
        assert source is not None and not source._closed and not source._invalidated
        assert source._lease.root == f.root
        result = original(gid, did, **kwargs)
        reads.append((phase["name"], source))
        return result

    monkeypatch.setattr(f.service, "_installed_configuration", read)
    async with asyncio.timeout(20):
        first = await f.factory.setup_status(f.service, f.factory.guide_id, f.factory.deployment_id)
        phase["name"] = "settings-form"
        options = await f.factory.settings_options(f.service, f.factory.guide_id, f.factory.deployment_id)
        phase["name"] = "configure"
        result = await configure(f, settings={"CONSOLE_LOGIN_PASSWORD": "inert-gcp-private-password"})
        phase["name"] = "saved-status"
        after = await f.factory.setup_status(f.service, f.factory.guide_id, f.factory.deployment_id)
    assert first["available"] and after["available"] and after["action"] == "prepare-setup"
    assert options["version"] == arguments(f)["version"] and options["source_verified"]
    assert result["action_request"]["phase"] == "settled" and result["action_request"]["outcome"] == "returned"
    assert deployment.private_configuration_snapshot(f.root)[0]["CONSOLE_LOGIN_PASSWORD"] == "inert-gcp-private-password"
    assert "inert-gcp-private-password" not in json.dumps(result) + json.dumps(options) + json.dumps(after)
    assert {name for name, _ in reads} == {"initial-status", "settings-form", "configure", "saved-status"}
    assert all(source._closed and not source._invalidated for _, source in reads)
    assert before == (retained(f), f.selected.context, f.acquisition.read(), f.viewer.read())
    # Authenticated private source forwarding does not loosen the public path.
    with pytest.raises(DraftConflict, match="could not be verified"):
        original(f.factory.guide_id, f.factory.deployment_id)
    assert_stopped(f)
    with project_lock(f.root) as lease:
        lease.require_current()
