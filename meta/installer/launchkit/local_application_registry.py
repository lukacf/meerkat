"""Retain original local application owners across deliberate Build and Install.

The trusted launcher supplies the real services and optional product selection.
Build creates source only after its workspace is reserved. Later callbacks use
the exact retained build and canonical installation, never discover a factory
from files or choose a runtime from an HTTP argument. A claimed setup action
can recover the original installed source after restart. Import and ordinary
owner lookup publish no source.
"""
from __future__ import annotations

from copy import deepcopy
from dataclasses import dataclass
import inspect
import json
from pathlib import Path
import re
from threading import RLock
import uuid

from .connect_installer_caller import CustodyError, MAX_INPUT_BYTES, _json, _private_directory, _read
from .deployment import DeploymentService, read_receipt
from .deployment_application_bootstrap import (FRESH_ENTRY_NAME, bootstrap_installation_binding,
    require_owner_installation, same_installation_apart_from_staged_update)
from .deployment_sources import read_package_source
from .deployment_updates import (FIRST_SETUP_UPDATE, ORDINARY_OWNER_BLOCKER, ORDINARY_REPLACEMENT_BLOCKER,
    OWNER_RECONNECT_UPDATE, checked_package, inspect_transaction, _candidate_source_selection, _same_toml)
from .forge_application_host import ForgeApplicationHost, application_setup_binding, owner_choices_match
from .guide_store import GuideStore
from .implementation import ImplementationService
from .implementation_workspace import ImplementationWorkspace
from .local_application_factory import LocalApplicationFactory, _replacement_supplier
from .local_application_source import (ORDINARY_SOURCE_UNAVAILABLE,
    create_local_application_source, _guide, _store_identity)
from .projects import DraftConflict
from .runtime_selection import MetaToolkitRuntimeTarget, parse_candidate_selection
from .ordinary_application_source import (OrdinaryApplicationHost, RetainedOrdinaryApplication,
    create_ordinary_application_source, ordinary_application)


@dataclass(frozen=True)
class _BuildOwner:
    guide_id: str
    build_id: str
    guide_version: int
    blueprint: dict
    factory: LocalApplicationFactory | None
    host: ForgeApplicationHost | OrdinaryApplicationHost


def _identity(value):
    try:
        if str(uuid.UUID(value)) != value:
            raise ValueError
    except (ValueError, TypeError, AttributeError) as error:
        raise CustodyError("Use this owner's original guide, build and installation identities.") from error
    return value


def _installation_binding(receipt, source):
    """Use the existing original source and Restore service interpretation."""
    return {**bootstrap_installation_binding(receipt),
        "accepted_source_digest": source["source_digest"]}


def _set_up(directory):
    """An installation that entered Fresh keeps the owner of its setup custody."""
    entry = directory / FRESH_ENTRY_NAME
    return entry.exists() or entry.is_symlink()


REDIRECTED = "The original installation cannot be redirected to another build or source owner."
MANAGED_MODELS_WITHOUT_CUSTODY = ("This plan's agents use host-managed models, and this Forge was started without "
    "an application model configuration for them. Choose models that use your own provider key, or start Forge "
    "with the application model configuration its product provides.")


