"""Initial product Fresh/Restore reconstruction with inert native inputs."""
from copy import deepcopy
import json
import os
import tomllib

import pytest

from launchkit import deployment, deployment_application_bootstrap as bootstrap
from launchkit import runtime_selection as selection
from launchkit.connect_installer_caller import CustodyError, ExactFile, ProductGatewayFile
from launchkit.templates import install_candidate as adapter
from launchkit.templates.storage_setup import project_lock
from test_deployment_application_bootstrap import Inputs, write, no_runtime_operations
from test_meta_toolkit_initial_install import initial, implementation_service, product, no_product_execution, no_processes


@pytest.fixture
async def product_bootstrap(initial, monkeypatch):
    f = initial
    receipt = await f.submit()
    directory = f.service._directory(f.guide_id)
    package = deployment.materialize(directory)
    receipt = deployment.update_receipt(directory, status="stopped", phase="configure")
    inputs = Inputs(directory, receipt, f.destination, monkeypatch)
    definition = package["config/mob.toml"][0].decode()
    mob_id = tomllib.loads(definition)["mob"]["id"]
    deployment_value = json.loads(package["deployment.json"][0])
    rows = [{"identity": row["identity"], "profile": row["profile"]} for row in deployment_value["agents"]]
    members = [{"member": {"mob_id": mob_id, "role": row["profile"], "member": "inert-initial-member-" + str(i)}}
        for i, row in enumerate(rows)]
    raw_policy = package["application/tool-policy.json"][0]
    from launchkit.connect_policy_input import application_tool_policy_selection
    native_policy = application_tool_policy_selection(raw_policy, selected_member_identities=tuple(row["identity"] for row in rows))
    native_ids = {row["identity"]: member["member"]["member"] for row, member in zip(rows, members)}
    for row in native_policy["members"]: row["member_identity"] = native_ids[row["member_identity"]]
    native_policy["source"] = {"source_id": receipt["build_id"], "source_digest": "sha256:" + bootstrap.updates.digest(raw_policy)}
    inputs.plan_value["application"].update(definition={"format": "toml", "text": definition},
        roster=rows, members=members, helper=members[0], tool_policy=native_policy)
    inputs.plan = write(inputs.plan.path, inputs.plan_value)
    inputs.fresh["preparation_plan"] = inputs.restore["preparation_plan"] = inputs.plan.pin()
    inputs.policy = write(inputs.policy.path, raw_policy)
    inputs.fresh["source_policy"] = inputs.policy.pin()
    artifacts = selection.selected_runtime_artifacts(f.target)
    gateway = adapter.install_meta_toolkit_prefix(artifacts, deployment_root=f.destination, require_current=lambda: None)
    inputs.gateway = ExactFile(gateway, artifacts.candidate["binary"]["sha256"])
    inputs.fresh["gateway"] = inputs.restore["gateway"] = inputs.gateway.pin()
    inputs.target = f.target
    return inputs


@pytest.mark.parametrize("kind", ["fresh", "restore"])
async def test_initial_native_factory_retains_exact_product_wrapper_through_every_reconstruction(product_bootstrap, kind):
    f = product_bootstrap
    admission, observed = f.admit(kind)
    assert type(observed.gateway) is ProductGatewayFile
    assert observed.gateway.pin() == f.gateway.pin()
    selected, observation = bootstrap.observe_bootstrap_admission(f.directory, admission)
    assert type(observation.gateway) is ProductGatewayFile
    assert selected["installation"]["runtime_candidate"] == selection.candidate_selection(f.target)
    factory = bootstrap.BootstrapFactory(f.directory, admission)
    with project_lock(f.destination) as lease:
        async with factory(lease) as owner:
            assert type(owner.gateway) is ProductGatewayFile and owner.gateway.pin() == f.gateway.pin()
            assert owner.gateway is owner._original_gateway
            assert owner.gateway.artifacts.selection == f.target
            assert type(factory._observed.gateway) is ProductGatewayFile
            assert factory._observed.current is None if kind == "fresh" else factory._observed.current is not None
            owner._require_current()
            factory.require_current()
            # Reassigning to the original two-field pin cannot erase full-prefix custody.
            owner.gateway = ExactFile(owner.gateway.path, owner.gateway.sha256)
            with pytest.raises(CustodyError, match="original selected product gateway"):
                owner._require_current()
            owner.gateway = owner._original_gateway
        assert factory.source._closed


@pytest.mark.parametrize("kind", ["fresh", "restore"])
async def test_original_bootstrap_runtime_selection_cannot_change_after_observation(product_bootstrap, kind):
    f = product_bootstrap
    pin, observed = f.admit(kind)
    selected = deepcopy(deployment.read_receipt(f.directory)["runtime_candidate"])
    selected["assembly"]["sha256"] = "f" * 64
    deployment.update_receipt(f.directory, runtime_candidate=selected)
    with pytest.raises(CustodyError, match="installation changed"):
        bootstrap.read_bootstrap_admission(f.directory, pin)
    with pytest.raises(CustodyError): observed.require_current()


@pytest.mark.parametrize("defect", ["private-bin", "resource", "outside-prefix", "missing-selection", "changed-assembly"])
def test_initial_product_bootstrap_refuses_wrong_prefix_custody_or_source_selection(product_bootstrap, defect, tmp_path):
    f = product_bootstrap
    if defect == "private-bin": f.gateway.path.parent.chmod(0o700)
    elif defect == "resource":
        path = f.gateway.path.parent.parent / "libexec/meerkat-connect/resources.json"
        path.chmod(0o644); path.write_bytes(b"changed selected resource"); path.chmod(0o444)
    elif defect == "outside-prefix":
        enclosing = tmp_path / "unrelated private prefix"
        enclosing.mkdir(mode=0o700)
        artifacts = selection.selected_runtime_artifacts(f.target)
        other = adapter.install_meta_toolkit_prefix(artifacts, deployment_root=enclosing, require_current=lambda: None)
        f.fresh["gateway"] = ExactFile(other, f.gateway.sha256).pin()
    elif defect == "missing-selection":
        receipt = deployment.read_receipt(f.directory); receipt.pop("runtime_candidate")
        deployment.write_private(f.directory / "receipt.json", receipt)
    else: f.target.assembly.path.write_bytes(b"changed admitted producer assembly")
    with pytest.raises((ValueError, OSError)):
        f.admit()
