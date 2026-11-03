"""The packaged default entry also builds ordinary applications.

A product package may include the Meta RPC gateway release of its selected
Meta SDK. The packaged entry then selects it as the ordinary Meta gateway, so
an application without a connection setup helper builds through the ordinary
route. All bytes are inert; no gateway, SDK or server process runs.
"""
from copy import deepcopy
import hashlib
from pathlib import Path
import stat
from types import SimpleNamespace
import zipfile

import pytest

from launchkit import bootstrap, packaged_runtime
from launchkit.connect_installer_caller import ExactFile, _private_parent
from test_meta_toolkit_runtime_selection import product, no_product_execution, json_bytes, put
from test_distribution import bundle_source
from test_development_product_selection import development, guide_product_inputs


@pytest.fixture
def fresh_entry(monkeypatch):
    """Each row opens its own package in this process, as one launch does."""
    for name in ("_selected_target", "_selected_root", "_selected_ordinary_gateway"):
        monkeypatch.setattr(packaged_runtime, name, None)
    monkeypatch.setattr(bootstrap, "_selected_guide_runtime", None)


def ordinary_gateway(product, content=b"inert ordinary Meta RPC gateway, never execute\n"):
    pin = put(product.root / "ordinary-release/rpc_gateway", content)
    pin.path.chmod(0o700)
    return ExactFile(pin.path, pin.sha256)


def packaged(product, bundle_source, monkeypatch, tmp_path, *, gateway=True, meta_version="0.9.4"):
    from launchkit import product_distribution
    root, _, _, guide = guide_product_inputs(product, bundle_source, monkeypatch)
    target, _, _ = development(product, meta_version)
    selected = ordinary_gateway(product) if gateway else None
    output = tmp_path / "selected-product.zip"
    product_distribution.package(root, target, guide, output,
        **({"ordinary_gateway": selected} if gateway else {}))
    extracted = tmp_path / "selected-product"
    with zipfile.ZipFile(output) as archive:
        archive.extractall(extracted)
    return extracted, target, selected


def test_package_carries_the_selected_meta_release_gateway_for_ordinary_applications(
        product, bundle_source, tmp_path, monkeypatch, fresh_entry):
    from launchkit import product_distribution
    extracted, target, selected = packaged(product, bundle_source, monkeypatch, tmp_path)
    observed = product_distribution.verify(extracted)
    assert observed["ordinary"] == {"version": target.meta.version, "gateway": {
        "path": "vendor/ordinary/rpc_gateway", "sha256": selected.sha256,
        "bytes": selected.path.stat().st_size}}
    gateway = extracted / "vendor/ordinary/rpc_gateway"
    assert stat.S_IMODE(gateway.stat().st_mode) == 0o700
    assert stat.S_IMODE(gateway.parent.stat().st_mode) == 0o700
    readme = (extracted / "README.md").read_text()
    assert f"use the included Meta {target.meta.version} RPC gateway" in readme
    assert "does not include the ordinary Meta RPC gateway" not in readme
    # The packaged entry opens the same verified bytes; the registry's own
    # custody checks for an ordinary gateway hold for the extracted file.
    assert packaged_runtime.open_selection(extracted).meta.version == target.meta.version
    chosen = packaged_runtime.selected_ordinary_gateway()
    assert chosen == ExactFile(gateway.resolve(), selected.sha256)
    _private_parent(chosen.path)
    chosen.verify(executable=True)


def test_late_ordinary_locator_refusal_keeps_selection_retryable(
        product, bundle_source, tmp_path, monkeypatch, fresh_entry):
    extracted, _, selected = packaged(product, bundle_source, monkeypatch, tmp_path)
    gateway = extracted.resolve() / "vendor/ordinary/rpc_gateway"
    retained = gateway.with_name("retained-gateway")
    local = packaged_runtime._local
    changed = []

    def replace_ordinary_with_link(root, name):
        if name == "vendor/ordinary/rpc_gateway":
            gateway.rename(retained)
            gateway.symlink_to(retained)
            changed.append(name)
        return local(root, name)

    with monkeypatch.context() as fault:
        fault.setattr(packaged_runtime, "_local", replace_ordinary_with_link)
        try:
            with pytest.raises(ValueError, match="locator contains a link"):
                packaged_runtime.open_selection(extracted)
            assert changed == ["vendor/ordinary/rpc_gateway"]
            assert bootstrap.selected_guide_runtime() is None
            assert packaged_runtime._selected_target is None
            assert packaged_runtime._selected_root is None
            assert packaged_runtime._selected_ordinary_gateway is None
        finally:
            if changed:
                gateway.unlink()
                retained.rename(gateway)
    opened = packaged_runtime.open_selection(extracted)
    owner = bootstrap.selected_guide_runtime()
    assert owner is not None
    assert packaged_runtime.selected_ordinary_gateway() == ExactFile(gateway, selected.sha256)
    assert packaged_runtime.open_selection(extracted) is opened
    assert bootstrap.selected_guide_runtime() is owner


