"""Select an explicit trusted host in the existing server process before serving.

META_FORGE_APPLICATION_HOST_FACTORY names a synchronous zero-argument Python
factory as module:factory. It returns an original application host or an exact
qualified product runtime target, optionally with its application model
configuration owner, or original local and Linux control selections.
Targets enable source creation inside deliberate Build; the launcher itself
does not prepare, install or start applications.

Application model custody that a launcher configured for this process with
packaged_runtime.configure_application_model_custody is never dropped here. A
bare product target and an ordinary selection without its own owner take it.
A selection whose owner has the same selection keeps its own. Any other
selection refuses before the server is selected, an application host included:
its model owner comes from its preparation inputs, which this entry cannot
check.
"""
from __future__ import annotations

import importlib
import inspect
import keyword
import os
import sys
from dataclasses import dataclass, replace


FACTORY_ENV = "META_FORGE_APPLICATION_HOST_FACTORY"
APPLICATION_MODEL_OWNERS_DIFFER = ("The application model custody configured for this Forge differs from the "
    "model owner its application host factory selected. Select one application model owner.")
APPLICATION_MODEL_CUSTODY_UNCARRIED = ("The application model custody configured for this Forge cannot reach the "
    "selection its application host factory returned. Return a Meta Toolkit product target, a "
    "ToolkitApplicationSelection or an OrdinaryMetaApplicationSelection, or start this host without configured "
    "custody.")


def _factory_reference(value):
    if not isinstance(value, str) or value != value.strip() or value.count(":") != 1:
        raise ValueError(f"{FACTORY_ENV} must name a trusted Python module:factory.")
    module, attribute = value.split(":")
    names = (*module.split("."), attribute)
    if any(not name.isascii() or not name.isidentifier() or keyword.iskeyword(name) for name in names):
        raise ValueError(f"{FACTORY_ENV} must name a trusted Python module:factory.")
    return module, attribute


@dataclass(frozen=True)
class ApplicationRuntimeSelection:
    """Original trusted local target plus a separately selected Linux control."""

    local_target: object
    remote_control: object

    def __post_init__(self):
        from .runtime_selection import MetaToolkitRuntimeTarget
        from .remote_product_control import ProductControlSelection
        if (type(self.local_target) is not MetaToolkitRuntimeTarget
                or self.local_target.target not in {"macos-arm64", "linux-x86_64"}
                or type(self.remote_control) is not ProductControlSelection):
            raise TypeError("Select exact local and Linux product/control owners.")


@dataclass(frozen=True)
class OrdinaryMetaApplicationSelection:
    """The trusted launcher selects an ordinary gateway and its Meta SDK pair.

    Constructing this value is not native producer qualification. It is never
    parsed from an HTTP request, blueprint, package or runtime environment path.
    The selecting owner must already have accepted the exact native producer.
    """

    runtime_target: object
    gateway: object
    model_configuration: object = None

    def __post_init__(self):
        from .connect_installer_caller import ExactFile
        from .runtime_selection import MetaToolkitRuntimeTarget
        if type(self.runtime_target) is not MetaToolkitRuntimeTarget or type(self.gateway) is not ExactFile:
            raise TypeError("Select the exact ordinary Meta gateway and original accepted SDK product.")
        if self.model_configuration is not None:
            from .connect_application_startup import _ModelConfiguration
            if type(self.model_configuration) is not _ModelConfiguration:
                raise TypeError("Select the original application model configuration owner.")
            self.model_configuration.require_environment()
            self.model_configuration.require_filesystem_current()


@dataclass(frozen=True)
class ToolkitApplicationSelection:
    """The trusted launcher selects a Meta Toolkit product with its model owner.

    Local Build and Install owners retain this original application model
    configuration custody, as they do for an ordinary selection. Constructing
    this value does not read credentials or establish provider readiness. It
    is never parsed from an HTTP request, blueprint, package or environment.
    """

    runtime_target: object
    model_configuration: object

    def __post_init__(self):
        _require_toolkit_selection(self)