class LocalApplicationRegistry:
    """One launcher-owned registry with separate build and installation owners.

    A manual host is an explicit independent selection and remains the exact
    same object. An automatic host exists only after this registry's actual
    workspace hook creates or deliberately reuses its selected source, or an
    admitted action authenticates its original accepted installed source.
    """

    def __init__(self, *, guide_store, implementation, deployment,
            runtime_target=None, manual_host=None, remote_control_selection=None,
            model_configuration=None, ordinary_meta_gateway=None, replacement_selection=None,
            retained_ordinary_applications=(), application_configuration_selection=None):
        _replacement_supplier(replacement_selection)
        if replacement_selection is not None and (manual_host is not None or remote_control_selection is not None):
            raise CustodyError("Select replacement inputs through their original local automatic application owner.")
        self.replacement_selection = replacement_selection
        if application_configuration_selection is not None and (
                not callable(application_configuration_selection) or manual_host is not None
                or remote_control_selection is not None or type(runtime_target) is not MetaToolkitRuntimeTarget):
            raise CustodyError("Select application field declarations through the original automatic local source owner.")
        if application_configuration_selection is not None:
            call = getattr(application_configuration_selection, "__call__", None)
            if any(inspect.iscoroutinefunction(value) or inspect.isasyncgenfunction(value)
                    for value in (application_configuration_selection, call)):
                raise CustodyError("Application declarations need a synchronous original source supplier.")
            try:
                inspect.signature(application_configuration_selection).bind("guide", 1)
            except (TypeError, ValueError) as error:
                raise CustodyError("Application declarations need their exact guide and version.") from error
        self.application_configuration_selection = application_configuration_selection
        if ordinary_meta_gateway is not None:
            from .connect_installer_caller import ExactFile, _private_parent
            if (type(ordinary_meta_gateway) is not ExactFile
                    or type(runtime_target) is not MetaToolkitRuntimeTarget
                    or manual_host is not None or remote_control_selection is not None):
                raise CustodyError("Select the original local ordinary Meta gateway with its accepted SDK product.")
            _private_parent(ordinary_meta_gateway.path)
            ordinary_meta_gateway.verify(executable=True)
        from .connect_application_update_startup import _file_identity
        self.ordinary_meta_gateway = ordinary_meta_gateway
        self._ordinary_gateway_selection = ordinary_meta_gateway
        self._ordinary_gateway_identity = (None if ordinary_meta_gateway is None else
            _file_identity(ordinary_meta_gateway.path))
        if (type(retained_ordinary_applications) is not tuple
                or retained_ordinary_applications and (manual_host is not None or remote_control_selection is not None
                    or type(runtime_target) is not MetaToolkitRuntimeTarget)):
            raise CustodyError("Select retained ordinary applications through their original local launcher.")
        from .connect_installer_caller import ExactFile, _private_parent
        retained, seen = [], set()
        for entry in retained_ordinary_applications:
            if (type(entry) is not RetainedOrdinaryApplication
                    or type(entry.runtime_target) is not MetaToolkitRuntimeTarget
                    or type(entry.gateway) is not ExactFile):
                raise CustodyError("Select each retained application's exact native runtime and ordinary gateway.")
            key = (_identity(entry.guide_id), _identity(entry.deployment_id))
            _identity(entry.build_id)
            if type(entry.package_sha256) is not str or not re.fullmatch("[0-9a-f]{64}", entry.package_sha256):
                raise CustodyError("Select the retained installation's exact accepted package digest.")
            if key in seen:
                raise CustodyError("Select one retained runtime per original installation.")
            seen.add(key)
            _private_parent(entry.gateway.path)
            entry.gateway.verify(executable=True)
            retained.append((entry, *key, entry.build_id, entry.package_sha256, entry.runtime_target, entry.gateway,
                entry.gateway.path, entry.gateway.sha256, _file_identity(entry.gateway.path)))
        self.retained_ordinary_applications = retained_ordinary_applications
        self._retained_ordinary_inputs = tuple(retained)
        if remote_control_selection is not None:
            from .remote_product_control import ProductControlSelection
            if (type(remote_control_selection) is not ProductControlSelection
                    or type(runtime_target) is not MetaToolkitRuntimeTarget
                    or manual_host is not None):
                raise CustodyError("Select the original local and remote product source owners.")
        if (type(guide_store) is not GuideStore
                or type(implementation) is not ImplementationService
                or not isinstance(deployment, DeploymentService)
                or implementation.guide.store is not guide_store
                or deployment.guide.store is not guide_store
                or deployment.implementation is not implementation
                or runtime_target is not None and type(runtime_target) is not MetaToolkitRuntimeTarget
                or manual_host is not None and type(manual_host) is not ForgeApplicationHost):
            raise CustodyError("The registry needs its actual services and trusted launcher selections.")
        self.guide_store, self.implementation, self.deployment = guide_store, implementation, deployment
        self.runtime_target, self.manual_host = runtime_target, manual_host
        self.model_configuration = deepcopy(model_configuration)
        self._model_custody = None
        if model_configuration is not None:
            from .connect_application_startup import _ModelConfiguration
            self._model_custody = _ModelConfiguration.observe(self.model_configuration)
        self._original_model_custody = self._model_custody
        self.remote_control_selection = remote_control_selection
        self.remote_runtime_target = remote_control_selection.runtime_target if remote_control_selection is not None else None
        self._configured = (guide_store, implementation, deployment, runtime_target, manual_host,
            remote_control_selection, self.remote_runtime_target, replacement_selection, retained_ordinary_applications,
            application_configuration_selection)
        self._roots = (guide_store.root, implementation.root, deployment.root)
        self._storage = None
        self._lock = RLock()
        self._builds = {}
        self._ordinary_builds = set()
        self._attempts = set()
        self._installations = {}

    def _selected_model_configuration(self):
        selected = self._model_custody
        if selected is not self._original_model_custody:
            raise CustodyError("Preserve the registry's original model configuration owner.")
        original = None if selected is None else selected.selection()
        if self.model_configuration != original:
            raise CustodyError("Preserve the registry's original private model selection.")
        if selected is not None:
            selected.require_environment()
            selected.require_filesystem_current()
        return deepcopy(original)

    def _require_registry(self):
        self._selected_model_configuration()
        from .connect_application_update_startup import _file_identity
        if (self.ordinary_meta_gateway is not self._ordinary_gateway_selection
                or self.ordinary_meta_gateway is not None and
                    _file_identity(self.ordinary_meta_gateway.path) != self._ordinary_gateway_identity):
            raise CustodyError("Preserve the original separately selected ordinary Meta gateway.")
        for entry, guide_id, deployment_id, build_id, package_sha256, target, gateway, path, sha256, identity in self._retained_ordinary_inputs:
            if (entry.guide_id != guide_id or entry.deployment_id != deployment_id
                    or entry.build_id != build_id or entry.package_sha256 != package_sha256
                    or entry.runtime_target is not target or entry.gateway is not gateway
                    or (gateway.path, gateway.sha256) != (path, sha256)
                    or _file_identity(path) != identity):
                raise CustodyError("Preserve each retained installation's original runtime and gateway selection.")
        if (any(current is not original for current, original in zip(
                (self.guide_store, self.implementation, self.deployment, self.runtime_target, self.manual_host,
                    self.remote_control_selection, self.remote_runtime_target, self.replacement_selection,
                    self.retained_ordinary_applications, self.application_configuration_selection),
                self._configured))
                or self.remote_control_selection is not None
                    and self.remote_control_selection.runtime_target is not self.remote_runtime_target
                or (self.guide_store.root, self.implementation.root, self.deployment.root) != self._roots
                or self.implementation.guide.store is not self.guide_store
                or self.deployment.guide.store is not self.guide_store
                or self.deployment.implementation is not self.implementation):
            raise CustodyError("Preserve the registry's original services and source selections.")
        if self._storage is not None and _store_identity(self.guide_store) != self._storage:
            raise CustodyError("The registry's original guide storage disappeared or changed.")

    def _current_plan(self, guide_id, guide_version):
        self._require_registry()
        if type(guide_version) is not int or guide_version < 1:
            raise CustodyError("Build needs its exact current guide version.")
        if self._storage is None:
            self._storage = _store_identity(self.guide_store)
        result = _guide(self.guide_store, _identity(guide_id), guide_version, self._storage)
        self._require_registry()
        return result.model_dump(mode="json")

    def _runtime_target_for_blueprint(self, blueprint):
        """Choose held source inputs; this does not grant remote admission.

        Live Build uses its current validated Blueprint. Installed recovery uses
        the original checked installed Blueprint and signed accepted source.
        Keep local Linux separate when it is the actual launcher selection.
        """
        self._require_registry()
        if self.remote_control_selection is not None and (
                blueprint.get("target") == "gcp" or blueprint.get("target") == "linux"
                and self.runtime_target.target != "linux-x86_64"):
            return self.remote_runtime_target
        return self.runtime_target

    def initial_build_options(self, guide_id, guide_version):
        """Check the original build inputs; actual Build owns source publication."""
        with self._lock:
            self._require_registry()
            if self.manual_host is not None and self.manual_host.applies(guide_id):
                self._current_plan(guide_id, guide_version)
                return self.manual_host.initial_build_options(guide_id)
            if self.runtime_target is None:
                return {}
            plan = self._current_plan(guide_id, guide_version)
            from .compiler import _product_application_supported
            from .blueprint import Blueprint
            if (ordinary_application(plan) and self.ordinary_meta_gateway is None
                    and not _product_application_supported(Blueprint.model_validate(plan),
                        runtime_target=self._runtime_target_for_blueprint(plan))):
                raise CustodyError(ORDINARY_SOURCE_UNAVAILABLE)
            return {"runtime_target": self._runtime_target_for_blueprint(plan)}

    def legacy_ordinary_continuation(self, guide_id, build_id):
        """Read the predecessor's signed route and retain its launcher gateway.

        A changed guide may need newly compiled source, but it does not migrate
        an ordinary installation to Connect. Authored file presence alone never
        selects this route: the source must be in the host-signed baseline.
        """
        from .local_application_recovery import _pin
        from .ordinary_application_source import _source_files
        from .application_inventory_source import selects_application_inventory
        import json
        with self._lock:
            self._require_registry()
            guide_id, build_id = _identity(guide_id), _identity(build_id)
            directory = self.implementation._directory(guide_id, build_id)
            frozen = json.loads(_pin(directory / "input.json").read())
            if frozen.get("guide_id") != guide_id:
                raise CustodyError("Continue only this guide's original Build source.")
            old = ImplementationWorkspace(directory / "workspace", frozen["blueprint"])
            with old._locked():
                state = old._state()
                files = old._files(state)
                selected = {item.path: item.as_bytes() for item in files}
                candidate = old._runtime_candidate(state, files)
                target = parse_candidate_selection(candidate) if candidate is not None else None
                if (type(target) is not MetaToolkitRuntimeTarget
                        or not ordinary_application(frozen["blueprint"])
                        or selects_application_inventory(selected)):
                    return False
                if self.ordinary_meta_gateway is None:
                    raise CustodyError(ORDINARY_SOURCE_UNAVAILABLE)
                expected = _source_files(guide_id, self.ordinary_meta_gateway)
                if (candidate != frozen.get("runtime_candidate")
                        or not expected.keys() <= set(old._protected_application_paths(state))
                        or any(selected.get(name) != data for name, data in expected.items())):
                    raise CustodyError("Continue with the original ordinary gateway and protected Console source.")
                self.ordinary_meta_gateway.read()
            self._require_registry()
            return True

    def _retained_ordinary_application(self, guide_id, deployment_id):
        self._require_registry()
        return next((entry for entry in self.retained_ordinary_applications
            if entry.guide_id == guide_id and entry.deployment_id == deployment_id), None)

    def _has_ordinary_runtime(self, guide_id, deployment_id):
        self._require_registry()
        return self.ordinary_meta_gateway is not None or self._retained_ordinary_application(
            guide_id, deployment_id) is not None

    def _ordinary_runtime_selection(self, guide_id, blueprint, *, installation=None):
        """Use a held installation choice, never promote a receipt into authority."""
        self._require_registry()
        if installation is not None and ordinary_application(blueprint):
            if installation.get("guide_id") != guide_id:
                raise CustodyError("The retained runtime belongs to its original installation.")
            entry = self._retained_ordinary_application(guide_id, installation.get("deployment_id"))
            if entry is not None:
                if (installation.get("build_id") != entry.build_id
                        or installation.get("package_sha256") != entry.package_sha256):
                    raise CustodyError("Recover only the retained installation's explicitly selected accepted source.")
                return entry.runtime_target, entry.gateway
        return self._runtime_target_for_blueprint(blueprint), self.ordinary_meta_gateway

    def prepare_workspace(self, guide_id, build_id, workspace, *, guide_version,
            source_build_id=None):
        """Select source only inside this actual reserved Build workspace.

        The existing Build caller supplies its frozen current version and, for
        same-source continuation, the actual explicitly selected predecessor.
        A changed plan requires a new source selection, not that predecessor.
        Interrupted attempts stay reserved and cannot be retried by lookup.
        """
        with self._lock:
            self._require_registry()
            key = (_identity(guide_id), _identity(build_id))
            if type(workspace) is not ImplementationWorkspace:
                raise CustodyError("Build must supply its actual reserved implementation workspace.")
            directory = self.implementation._directory(guide_id, build_id)
            if workspace.root != directory / "workspace":
                raise CustodyError("The source owner differs from this Build's canonical workspace.")
            directory_identity = _private_directory(directory)
            workspace_identity = _private_directory(workspace.root)
            plan = self._current_plan(guide_id, guide_version)
            if workspace.blueprint.model_dump(mode="json") != plan:
                raise CustodyError("The reserved workspace differs from the exact current guide plan.")
            if key in self._attempts:
                raise CustodyError("This original source preparation is already retained; do not replay it.")
            selection = workspace.framework_provenance().get("runtime_candidate")
            target = parse_candidate_selection(selection) if selection is not None else None
            manual = self.manual_host if self.manual_host is not None and self.manual_host.applies(guide_id) else None
            if manual is None and type(target) is not MetaToolkitRuntimeTarget:
                if source_build_id is not None:
                    raise CustodyError("An ordinary workspace cannot replace a selected product source owner.")
                self._ordinary_builds.add(key)
                return
            previous = None
            if source_build_id is not None:
                previous = self._builds.get((guide_id, _identity(source_build_id)))
                if previous is None or previous.blueprint != plan or previous.host._target != target:
                    raise CustodyError("Continuation needs its exact retained source, plan and runtime owner.")
                previous.host.require_current()
            if manual is None and (target != self._runtime_target_for_blueprint(plan) and previous is None):
                raise CustodyError("The workspace product differs from the trusted launcher selection.")
            # Retain this attempt before the first possible source publication.
            self._attempts.add(key)
            factory = None
            from .application_inventory_source import selects_application_inventory
            authored_inventory = selects_application_inventory(
                {"deployment.json": workspace.read_file("deployment.json").encode("utf-8")})
            if manual is not None:
                host = manual
            elif (not authored_inventory and ordinary_application(plan)
                    and self.ordinary_meta_gateway is not None):
                if previous is not None:
                    if type(previous.host) is not OrdinaryApplicationHost:
                        raise CustodyError("Continue the original ordinary application source owner.")
                    host = OrdinaryApplicationHost(guide_id=guide_id, runtime_target=target,
                        gateway=self.ordinary_meta_gateway, blueprint=plan,
                        definition=previous.host._definition, protected=previous.host._protected,
                        require_current=self._require_registry,
                        model_configuration=self._selected_model_configuration())
                else:
                    host = create_ordinary_application_source(guide_id=guide_id, runtime_target=target,
                        gateway=self.ordinary_meta_gateway, blueprint=plan,
                        definition=workspace.read_file("config/mob.toml"),
                        output_directory=directory / "ordinary-application-source",
                        require_current=self._require_registry,
                        model_configuration=self._selected_model_configuration())
            elif previous is not None:
                original = previous.factory
                if type(original) is not LocalApplicationFactory:
                    raise CustodyError("Automatic continuation needs its original local source factory.")
                factory = LocalApplicationFactory(guide_store=self.guide_store, guide_id=guide_id,
                    runtime_target=target, acquisition_selection=original.acquisition_selection,
                    viewer_selection=original.viewer_selection,
                    console_setup_selection=original.console_setup_selection,
                    model_configuration=self._selected_model_configuration(),
                    replacement_selection=self.replacement_selection)
                host = factory()
            else:
                # Select one complete source document for this exact new Build.
                # Continuations above retain their original protected selection.
                configuration = (self.application_configuration_selection(guide_id, guide_version)
                    if self.application_configuration_selection is not None else None)
                if inspect.iscoroutine(configuration):
                    configuration.close()
                self._require_registry()
                factory = create_local_application_source(guide_store=self.guide_store,
                    guide_id=guide_id, guide_version=guide_version, runtime_target=target,
                    output_directory=directory / "local-application-source",
                    model_configuration=self._selected_model_configuration(),
                    replacement_selection=self.replacement_selection,
                    configuration_source=configuration)
                host = factory()
            host.prepare_workspace(guide_id, build_id, workspace)
            if (_private_directory(directory) != directory_identity
                    or _private_directory(workspace.root) != workspace_identity
                    or self._current_plan(guide_id, guide_version) != plan
                    or parse_candidate_selection(workspace.framework_provenance().get("runtime_candidate")) != target):
                raise CustodyError("The original reserved Build changed during source preparation.")
            host.require_current()
            self._builds[key] = _BuildOwner(guide_id, build_id, guide_version,
                deepcopy(plan), factory, host)

    def host_for_build(self, guide_id, build_id):
        """Return only this process's actual retained original Build host."""
        with self._lock:
            self._require_registry()
            chosen = self._builds.get((_identity(guide_id), _identity(build_id)))
            if chosen is None:
                if self.manual_host is not None and self.manual_host.applies(guide_id):
                    self.manual_host.require_current()
                    return self.manual_host
                if (guide_id, build_id) in self._ordinary_builds:
                    return None
                raise CustodyError("This build has no retained local source owner. Preserve its saved source.")
            chosen.host.require_current()
            return chosen.host

    def _receipt_host(self, guide_id, deployment_id, *, staged_update_allowed=True):
        self._require_registry()
        guide_id, deployment_id = _identity(guide_id), _identity(deployment_id)
        directory = self.deployment._directory(guide_id, deployment_id)
        if self.deployment._directory(guide_id) != directory:
            raise CustodyError("Preserve this installation's exact current canonical selection.")
        receipt = read_receipt(directory)
        if receipt.get("guide_id") != guide_id or receipt.get("deployment_id") != deployment_id:
            raise CustodyError("The canonical receipt belongs to another installation.")
        key = (guide_id, deployment_id)
        retained = self._installations.get(key)
        if receipt.get("source_kind", "application") != "application":
            if retained is not None:
                raise CustodyError("The original installed application source kind changed.")
            return None, None
        build_id = _identity(receipt.get("build_id"))
        manual = self.manual_host if self.manual_host is not None and self.manual_host.applies(guide_id) else None
        if _set_up(directory) or retained is not None and type(retained[0]) is OrdinaryApplicationHost:
            # Its retained setup host holds the installation's custody. Another
            # build's host never becomes its owner, even one built here; without
            # a retained entry only a claimed recovery rebuilds that owner. The
            # retained owner of an application without a setup helper keeps its
            # installation through the installation's own completed updates.
            chosen = None
            host = retained[0] if retained is not None else manual
        else:
            chosen = self._builds.get((guide_id, build_id))
            host = chosen.host if chosen is not None else manual
        selection = receipt.get("runtime_candidate")
        target = parse_candidate_selection(selection) if selection is not None else None
        if host is None and (retained is not None or type(target) is MetaToolkitRuntimeTarget):
            raise CustodyError("This product installation needs its retained original host; saved files cannot select it.")
        if host is not None and (target != host._target
                or chosen is not None and receipt.get("guide_version") != chosen.guide_version):
            raise CustodyError("The canonical installation changed its original build or product selection.")
        _, files = checked_package(directory, "package.zip", receipt["package_sha256"])
        source = read_package_source(files, receipt)
        report = json.loads(files["implementation-report.json"][0])
        if (not _same_toml(_candidate_source_selection(files), selection)
                or not _same_toml(report.get("framework", {}).get("runtime_candidate"), selection)):
            raise CustodyError("The canonical package changed its original runtime selection.")
        if host is None:
            return None, None
        if not owner_choices_match(host, files):
            raise CustodyError("The canonical package differs from this original protected application owner.")
        binding = _installation_binding(receipt, source)
        if retained is not None:
            if retained[0] is not host:
                raise CustodyError(REDIRECTED)
            if not staged_update_allowed:
                # Only Apply's replacement lookup compares the update pointer itself.
                if retained[1] != binding:
                    raise CustodyError(REDIRECTED)
            elif not same_installation_apart_from_staged_update(retained[1], binding, receipt):
                # The retained anchor moves only through this installation's
                # own completed source updates.
                require_owner_installation(directory, receipt, {name: value for name, value
                    in retained[1].items() if name != "accepted_source_digest"}, REDIRECTED,
                    setup_entry=type(host) is not OrdinaryApplicationHost)
        current = read_receipt(directory)
        if (self.deployment._directory(guide_id) != directory
                or _installation_binding(current, source) != binding):
            raise CustodyError("The canonical installation changed during original owner lookup.")
        self._require_registry()
        return host, binding

    def continues_installed_source(self, guide_id, deployment_id, build_id, source_digest):
        """Whether an accepted Build continues the installed Build's own signed source.

        Its signed accepted-baseline chain must reach the installed Build, so it
        ends at the same initial Build and source owner. The walk is bounded and
        read-only, and holds no registry lock while it reads Build storage.
        """
        from .local_application_recovery import _accepted_build
        with self._lock:
            self._require_registry()
            guide_id, deployment_id = _identity(guide_id), _identity(deployment_id)
            directory = self.deployment._directory(guide_id, deployment_id)
            receipt = read_receipt(directory)
            _, files = checked_package(directory, "package.zip", receipt["package_sha256"])
            installed = (receipt["build_id"], read_package_source(files, receipt)["source_digest"])
            blueprint = json.loads(files["blueprint.json"][0])
            selection = receipt.get("runtime_candidate")
            target = parse_candidate_selection(selection) if selection is not None else None
        current, seen = (build_id, source_digest), set()
        try:
            while current != installed:
                if current[0] in seen or current[0] == installed[0] or len(seen) >= 128:
                    return False
                seen.add(_identity(current[0]))
                _, provenance, *_ = _accepted_build(self, guide_id, current[0], current[1], blueprint, target)
                previous = provenance.get("previous")
                if (provenance.get("basis") != "accepted-baseline" or not isinstance(previous, dict)
                        or set(previous) != {"build_id", "source_digest"}):
                    return False
                current = (previous["build_id"], previous["source_digest"])
        except (CustodyError, DraftConflict, OSError, KeyError, TypeError, ValueError):
            return False
        self._require_registry()
        return True

    def selected_host(self, guide_id, deployment_id, *, staged_update_allowed=True):
        """Read the exact original installation owner without native effects."""
        with self._lock:
            host, binding = self._receipt_host(guide_id, deployment_id,
                staged_update_allowed=staged_update_allowed)
            if host is not None:
                key = (guide_id, deployment_id)
                if key not in self._installations:
                    self._installations[key] = (host, binding)
            return host

    def _require_selected(self, deployment, guide_id, deployment_id):
        if deployment is not self.deployment:
            raise CustodyError("Use the registry's original deployment service.")
        return self.selected_host(guide_id, deployment_id)

    def _ordinary_install(self, guide_id, build_id):
        """Preserve existing ordinary Install without selecting a source host.

        The original implementation reader joins its saved target to its signed
        workspace. Normal deployment.submit still owns all accepted-build,
        current-guide, package and installation admission checks afterward.
        """
        self._require_registry()
        directory = self.implementation._directory(guide_id, build_id)
        if self.implementation._directory(guide_id) != directory:
            raise CustodyError("Install needs the currently selected original application build.")
        directory_identity = _private_directory(directory)
        workspace_identity = _private_directory(directory / "workspace")
        frozen_bytes = (directory / "input.json").read_bytes()
        receipt_bytes = (directory / "receipt.json").read_bytes()
        frozen, receipt = json.loads(frozen_bytes), json.loads(receipt_bytes)
        if (receipt.get("guide_id") != guide_id or receipt.get("build_id") != build_id
                or frozen.get("guide_id") != guide_id
                or not _same_toml(receipt.get("framework", {}).get("runtime_candidate"),
                    frozen.get("framework", {}).get("runtime_candidate"))):
            raise CustodyError("The original ordinary build receipt or selected source changed.")
        selection = self.implementation._retained_runtime_candidate(directory, frozen)
        target = parse_candidate_selection(selection) if selection is not None else None
        if type(target) is MetaToolkitRuntimeTarget:
            raise CustodyError("This product Build needs its retained original local source host.")
        if (self.implementation._directory(guide_id) != directory
                or _private_directory(directory) != directory_identity
                or _private_directory(directory / "workspace") != workspace_identity
                or (directory / "input.json").read_bytes() != frozen_bytes
                or (directory / "receipt.json").read_bytes() != receipt_bytes):
            raise CustodyError("The selected ordinary application build changed during Install validation.")
        self._require_registry()

    def validate_settings(self, guide_id, settings, *, build_id=None):
        if build_id is None:
            if self.manual_host is not None and self.manual_host.applies(guide_id):
                return self.manual_host.validate_settings(guide_id, settings)
            if self.runtime_target is None:
                self._require_registry()
                return
            raise CustodyError("Install needs its exact retained accepted build owner.")
        with self._lock:
            self._require_registry()
            key = (_identity(guide_id), _identity(build_id))
            if (key not in self._builds and key not in self._ordinary_builds
                    and not (self.manual_host is not None and self.manual_host.applies(guide_id))):
                if self.runtime_target is not None:
                    from .local_application_recovery import recover_local_build
                    from .ordinary_application_source import recover_ordinary_application_source
                    recovered = (recover_ordinary_application_source(self, guide_id, build_id)
                        if self.ordinary_meta_gateway is not None else None)
                    if recovered is None:
                        recovered = recover_local_build(self, guide_id, build_id)
                    if recovered is not None:
                        version, blueprint, factory, selected = recovered
                        self._builds[key] = _BuildOwner(guide_id, build_id, version,
                            deepcopy(blueprint), factory, selected)
                if key not in self._builds:
                    return self._ordinary_install(guide_id, build_id)
        host = self.host_for_build(guide_id, build_id)
        if host is None:
            return
        from .local_application_recovery import bind_accepted_build_inventory
        bind_accepted_build_inventory(self, host, guide_id, build_id, publish=True)
        # Start would refuse host-managed models without this launcher's
        # application model configuration; Install says so before any worker.
        from .connect_application_startup import _managed_model_classes
        if (_managed_model_classes({"classes": host._blueprint["agent_classes"]})
                and self._selected_model_configuration() is None):
            raise DraftConflict(MANAGED_MODELS_WITHOUT_CUSTODY)
        host.validate_settings(guide_id, settings)
        if self.host_for_build(guide_id, build_id) is not host:
            raise CustodyError("The original Build host changed during settings validation.")

    def validate_installed_settings(self, guide_id, receipt, destination, files, settings):
        deployment_id = receipt.get("deployment_id")
        host = self.selected_host(guide_id, deployment_id)
        if host is not None:
            host.validate_installed_settings(guide_id, receipt, destination, files, settings)
            if self.selected_host(guide_id, deployment_id) is not host:
                raise CustodyError("The original installed settings owner changed.")

    def retain_installation_inputs(self, deployment, guide_id, deployment_id):
        if self._reconnects_saved_setup(deployment, guide_id, deployment_id):
            # After a Forge restart, a retried Install's saved setup belongs to
            # its claimed setup action: Continue setup rebuilds the original
            # owner from the installed source. Completion retains nothing new.
            return
        self._recover_install_build(deployment, guide_id, deployment_id)
        host = self._require_selected(deployment, guide_id, deployment_id)
        if host is not None:
            result = host.retain_installation_inputs(deployment, guide_id, deployment_id)
            if self._require_selected(deployment, guide_id, deployment_id) is not host:
                raise CustodyError("The original Install retainer changed during completion.")
            return result

    def _reconnects_saved_setup(self, deployment, guide_id, deployment_id):
        """Whether only Continue setup can reconnect this installation's owner.

        The reconnect hint reads the installation's saved setup without
        constructing a host. A product installation whose Install never saved
        its setup has no such hint, and its Build owner is recovered instead.
        """
        try:
            return self._reconnect_status(deployment, guide_id, deployment_id) is not None
        except FileNotFoundError:
            return False

    def _recover_install_build(self, deployment, guide_id, deployment_id):
        """Recover the installed Build's owner for a retried Install, as Install does.

        After a Forge restart this process holds no Build owner. The same
        signed-acceptance readers as a deliberate Install rebuild it; a set-up
        installation or a retained one keeps its existing owner.
        """
        self._require_registry()
        if deployment is not self.deployment:
            raise CustodyError("Use the registry's original deployment service.")
        if self.runtime_target is None or self.manual_host is not None:
            return
        guide_id, deployment_id = _identity(guide_id), _identity(deployment_id)
        directory = deployment._directory(guide_id, deployment_id)
        receipt = read_receipt(directory)
        if receipt.get("source_kind", "application") != "application":
            return
        build_id = _identity(receipt.get("build_id"))
        key, build_key = (guide_id, deployment_id), (guide_id, build_id)
        with self._lock:
            if key in self._installations or build_key in self._builds or _set_up(directory):
                return
        from .local_application_recovery import recover_local_build
        from .ordinary_application_source import recover_ordinary_application_source
        recovered = (recover_ordinary_application_source(self, guide_id, build_id)
            if self.ordinary_meta_gateway is not None else None)
        if recovered is None:
            recovered = recover_local_build(self, guide_id, build_id)
        if recovered is not None:
            version, blueprint, factory, selected = recovered
            with self._lock:
                self._builds.setdefault(build_key, _BuildOwner(guide_id, build_id, version,
                    deepcopy(blueprint), factory, selected))

    def _selected_startup(self, guide_id, deployment_id):
        host = self.selected_host(guide_id, deployment_id)
        if host is None:
            raise CustodyError("This installation has no trusted local application startup owner.")
        result = host._selected_startup(guide_id, deployment_id)
        if self.selected_host(guide_id, deployment_id) is not host:
            raise CustodyError("The original application startup owner changed.")
        return result

    async def prepare_application(self, deployment, guide_id, deployment_id):
        recovered = await self._recover_installation(deployment, guide_id, deployment_id)
        host = self._require_selected(deployment, guide_id, deployment_id)
        if host is None:
            raise CustodyError("This installation has no trusted local preparation owner.")
        if recovered:
            if type(host) is OrdinaryApplicationHost:
                return await host.prepare_application(deployment, guide_id, deployment_id)
            try:
                retained = host._startup_selection(guide_id, deployment_id)
            except FileNotFoundError:
                retained = None
            if retained is not None:
                await host.continue_application_startup(deployment, guide_id, deployment_id)
                result = host._selected_startup(guide_id, deployment_id)
                if self._require_selected(deployment, guide_id, deployment_id) is not host:
                    raise CustodyError("The original recovered setup owner changed.")
                return result
        result = await host.prepare_application(deployment, guide_id, deployment_id)
        if self._require_selected(deployment, guide_id, deployment_id) is not host:
            raise CustodyError("The original application preparation owner changed.")
        return result

    async def continue_application_startup(self, deployment, guide_id, deployment_id):
        await self._recover_installation(deployment, guide_id, deployment_id)
        host = self._require_selected(deployment, guide_id, deployment_id)
        if host is not None:
            result = await host.continue_application_startup(deployment, guide_id, deployment_id)
            if self._require_selected(deployment, guide_id, deployment_id) is not host:
                raise CustodyError("The original application continuation owner changed.")
            return result

    async def _recover_installation(self, deployment, guide_id, deployment_id):
        """Authenticate the original missing owner within the claimed action."""
        self._require_registry()
        if deployment is not self.deployment:
            raise CustodyError("Use the registry's original deployment service.")
        if self.runtime_target is None or self.manual_host is not None:
            return False
        guide_id, deployment_id = _identity(guide_id), _identity(deployment_id)
        with self._lock:
            directory = deployment._directory(guide_id, deployment_id)
            if deployment._directory(guide_id) != directory:
                raise CustodyError("Preserve this installation's exact current canonical selection.")
            receipt = read_receipt(directory)
            if receipt.get("source_kind", "application") != "application":
                return False
            build_id = _identity(receipt.get("build_id"))
            key, build_key = (guide_id, deployment_id), (guide_id, build_id)
            # A build host retained here never owns a set-up installation.
            set_up = _set_up(directory)
            if key in self._installations or not set_up and build_key in self._builds:
                return False
            selection = receipt.get("runtime_candidate")
            if selection is None or type(parse_candidate_selection(selection)) is not MetaToolkitRuntimeTarget:
                return False
        if not deployment.lock.locked():
            raise CustodyError("Reconnect setup through the original claimed installation action.")
        if self._has_ordinary_runtime(guide_id, deployment_id):
            from .ordinary_application_source import recover_ordinary_application_source
            directory, receipt, stopped = deployment._stopped_application_owner(guide_id, deployment_id)
            _, files = checked_package(directory, "package.zip", receipt["package_sha256"])
            if ordinary_application(_json(files["blueprint.json"][0])):
                deployment._installed_configuration(guide_id, deployment_id)
                recovered = recover_ordinary_application_source(self, guide_id, build_id, installed=(receipt, files))
                stopped()
                with self._lock:
                    if key in self._installations or build_key in self._builds or recovered is None:
                        raise CustodyError("The original ordinary recovery owner changed before retention.")
                    version, blueprint, factory, host = recovered
                    host.retain_installation_inputs(deployment, guide_id, deployment_id)
                    source = read_package_source(files, receipt)
                    stopped()
                    # An explicit retained runtime belongs only to this installed
                    # owner. Do not promote it into a reusable Build/Install host.
                    if self._retained_ordinary_application(guide_id, deployment_id) is None:
                        self._builds[build_key] = _BuildOwner(guide_id, build_id, version, deepcopy(blueprint), factory, host)
                    self._installations[key] = (host, _installation_binding(receipt, source))
                    return True
        from .local_application_recovery import RecoveredLocalApplication, recover_local_application
        recovered = await recover_local_application(self, guide_id, deployment_id)
        if recovered is None:
            return False
        with self._lock:
            if (type(recovered) is not RecoveredLocalApplication
                    or (recovered.guide_id, recovered.deployment_id, recovered.build_id)
                        != (guide_id, deployment_id, build_id)
                    or type(recovered.factory) is not LocalApplicationFactory
                    or type(recovered.host) is not ForgeApplicationHost
                    or recovered.factory.host is not recovered.host
                    or recovered.host._blueprint != recovered.blueprint
                    or recovered.host._target != self._runtime_target_for_blueprint(recovered.blueprint)
                    or key in self._installations or not set_up and build_key in self._builds):
                raise CustodyError("The original installation recovery owner changed before retention.")
            recovered.host.require_current()
            current_directory, current, stopped = deployment._stopped_application_owner(guide_id, deployment_id)
            _, files = checked_package(current_directory, "package.zip", current["package_sha256"])
            source = read_package_source(files, current)
            stopped()
            if (current_directory != directory
                    or _installation_binding(current, source) != recovered.binding):
                raise CustodyError("The original installed source changed before recovery retention.")
            self._require_registry()
            # A build host already retained here stays that build's owner only.
            self._builds.setdefault(build_key, _BuildOwner(guide_id, build_id, recovered.guide_version,
                deepcopy(recovered.blueprint), recovered.factory, recovered.host))
            self._installations[key] = (recovered.host, deepcopy(recovered.binding))
            return True

    def _selected_replacement(self, guide_id, deployment_id, update_id):
        if self.replacement_selection is None:
            return self._no_replacement(guide_id, deployment_id, update_id)
        host = self.selected_host(guide_id, deployment_id, staged_update_allowed=False)
        if host is None:
            raise CustodyError("This installation has no trusted replacement source owner.")
        if not callable(getattr(host, "_selected_replacement", None)):
            # A helperless application has no installed replacement owner.
            raise CustodyError("The original installation cannot be redirected to another build or source owner.")
        result = host._selected_replacement(guide_id, deployment_id, update_id)
        if self.selected_host(guide_id, deployment_id, staged_update_allowed=False) is not host:
            raise CustodyError("The original installed replacement owner changed.")
        return result

    def _no_replacement(self, guide_id, deployment_id, update_id):
        """Without a launcher-selected supplier, no installation has a replacement owner.

        Apply then takes the ordinary update path, which checks the set-up
        owner's completed Restore itself, and whose worker leaves an
        application without a setup helper stopped for its owner's Start. The
        owner lookup still refuses a redirect to another build or source owner,
        and that owner must accept the staged candidate's protected choices and
        source. A transaction an earlier Forge staged with a native definition,
        instruction, runtime or maintained-host change is refused here, as Stage
        now blocks it. A product application that has not entered its setup has no
        update worker that could start it, so Apply refuses before any worker.
        After a Forge restart, an owner that only a claimed setup action can
        reconnect is named as such before any lookup.
        """
        if self._reconnect_status(self.deployment, guide_id, deployment_id) is not None:
            raise CustodyError(OWNER_RECONNECT_UPDATE)
        host = self.selected_host(guide_id, deployment_id)
        if type(host) is ForgeApplicationHost and "helper_identity" not in host._inventory:
            from .connect_installer_caller import HELPERLESS_UPDATE_UNAVAILABLE
            raise CustodyError(HELPERLESS_UPDATE_UNAVAILABLE)
        directory = self.deployment._directory(guide_id, deployment_id)
        if type(host) is OrdinaryApplicationHost:
            _, root, update = inspect_transaction(directory, update_id)
            # An earlier Forge staged without these blockers; refuse what Stage now blocks.
            if any(update.get(name) for name in ("native_definition_update", "host_instruction_update",
                    "runtime_dependency_update", "maintained_host_update")):
                raise CustodyError(ORDINARY_REPLACEMENT_BLOCKER)
            _, files = checked_package(root, "candidate.zip", update["package_sha256"])
            if (not owner_choices_match(host, files) or not self.continues_installed_source(
                    guide_id, deployment_id, update["to_build_id"], update["source_digest"])):
                raise CustodyError(ORDINARY_OWNER_BLOCKER)
        elif host is not None and not _set_up(directory):
            raise CustodyError(FIRST_SETUP_UPDATE)
        if self.selected_host(guide_id, deployment_id) is not host:
            raise CustodyError("The original installed replacement owner changed.")
        return None

    async def setup_status(self, deployment, guide_id, deployment_id):
        reconnect = self._reconnect_status(deployment, guide_id, deployment_id)
        if reconnect is not None:
            return reconnect
        host = self._require_selected(deployment, guide_id, deployment_id)
        if host is None:
            settings = await deployment.settings_options(guide_id, deployment_id)
            if self._require_selected(deployment, guide_id, deployment_id) is not None:
                raise CustodyError("The original application setup owner changed during status.")
            return {**application_setup_binding(settings, guide_id, deployment_id),
                "available": False, "reason": "host_unavailable",
                "detail": "Application setup has no trusted local host selection for this installation."}
        result = await host.setup_status(deployment, guide_id, deployment_id)
        if self._require_selected(deployment, guide_id, deployment_id) is not host:
            raise CustodyError("The original application setup owner changed during status.")
        return result

    def _reconnect_status(self, deployment, guide_id, deployment_id):
        """Read a reconnect hint, without constructing or authenticating a host.

        The claimed setup action must still authenticate the accepted Build,
        original private source and stopped installation before reconstruction.
        Neither source presence nor this hint asserts Fresh/Restore readiness.
        """
        self._require_registry()
        if deployment is not self.deployment:
            raise CustodyError("Use the registry's original deployment service.")
        if self.runtime_target is None or self.manual_host is not None:
            return None
        guide_id, deployment_id = _identity(guide_id), _identity(deployment_id)
        with self._lock:
            directory = deployment._directory(guide_id, deployment_id)
            if deployment._directory(guide_id) != directory:
                raise CustodyError("Preserve this installation's exact current canonical selection.")
            receipt = read_receipt(directory)
            if receipt.get("source_kind", "application") != "application":
                return None
            build_id = _identity(receipt.get("build_id"))
            if ((guide_id, deployment_id) in self._installations
                    or not _set_up(directory) and (guide_id, build_id) in self._builds):
                return None
            selection = receipt.get("runtime_candidate")
            if selection is None or type(parse_candidate_selection(selection)) is not MetaToolkitRuntimeTarget:
                return None
            snapshot, files = deployment._installed_configuration(guide_id, deployment_id, with_files=True)
            _, receipt, destination, _, _, _ = snapshot
            if (receipt.get("target", "local") != "local"
                    or receipt.get("execution_mode", "interactive") != "interactive"
                    or not receipt.get("installed_candidate_runtime_proof")):
                return None
            source = read_package_source(files, receipt)
            blueprint = _json(files["blueprint.json"][0])
            ordinary_target, ordinary_gateway = self._ordinary_runtime_selection(guide_id, blueprint,
                installation=receipt)
            if parse_candidate_selection(selection) != ordinary_target:
                return None
            report = _json(files["implementation-report.json"][0])
            if (not _same_toml(_candidate_source_selection(files), selection)
                    or not _same_toml(report.get("framework", {}).get("runtime_candidate"), selection)):
                raise CustodyError("The canonical package changed its original runtime selection.")
            if ordinary_gateway is not None and ordinary_application(blueprint):
                self._require_registry()
                return {**application_setup_binding(receipt, guide_id, deployment_id),
                    "available": False, "reason": "owner_reconnect", "action": "prepare-setup",
                    "detail": "Continue setup to check this ordinary application's original accepted source and startup inputs."}
            home = Path(destination) / ".mobkit" / "forge-local-setup"
            _private_directory(home)
            choices_bytes = _read(home / "local-application-policy.json", MAX_INPUT_BYTES)
            template_bytes = _read(home / "activation-template.json", MAX_INPUT_BYTES)
            choices, template = _json(choices_bytes), _json(template_bytes)
            if (choices.get("guide_id") != guide_id or choices.get("deployment_id") != deployment_id
                    or template != {"activation_id": deployment_id}):
                raise CustodyError("The saved setup belongs to another installation.")
            binding = _installation_binding(receipt, source)
            current = read_receipt(directory)
            if (deployment._directory(guide_id) != directory
                    or current != receipt
                    or _installation_binding(current, source) != binding
                    or _read(home / "local-application-policy.json", MAX_INPUT_BYTES) != choices_bytes
                    or _read(home / "activation-template.json", MAX_INPUT_BYTES) != template_bytes):
                raise CustodyError("The saved installation changed while reading its reconnect hint.")
            self._require_registry()
            name = _json(files["blueprint.json"][0]).get("project", {}).get("name")
            return {**application_setup_binding(receipt, guide_id, deployment_id),
                "available": False, "reason": "owner_reconnect", "action": "prepare-setup",
                **({"application_name": name} if isinstance(name, str) and 0 < len(name) <= 200 else {}),
                "detail": "Continue setup to check and reconnect this installation's original saved inputs."}