def test_refused_package_selection_preserves_an_existing_guide_owner(
        product, bundle_source, tmp_path, monkeypatch, fresh_entry):
    from launchkit import product_distribution

    extracted, _, _ = packaged(product, bundle_source, monkeypatch, tmp_path)
    guide = product_distribution.verify(extracted)["guide"]
    original = bootstrap.SelectedGuideRuntime(
        gateway=ExactFile(extracted.resolve() / guide["gateway"]["path"], guide["gateway"]["sha256"]),
        version=guide["version"], core_version=guide["core_version"],
        meta_source=guide["meta_source"], core_source=guide["core_source"])
    bootstrap.configure_selected_guide_runtime(original)
    with pytest.raises(bootstrap.GatewayInstallError, match="original guide runtime cannot be replaced"):
        packaged_runtime.open_selection(extracted)
    assert bootstrap.selected_guide_runtime() is original
    assert packaged_runtime._selected_target is None
    assert packaged_runtime._selected_root is None
    assert packaged_runtime._selected_ordinary_gateway is None


def test_package_without_the_gateway_keeps_the_helper_only_entry(
        product, bundle_source, tmp_path, monkeypatch, fresh_entry):
    from launchkit import product_distribution
    extracted, _, _ = packaged(product, bundle_source, monkeypatch, tmp_path, gateway=False)
    assert "ordinary" not in product_distribution.verify(extracted)
    assert not (extracted / "vendor/ordinary").exists()
    readme = (extracted / "README.md").read_text()
    assert "does not include the ordinary Meta RPC gateway" in readme
    assert "Ask the package owner for that package" in readme
    assert "use the included Meta" not in readme
    packaged_runtime.open_selection(extracted)
    assert packaged_runtime.selected_ordinary_gateway() is None


@pytest.mark.parametrize("drift", ["version", "sha256", "unbound-file", "bytes", "mode"])
def test_ordinary_gateway_binding_refuses_drift(product, bundle_source, tmp_path, monkeypatch, fresh_entry, drift):
    from launchkit import distribution, product_distribution
    extracted, target, _ = packaged(product, bundle_source, monkeypatch, tmp_path)
    manifest = extracted / distribution.MANIFEST
    value = product_distribution.verify(extracted)
    changed = deepcopy(value)
    if drift == "version":
        changed["ordinary"]["version"] = "0.9.3"
    elif drift == "sha256":
        changed["ordinary"]["gateway"]["sha256"] = "f" * 64
    elif drift == "unbound-file":
        changed.pop("ordinary")
    elif drift == "mode":
        changed["file_modes"]["vendor/ordinary/rpc_gateway"] = 0o755
    if drift == "bytes":
        (extracted / "vendor/ordinary/rpc_gateway").write_bytes(b"changed ordinary gateway\n")
    else:
        manifest.write_bytes(json_bytes(changed))
    with pytest.raises(distribution.DistributionError):
        product_distribution.verify(extracted)
    with pytest.raises(distribution.DistributionError):
        packaged_runtime.open_selection(extracted)


def test_package_refuses_gateway_bytes_that_differ_from_their_pin(product, bundle_source, tmp_path, monkeypatch):
    from launchkit import distribution, product_distribution
    root, _, _, guide = guide_product_inputs(product, bundle_source, monkeypatch)
    target, _, _ = development(product, "0.9.4")
    selected = ordinary_gateway(product)
    output = tmp_path / "refused.zip"
    with pytest.raises(distribution.DistributionError, match="ordinary Meta gateway"):
        product_distribution.package(root, target, guide, output,
            ordinary_gateway=ExactFile(selected.path, "e" * 64))
    assert not output.exists()
    with pytest.raises(distribution.DistributionError, match="exact product inputs"):
        product_distribution.package(root, target, guide, output, ordinary_gateway=str(selected.path))
    assert not output.exists()


