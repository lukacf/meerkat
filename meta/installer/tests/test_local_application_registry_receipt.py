"""A lost registry cannot turn an installed product into an ordinary package.

The original Build/Install fixture uses real source and package readers with
explicit inert builder, acceptance and worker boundaries. These are ownership
controls, not native/application acceptance or execution.
"""
import json

import pytest

from launchkit.connect_installer_caller import CustodyError
from launchkit.local_application_registry import LocalApplicationRegistry
from test_local_application_registry import (
    accept_source_fixture,
    build,
    install_fixture,
    new_guide,
    no_processes,
    no_product_execution,
    product,
    registry,
    source_bytes,
)


@pytest.mark.parametrize("selection", ["missing", "null"])
async def test_lost_registry_refuses_product_receipt_without_selected_runtime(
        registry, monkeypatch, selection):
    f = registry
    saved = new_guide(f)
    result = await build(f, saved)
    await accept_source_fixture(f, result)
    host = f.registry.host_for_build(saved["id"], result["build_id"])
    directory, destination = await install_fixture(f, saved, result, monkeypatch)
    assert f.registry.selected_host(saved["id"], directory.name) is host
    assert host._retained_preparation_inputs is not None

    lost = LocalApplicationRegistry(guide_store=f.store, implementation=f.implementation,
        deployment=f.service, runtime_target=f.target)
    with pytest.raises(CustodyError, match="retained original host"):
        lost.selected_host(saved["id"], directory.name)

    receipt_path = directory / "receipt.json"
    receipt = json.loads(receipt_path.read_bytes())
    assert receipt["runtime_candidate"] is not None
    if selection == "missing":
        receipt.pop("runtime_candidate")
    else:
        receipt["runtime_candidate"] = None
    receipt_path.write_text(json.dumps(receipt, indent=2) + "\n")
    build_directory = f.implementation._directory(saved["id"], result["build_id"])

    def retained_bytes():
        return (receipt_path.read_bytes(), (directory / "package.zip").read_bytes(),
            (build_directory / "input.json").read_bytes(),
            source_bytes(build_directory / "local-application-source"),
            source_bytes(destination / ".mobkit/forge-local-setup"),
            (destination / ".env").read_bytes())

    before = retained_bytes()
    with pytest.raises(CustodyError, match="canonical package changed its original runtime selection"):
        lost.selected_host(saved["id"], directory.name)
    assert retained_bytes() == before
