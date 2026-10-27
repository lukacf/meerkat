"""Concrete trusted local single-user Forge host composition.

The original launcher configures this callable with its actual GuideStore,
qualified product target and protected application choices. Calling it takes no
arguments and selects no HTTP, environment or package-supplied runtime. Local
preparation defaults are new application declarations, never native evidence.
Nothing prepares, installs or starts an application on import or factory entry.
"""
from __future__ import annotations

from copy import deepcopy
import inspect
import os
from pathlib import Path
import stat
from urllib.parse import urlsplit

from .blueprint import Blueprint
from .connect_installer_caller import (CustodyError, ExactFile, _json, _path,
    _private_directory, _private_parent, _same_json_value)
from .forge_application_host import ForgeApplicationHost
from .guide_store import GuideStore
from .runtime_selection import MetaToolkitRuntimeTarget


def _replacement_supplier(selected):
    """Validate a trusted read-only envelope supplier without entering it."""
    if selected is None:
        return
    call = getattr(selected, "__call__", None)
    if (not callable(selected) or any(inspect.iscoroutinefunction(value)
            or inspect.isasyncgenfunction(value) for value in (selected, call))):
        raise CustodyError("Replacement inputs need their original synchronous selection owner.")
    try:
        inspect.signature(selected).bind("guide", "installation", "update")
    except (TypeError, ValueError) as error:
        raise CustodyError("Replacement selection needs its exact guide, installation and update identities.") from error