@pytest.mark.parametrize("included", [True, False])
def test_packaged_entry_selects_the_ordinary_gateway_only_when_included(monkeypatch, included):
    target, gateway = object(), ExactFile(Path("/inert/vendor/ordinary/rpc_gateway"), "a" * 64)
    selections = []

    def open_selection(root=None):
        monkeypatch.setattr(packaged_runtime, "_selected_target", target)
        monkeypatch.setattr(packaged_runtime, "_selected_ordinary_gateway", gateway if included else None)
        return target

    server = SimpleNamespace(app=object(),
        select_local_application_runtime=lambda selected, **options: selections.append((selected, options)))
    monkeypatch.setattr(packaged_runtime, "open_selection", open_selection)
    monkeypatch.setattr(packaged_runtime.importlib, "import_module",
        lambda name: server if name == "launchkit.server" else pytest.fail(name))
    assert packaged_runtime.create_app() is server.app
    assert selections == [(target, {"ordinary_meta_gateway": gateway} if included else {})]


def test_selected_ordinary_gateway_needs_an_opened_package(monkeypatch):
    monkeypatch.setattr(packaged_runtime, "_selected_target", None)
    with pytest.raises(ValueError, match="Open the packaged runtime"):
        packaged_runtime.selected_ordinary_gateway()


def application_model_owner(tmp_path, monkeypatch):
    """An inert private application model source with Linux custody, for these rows only."""
    from launchkit import connect_application_startup as startup
    directory = tmp_path / "application-model-owner"
    directory.mkdir(mode=0o700)
    config = directory / "host.toml"
    config.write_bytes(b'[model]\nselected = "inert-application-custody-specimen"\n')
    config.chmod(0o600)
    root = directory / "xdg"
    root.mkdir(mode=0o700)
    monkeypatch.setattr(startup, "sys", SimpleNamespace(platform="linux"))
    monkeypatch.setenv("XDG_CONFIG_HOME", str(root))
    return startup._ModelConfiguration.observe({"host_config": {"path": str(config),
        "sha256": hashlib.sha256(config.read_bytes()).hexdigest()}, "credential_config_root": str(root)})


@pytest.fixture
def packaged_entry(monkeypatch):
    """create_app with its package opening and server selection observed, nothing else."""
    target, gateway = object(), ExactFile(Path("/inert/vendor/ordinary/rpc_gateway"), "a" * 64)
    selections = []
    state = SimpleNamespace(target=target, gateway=gateway, included=False, selections=selections)
    for name in ("_selected_target", "_selected_root", "_selected_ordinary_gateway", "_application_model_custody",
            "_private_model_route", "_private_model_custody"):
        monkeypatch.setattr(packaged_runtime, name, None)

    def open_selection(root=None):
        monkeypatch.setattr(packaged_runtime, "_selected_target", target)
        monkeypatch.setattr(packaged_runtime, "_selected_ordinary_gateway", gateway if state.included else None)
        return target

    state.server = SimpleNamespace(app=object(),
        select_local_application_runtime=lambda selected, **options: selections.append((selected, options)))
    monkeypatch.setattr(packaged_runtime, "open_selection", open_selection)
    monkeypatch.setattr(packaged_runtime.importlib, "import_module",
        lambda name: state.server if name == "launchkit.server" else pytest.fail(name))
    return state


@pytest.mark.parametrize("included", [False, True], ids=["helper-only", "ordinary-gateway"])
def test_packaged_entry_gives_its_applications_the_selected_application_model_custody(
        packaged_entry, tmp_path, monkeypatch, included):
    owner = application_model_owner(tmp_path, monkeypatch)
    packaged_entry.included = included
    packaged_runtime.configure_application_model_custody(owner)
    assert packaged_runtime.create_app() is packaged_entry.server.app
    expected = {"model_configuration": owner}
    if included:
        expected["ordinary_meta_gateway"] = packaged_entry.gateway
    assert packaged_entry.selections == [(packaged_entry.target, expected)]
    assert packaged_entry.selections[0][1]["model_configuration"] is owner


def test_packaged_entry_without_application_model_custody_is_unchanged(packaged_entry):
    assert packaged_runtime.create_app() is packaged_entry.server.app
    assert packaged_entry.selections == [(packaged_entry.target, {})]


def test_packaged_entry_forwards_explicit_retained_applications_without_selecting_them_for_builds(packaged_entry):
    retained = (object(),)
    assert packaged_runtime.create_app(retained_ordinary_applications=retained) is packaged_entry.server.app
    selected, options = packaged_entry.selections[0]
    assert selected is packaged_entry.target
    assert options == {"retained_ordinary_applications": retained}
    assert options["retained_ordinary_applications"] is retained