def _require_toolkit_selection(selected):
    from .connect_application_startup import _ModelConfiguration
    from .runtime_selection import MetaToolkitRuntimeTarget
    if (type(selected.runtime_target) is not MetaToolkitRuntimeTarget
            or type(selected.model_configuration) is not _ModelConfiguration):
        raise TypeError("Select the exact Meta Toolkit product and its original application model configuration owner.")
    selected.model_configuration.require_environment()
    selected.model_configuration.require_filesystem_current()


def _configured_application_model_custody():
    """The owner configure_application_model_custody selected in this process, or None.

    That function belongs to the packaged entry's module. A process that never
    imported the module configured no custody.
    """
    packaged = sys.modules.get("launchkit.packaged_runtime")
    return None if packaged is None else packaged.configured_application_model_custody()


def _with_configured_custody(selected, custody, target_type):
    """Carry configured application model custody into the factory's selection, or refuse."""
    from .connect_installer_caller import CustodyError
    if type(selected) is target_type:
        return ToolkitApplicationSelection(selected, custody)
    if type(selected) is OrdinaryMetaApplicationSelection and selected.model_configuration is None:
        return replace(selected, model_configuration=custody)
    if type(selected) in (ToolkitApplicationSelection, OrdinaryMetaApplicationSelection):
        owner = selected.model_configuration
        if not callable(getattr(owner, "selection", None)) or owner.selection() != custody.selection():
            raise CustodyError(APPLICATION_MODEL_OWNERS_DIFFER)
        return selected
    # An application host's model owner comes from its preparation inputs, which
    # this entry cannot check against custody; a local and Linux control
    # selection carries none. Both refuse.
    raise CustodyError(APPLICATION_MODEL_CUSTODY_UNCARRIED)


def create_app():
    """Uvicorn --factory entry point; configured failures stop startup."""
    module, attribute = _factory_reference(os.environ.get(FACTORY_ENV))
    factory = getattr(importlib.import_module(module), attribute)
    call = getattr(factory, "__call__", None)
    if not callable(factory) or any(
            inspect.iscoroutinefunction(value) or inspect.isasyncgenfunction(value)
            for value in (factory, call)):
        raise TypeError(f"{FACTORY_ENV} must select a synchronous zero-argument factory.")
    try:
        inspect.signature(factory).bind()
    except (TypeError, ValueError) as error:
        raise TypeError(f"{FACTORY_ENV} must select a synchronous zero-argument factory.") from error
    host_type = importlib.import_module("launchkit.forge_application_host").ForgeApplicationHost
    target_type = importlib.import_module("launchkit.runtime_selection").MetaToolkitRuntimeTarget
    selected = factory()
    if type(selected) not in (host_type, target_type, ApplicationRuntimeSelection,
            OrdinaryMetaApplicationSelection, ToolkitApplicationSelection):
        if inspect.iscoroutine(selected):
            selected.close()
        raise TypeError("The trusted factory must return an exact ForgeApplicationHost, MetaToolkitRuntimeTarget, ApplicationRuntimeSelection, OrdinaryMetaApplicationSelection or ToolkitApplicationSelection.")
    custody = _configured_application_model_custody()
    if custody is not None:
        selected = _with_configured_custody(selected, custody, target_type)
    server = importlib.import_module("launchkit.server")
    if type(selected) is ToolkitApplicationSelection:
        _require_toolkit_selection(selected)
        server.select_local_application_runtime(selected.runtime_target,
            model_configuration=selected.model_configuration)
        _require_toolkit_selection(selected)
    elif type(selected) is OrdinaryMetaApplicationSelection:
        options = {"ordinary_meta_gateway": selected.gateway}
        if selected.model_configuration is not None:
            selected.model_configuration.require_environment()
            selected.model_configuration.require_filesystem_current()
            options["model_configuration"] = selected.model_configuration
        server.select_local_application_runtime(selected.runtime_target, **options)
        if selected.model_configuration is not None:
            selected.model_configuration.require_environment()
            selected.model_configuration.require_filesystem_current()
    elif type(selected) is ApplicationRuntimeSelection:
        server.select_product_application_runtime(selected)
    elif type(selected) is target_type:
        server.select_local_application_runtime(selected)
    else:
        server.select_application_host(selected)
    return server.app