class LocalApplicationFactory:
    """A configured zero-argument factory for application_host_launch.

    The trusted Python launcher closes these original selections. Successful
    Install selects the actual local Console port unless an exact compatible
    source choice was supplied. No browser or second issuer selects it. Source
    changes require a new deliberate owner selection; a guide version alone may advance when
    the same complete proposal is accepted.

    An optional launcher-selected replacement supplier only reads an envelope
    already retained by that owner. This factory never prepares one implicitly.
    """

    def __init__(self, *, guide_store, guide_id, runtime_target,
            acquisition_selection, viewer_selection, console_setup_selection=None,
            model_configuration=None, replacement_selection=None, _installed_admission=None):
        _replacement_supplier(replacement_selection)
        self.replacement_selection = self._original_replacement_selection = replacement_selection
        if (type(guide_store) is not GuideStore
                or type(runtime_target) is not MetaToolkitRuntimeTarget
                or any(type(pin) is not ExactFile for pin in (
                    acquisition_selection, viewer_selection))
                or console_setup_selection is not None and type(console_setup_selection) is not ExactFile):
            raise CustodyError("The local factory needs its actual store, product and protected source choices.")
        if _installed_admission is not None:
            from .local_application_recovery import _InstalledLocalAdmission
            if type(_installed_admission) is not _InstalledLocalAdmission:
                raise CustodyError("Recovery needs its actual admitted original installed source owner.")
            _installed_admission.require_current()
            if (_installed_admission.registry.guide_store is not guide_store
                    or _installed_admission.guide_id != guide_id
                    or _installed_admission.runtime_target is not runtime_target
                    or _installed_admission.acquisition_selection != acquisition_selection
                    or _installed_admission.viewer_selection != viewer_selection
                    or console_setup_selection != _installed_admission.console_setup_selection):
                raise CustodyError("Recovery needs its actual admitted original installed source owner.")
        self.model_configuration = deepcopy(model_configuration)
        self._model_configuration_custody = None
        if model_configuration is not None:
            from .connect_application_startup import _ModelConfiguration
            self._model_configuration_custody = _ModelConfiguration.observe(self.model_configuration)
        self._original_model_configuration_custody = self._model_configuration_custody
        self._installed_admission = _installed_admission
        guide_store.identity(guide_id)
        self.guide_store, self.guide_id = guide_store, guide_id
        self._store_root = guide_store.root
        self._store_identity = self._observe_store()
        self.runtime_target = runtime_target
        self.acquisition_selection, self.viewer_selection = acquisition_selection, viewer_selection
        self.console_setup_selection = console_setup_selection
        self._configured_selection = (guide_store, guide_id, runtime_target,
            acquisition_selection, viewer_selection, console_setup_selection, _installed_admission)
        self._source_pins = (acquisition_selection, viewer_selection,
            *((console_setup_selection,) if console_setup_selection is not None else ()))
        self._source_bytes = tuple(self._read(pin) for pin in self._source_pins)
        if console_setup_selection is not None:
            self._console_choice(_json(self._source_bytes[2]))
        acquisition = _json(self._source_bytes[0])
        try:
            entry = acquisition["blueprint"]
            self._blueprint_pin = ExactFile(Path(entry["path"]), entry["sha256"])
            self._blueprint = Blueprint.model_validate(_json(self._read(self._blueprint_pin))).model_dump(mode="json")
        except (KeyError, TypeError, ValueError) as error:
            raise CustodyError("The local factory lacks its original complete application plan.") from error
        self.host = self.preparation_owner = None
        self.require_current()

    @staticmethod
    def _read(pin):
        _private_parent(pin.path)
        return pin.read()

    def _observe_store(self):
        """Fence the original store before GuideStore's creating read helper.

        SQLite content may advance in place. Storage replacement cannot select
        another guide owner or let an ordinary read recreate a missing store.
        """
        if self.guide_store.root != self._store_root:
            raise CustodyError("Preserve the local factory's original guide store root.")
        try:
            root = _private_directory(self._store_root)
            path = _path(self._store_root / "guided-setups.sqlite3")
            descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
            try:
                info, current = os.fstat(descriptor), path.lstat()
                if (not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid()
                        or info.st_nlink != 1 or info.st_mode & 0o077
                        or (info.st_dev, info.st_ino) != (current.st_dev, current.st_ino)):
                    raise CustodyError("The original local guide database is not private, owned and ordinary.")
                return root, (info.st_dev, info.st_ino)
            finally:
                os.close(descriptor)
        except OSError as error:
            raise CustodyError("The original local guide storage disappeared or changed.") from error

    def _require_store(self):
        if self._observe_store() != self._store_identity:
            raise CustodyError("The local factory's original guide storage was replaced.")

    @staticmethod
    def _console_choice(value):
        """Validate this local application's exact HTTP loopback source choice.

        The actual maintained Console advertises this origin. Native still
        validates its own selected artifact and presenter independently.
        """
        # Recovery is a separately selected operator capability. Preserve the
        # old source shape and never derive this choice from access management.
        if (not isinstance(value, dict) or set(value) not in (
                    {"origin", "access_management"},
                    {"origin", "access_management", "recovery_management"})
                or "recovery_management" in value and value["recovery_management"] != "operator_unpublished_service_approvals"
                or value.get("access_management") != "requester_issued_leaves"
                or not isinstance(value.get("origin"), str)):
            raise CustodyError("Local setup needs its protected requester-issued Console surface choice.")
        try:
            selected = urlsplit(value["origin"])
            port = selected.port
            if (type(port) is not int or not 1024 <= port <= 65535
                    or value["origin"] != "http://127.0.0.1:" + str(port)):
                raise ValueError("The selected origin differs from the local Console endpoint.")
        except ValueError as error:
            raise CustodyError("Local setup needs its exact HTTP loopback Console origin and port.") from error
        return value

    def _guide(self):
        self._require_store()
        state = self.guide_store.get(self.guide_id)
        self._require_store()
        operation = self.guide_store.operation(self.guide_id)
        self._require_store()
        if (not isinstance(state, dict) or state.get("id") != self.guide_id
                or type(state.get("version")) is not int or state["version"] < 1
                or operation is not None and not isinstance(operation, dict)
                or operation is not None and operation.get("status") == "running"):
            raise CustodyError("The local application's original guide is unavailable or still changing.")
        selected = state.get("proposal") if state.get("proposal") is not None else state.get("blueprint")
        try:
            plan = Blueprint.model_validate(selected).model_dump(mode="json")
        except (TypeError, ValueError) as error:
            raise CustodyError("The local application's selected complete plan changed.") from error
        return state["version"], plan

    def _selected_model_configuration(self):
        """Read the original trusted selection, never discover credentials."""
        selected = self._model_configuration_custody
        if selected is not self._original_model_configuration_custody:
            raise CustodyError("Preserve the original selected model configuration owner.")
        value = None if selected is None else selected.selection()
        if not _same_json_value(self.model_configuration, value):
            raise CustodyError("The original trusted model configuration selection changed.")
        if selected is not None:
            selected.require_filesystem_current()
        return value

    def require_current(self):
        self._selected_model_configuration()
        if (self.replacement_selection is not self._original_replacement_selection
                or self.host is not None and
                    self.host._replacement_selection is not self._original_replacement_selection):
            raise CustodyError("Preserve the local factory's original replacement selection owner.")
        if (self.guide_store, self.guide_id, self.runtime_target, self.acquisition_selection,
                self.viewer_selection, self.console_setup_selection, self._installed_admission) != self._configured_selection:
            raise CustodyError("Preserve the local factory's original guide, runtime and protected selections.")
        installed = self._installed_context()
        before = self._guide() if installed is None else self._installed_guide_owner()
        if (tuple(self._read(pin) for pin in self._source_pins) != self._source_bytes
                or Blueprint.model_validate(_json(self._read(self._blueprint_pin))).model_dump(mode="json") != self._blueprint):
            raise CustodyError("The local application's protected source choices changed.")
        current = self._installed_context()
        if current != installed:
            raise CustodyError("The original local installation changed during source validation.")
        after = self._guide() if installed is None else self._installed_guide_owner()
        if installed is None and (before[0] != after[0]
                or before[1] != self._blueprint or after[1] != self._blueprint):
            raise CustodyError("The current local guide differs from the original selected application.")

    def _installed_guide_owner(self):
        """The draft may evolve while its accepted installation stays usable."""
        self._require_store()
        state = self.guide_store.get(self.guide_id)
        self._require_store()
        if (not isinstance(state, dict) or state.get("id") != self.guide_id
                or type(state.get("version")) is not int or state["version"] < 1):
            raise CustodyError("The installed application's original guide owner is unavailable.")

    def _installed_context(self):
        """Read only the actual completed Install owner's retained association.

        File presence or a status flag cannot detach source from its proposal.
        The original completion hook must have retained these exact objects.
        Existing setup and runtime admissions still check the full installation.
        """
        admission = self._installed_admission
        if admission is not None:
            from .local_application_recovery import _InstalledLocalAdmission
            if type(admission) is not _InstalledLocalAdmission:
                raise CustodyError("The recovered installed source admission changed.")
            admission.require_current()
        host, owner = self.host, self.preparation_owner
        if host is None or host._retained_preparation_inputs is None:
            return admission.installation if admission is not None else None
        from .deployment_worker import Supervisor, read_receipt
        from .runtime_selection import parse_candidate_selection
        inputs = host._retained_preparation_inputs
        if (owner is None or inputs is not owner._inputs or owner.host is not host
                or inputs.supervisor is not host._installation_supervisor
                or type(inputs.supervisor) is not Supervisor
                or owner._guide_id != self.guide_id
                or inputs.supervisor.directory != owner._directory):
            raise CustodyError("Preserve the installed application's original retained owner.")
        receipt = read_receipt(inputs.supervisor.directory)
        if (receipt.get("guide_id") != self.guide_id
                or receipt.get("deployment_id") != owner._deployment_id
                or not receipt.get("installed_candidate_runtime_proof")
                or parse_candidate_selection(receipt.get("runtime_candidate")) != self.runtime_target
                or inputs.supervisor.destination != Path(receipt["destination"])):
            raise CustodyError("The original installed application source association changed.")
        # The retained installation is this owner's anchor. Its own completed
        # source updates may move it; nothing else may.
        from .deployment_application_bootstrap import require_owner_installation
        binding = require_owner_installation(inputs.supervisor.directory, receipt, owner._installation,
            "The original installed application source association changed.")
        if admission is not None:
            require_owner_installation(inputs.supervisor.directory, receipt, admission.installation,
                "The recovered private inputs changed their original admitted installation.")
        return binding

    def __call__(self):
        self.require_current()
        if self.host is None:
            from .local_application_preparation_inputs import LocalApplicationPreparationOwner
            owner = LocalApplicationPreparationOwner(self)
            host = ForgeApplicationHost(guide_id=self.guide_id, runtime_target=self.runtime_target,
                acquisition_selection=self.acquisition_selection, viewer_selection=self.viewer_selection,
                require_current=self.require_current, startup_selection=owner.startup_selection,
                preparation_inputs=owner.preparation_inputs, retain_preparation_inputs=owner.retain,
                startup_continuation=owner.retain_restore,
                replacement_selection=self.replacement_selection)
            owner.bind_host(host)
            self.preparation_owner, self.host = owner, host
        if (self.host._target != self.runtime_target or self.host.guide_id != self.guide_id
                or self.host._startup_selection != self.preparation_owner.startup_selection
                or self.host._preparation_inputs != self.preparation_owner.preparation_inputs
                or self.host._startup_continuation != self.preparation_owner.retain_restore
                or self.host._replacement_selection is not self.replacement_selection
                or self.host._retain_preparation_inputs != self.preparation_owner.retain):
            raise CustodyError("Preserve the local factory's original host and preparation owner.")
        self.host.require_current()
        return self.host