def test_packaged_entry_rejects_mutable_retained_choices_before_opening(packaged_entry):
    with pytest.raises(TypeError, match="tuple"):
        packaged_runtime.create_app(retained_ordinary_applications=[])
    assert packaged_runtime._selected_target is None and not packaged_entry.selections


def test_packaged_entry_keeps_configuration_supplier_with_retained_apps_and_model_owner(
        packaged_entry, tmp_path, monkeypatch):
    owner = application_model_owner(tmp_path, monkeypatch)
    packaged_runtime.configure_application_model_custody(owner)
    packaged_entry.included = True
    retained = (object(),)
    def declarations(guide_id, guide_version):
        pytest.fail("The original declarations are read only at Build, not package opening.")
    assert packaged_runtime.create_app(retained_ordinary_applications=retained,
        application_configuration_selection=declarations) is packaged_entry.server.app
    selected, options = packaged_entry.selections[0]
    assert selected is packaged_entry.target
    assert options["ordinary_meta_gateway"] is packaged_entry.gateway
    assert options["model_configuration"] is owner
    assert options["retained_ordinary_applications"] is retained
    assert options["application_configuration_selection"] is declarations


def test_application_model_custody_is_explicit_and_separate_from_the_guide_route(
        packaged_entry, tmp_path, monkeypatch):
    owner = application_model_owner(tmp_path, monkeypatch)
    # The Guide's private model route never supplies application credentials.
    packaged_runtime.configure_private_model_route({"provider": "openai", "model": "inert-model"}, owner)
    assert packaged_runtime.create_app() is packaged_entry.server.app
    assert packaged_entry.selections == [(packaged_entry.target, {})]


@pytest.mark.parametrize("misuse", ["after-open", "replaced", "not-an-owner", "changed-source"])
def test_application_model_custody_selection_refuses_misuse(packaged_entry, tmp_path, monkeypatch, misuse):
    from launchkit.connect_installer_caller import CustodyError
    other = None
    if misuse == "replaced":
        (tmp_path / "other").mkdir()
        other = application_model_owner(tmp_path / "other", monkeypatch)
    owner = application_model_owner(tmp_path, monkeypatch)
    if misuse == "after-open":
        packaged_runtime.open_selection()
        with pytest.raises(ValueError, match="before opening the packaged runtime"):
            packaged_runtime.configure_application_model_custody(owner)
    elif misuse == "replaced":
        packaged_runtime.configure_application_model_custody(owner)
        with pytest.raises(ValueError, match="cannot be replaced"):
            packaged_runtime.configure_application_model_custody(other)
    elif misuse == "not-an-owner":
        with pytest.raises(TypeError, match="application model configuration owner"):
            packaged_runtime.configure_application_model_custody(owner.selection())
    else:
        packaged_runtime.configure_application_model_custody(owner)
        owner.host_config.path.write_bytes(b"[model]\nselected = 'changed'\n")
        with pytest.raises(CustodyError):
            packaged_runtime.create_app()
    assert packaged_entry.selections == []


def test_one_owner_can_serve_both_the_guide_route_and_the_applications(packaged_entry, tmp_path, monkeypatch):
    owner = application_model_owner(tmp_path, monkeypatch)
    packaged_runtime.configure_private_model_route({"provider": "openai", "model": "inert-model"}, owner)
    packaged_runtime.configure_application_model_custody(owner)
    assert packaged_runtime.create_app() is packaged_entry.server.app
    assert packaged_entry.selections == [(packaged_entry.target, {"model_configuration": owner})]
    assert packaged_runtime._private_model_custody is owner


def test_application_model_custody_outside_the_process_root_refuses_when_selected(
        packaged_entry, tmp_path, monkeypatch):
    from launchkit.connect_installer_caller import CustodyError
    owner = application_model_owner(tmp_path, monkeypatch)
    other = tmp_path / "another-credential-root"
    other.mkdir(mode=0o700)
    monkeypatch.setenv("XDG_CONFIG_HOME", str(other))
    with pytest.raises(CustodyError, match="one model credential root"):
        packaged_runtime.configure_application_model_custody(owner)
    assert packaged_runtime._application_model_custody is None
    assert packaged_runtime.create_app() is packaged_entry.server.app
    assert packaged_entry.selections == [(packaged_entry.target, {})]
