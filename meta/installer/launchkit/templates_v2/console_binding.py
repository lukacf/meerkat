"""Compose the acquired setup component with its existing Console owners.

The application supplies a qualified executable and its current-lifetime
predicate. This module does not acquire or select a component, start a child,
read credentials, declare frontend capabilities or grant application effects.
"""

import copy
from pathlib import Path
import re
import uuid

if __package__:
    from .console_connect import ConsoleConnect, ConnectError
    from .console_presentation import load_console_presentation
else:
    from console_connect import ConsoleConnect, ConnectError
    from console_presentation import load_console_presentation


def console_connect_options(installed_root, application_id, binary, *,
                            is_current, provider_key=None, model=None):
    """Return optional ConsoleFront arguments for one actual app lifetime.

    Call once after the installation and application owners have selected this
    component. Each actual ConsoleConnectLogin gets its own dormant bridge.
    The realm's paths survive app restart; its instance and login do not.
    Without an explicit model override, native realm configuration selects it.
    """
    return _compose_console_options(installed_root, application_id, binary,
        is_current=is_current, provider_key=provider_key, model=model)


def console_service_options(installed_root, application_id, binary, *,
                            resource_manifest_sha256, is_current,
                            provider_key=None, model=None, auth_binding=None):
    """Prepare the separate proposed protocol-3 host composition.

    Only the installation/application owner may select a qualified matching
    executable, its compiled resource digest and permission to prepare those
    resources. This function does not acquire or qualify that selection. No
    generated application selects this composition by default. There is no
    fallback to the frontend adapter if negotiation fails.

    Preparation permission neither grants service installation/account access
    nor supplies original-work authority. Those remain native owner decisions.
    """
    if (not isinstance(resource_manifest_sha256, str)
            or re.fullmatch(r"[0-9a-f]{64}", resource_manifest_sha256, re.ASCII) is None):
        raise ValueError("Service setup requires the selected compiled resource digest.")
    if auth_binding is not None:
        if (not isinstance(auth_binding, dict)
                or not {"realm", "binding"} <= auth_binding.keys()
                or set(auth_binding) - {"realm", "binding", "profile"}
                or any(not isinstance(value, str) or not value for value in auth_binding.values())):
            raise ValueError("Service setup requires the selected native authentication binding.")
    selection = {"protocol_version": 3, "adapter": "forge_services",
        "services": {"resource_manifest_sha256": resource_manifest_sha256,
                     "preparation": "permitted"}}
    if auth_binding is not None:
        selection["auth_binding"] = copy.deepcopy(auth_binding)
    return _compose_console_options(installed_root, application_id, binary,
        is_current=is_current, provider_key=provider_key, model=model,
        selection=selection)


def _compose_console_options(installed_root, application_id, binary, *,
                             is_current, provider_key, model, selection=None):
    root, executable = Path(installed_root), Path(binary)
    if (not root.is_absolute() or root.resolve(strict=True) != root
            or not executable.is_absolute() or executable.parent.name != "bin"
            or not callable(is_current)):
        raise ValueError("Setup requires the selected installed application and component.")
    if (not isinstance(application_id, str) or not application_id
            or model is not None and (not isinstance(model, str) or not model)
            or provider_key is not None and (not isinstance(provider_key, str) or "\0" in provider_key)):
        raise ValueError("Setup requires trusted application and provider configuration.")
    public_assets, document_transform = load_console_presentation(root, setup=True)
    instance_id = uuid.uuid4().hex
    # Deliberately exclude ambient credentials, proxies, loaders and home paths.
    # Missing credentials remain a native authentication result.
    environment = {"HOME": str(root / ".mobkit/connect/user"), "PATH": ""}
    if provider_key:
        environment["OPENAI_API_KEY"] = provider_key

    def current(login):
        try:
            return login.is_current() is True and is_current() is True
        except Exception:
            return False

    def connect_factory(login):
        if not current(login):
            raise ConnectError("invalid_binding")
        bootstrap = {
            "protocol_version": 1,
            "application": {"application_id": application_id,
                "instance_id": instance_id, "origin": login.origin},
            "viewer": {"principal_id": login.principal,
                "login_scope_id": login.login_scope_id,
                "expires_at_ms": login.expires_at_ms},
            "realm": {"realm_id": "forge-" + application_id,
                "state_root": str(root / ".mobkit/connect/state"),
                "context_root": str(root),
                "user_config_root": str(root / ".mobkit/connect/user")},
        }
        if model is not None:
            bootstrap["model"] = model
        if selection is not None:
            bootstrap.update(copy.deepcopy(selection))
        return ConsoleConnect(executable, bootstrap,
            is_current=lambda: current(login), environment=environment)

    return {"connect_factory": connect_factory,
            "console_assets": public_assets,
            "document_transform": document_transform}


def application_console_options(application, binary, *, provider_key=None):
    """Select presentation only for the current loaded frontend owner/runtime.

    The application host is the existing authority for declared support and
    unchanged loaded source. Selecting a component does not create that support.
    """
    context = application.context
    runtime = context.runtime
    if runtime is None or context.stop_event.is_set():
        raise ValueError("Setup requires the current application runtime.")
    host = application.frontend_host()
    if host["application_id"] != context.deployment["slug"]:
        raise ValueError("Setup requires the current application source.")

    def is_current():
        return (context.runtime is runtime and not context.stop_event.is_set()
                and application.frontend_host() == host)

    selected = console_connect_options(context.root, host["application_id"], binary,
        is_current=is_current, provider_key=provider_key)
    if callable(getattr(getattr(application, "integration", None), "controller_review_factory", None)):
        selected["console_assets"], selected["document_transform"] = load_console_presentation(
            context.root, setup=True, controller_review=True)
    return selected
