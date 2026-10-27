"""Use the launcher-selected Meta SDK runtime for Forge's own agent work.

Application deployment targets retain their separate product preparation. These
helpers configure only the guide, builder and visual critic and never borrow an
installed application's activation or open a predecessor's native actor store.
"""
from __future__ import annotations

import json


def selected_runtime():
    from .bootstrap import selected_guide_runtime
    selected = selected_guide_runtime()
    if selected is None:
        raise RuntimeError("Forge needs its selected Meta runtime before starting its agents.")
    selected.recheck()
    return selected


def selected_model_configuration():
    """Return a copied model route after its original host custody is checked."""
    selected = selected_runtime()
    return selected.model_configuration


def model_profile_lines(provider, model, configuration=None):
    """Author the selected model, preserving its explicit native auth binding."""
    lines = [f'model = {json.dumps(model)}', f'provider = {json.dumps(provider)}']
    if configuration is not None:
        if configuration["provider"] != provider or configuration["model"] != model:
            raise RuntimeError("The Forge agent model changed its selected host route.")
        if configuration.get("auth_binding") is not None:
            lines.append(f'auth_binding = {json.dumps(configuration["auth_binding"])}')
    return lines


def configure_builder(builder, selected):
    """Bind a fresh native mob to this exact RPC gateway and model host config."""
    selected.recheck(verify_gateway=True)
    builder.gateway(str(selected.gateway.path))
    configuration = selected.model_configuration
    if configuration is not None and configuration.get("meerkat_config_path") is not None:
        builder.meerkat_config(configuration["meerkat_config_path"])
    return builder


def capture_worker_runtime(directory):
    """Seal host-only runtime custody beside an owned worker's private log."""
    from pathlib import Path
    import hashlib
    from .bootstrap import export_selected_guide_context
    from .runtime_dependencies import _create_json, _file, _unique
    selected_runtime().recheck()
    value = export_selected_guide_context()
    path = Path(directory) / "forge-agent-runtime.json"
    if path.exists() or path.is_symlink():
        content = _file(path, 256_000, private=True)
        if json.loads(content, object_pairs_hook=_unique) != value:
            raise RuntimeError("The saved build changed its original Forge agent runtime.")
    else:
        _create_json(path, value)
        content = _file(path, 256_000, private=True)
    selected_runtime().recheck()
    return hashlib.sha256(content).hexdigest()


def restore_worker_runtime(directory, digest):
    """Restore the original parent's exact private selection before native init."""
    from pathlib import Path
    import hashlib
    import re
    from .bootstrap import restore_selected_guide_context
    from .runtime_dependencies import _file, _unique
    if type(digest) is not str or re.fullmatch("[0-9a-f]{64}", digest) is None:
        raise RuntimeError("The builder needs its original parent's runtime control.")
    content = _file(Path(directory) / "forge-agent-runtime.json", 256_000, private=True)
    if hashlib.sha256(content).hexdigest() != digest:
        raise RuntimeError("The builder's private runtime control changed before startup.")
    return restore_selected_guide_context(json.loads(content, object_pairs_hook=_unique))


async def build_with_cleanup(builder, retained_cleanup):
    """Keep the SDK's failed-start cleanup capability until its owner settles it."""
    from meerkat_meta.errors import GatewayStartupCleanupError
    try:
        return await builder.build()
    except BaseException as error:
        pending, seen = [error], set()
        while pending and len(seen) < 64:
            current = pending.pop()
            if id(current) in seen:
                continue
            seen.add(id(current))
            cleanup = (current.cleanup if isinstance(current, GatewayStartupCleanupError)
                       else getattr(current, "gateway_cleanup", None))
            if cleanup is not None and not any(cleanup is original for original in retained_cleanup):
                retained_cleanup.append(cleanup)
            if isinstance(current, BaseExceptionGroup):
                pending.extend(current.exceptions)
            pending.extend(link for link in (current.__cause__, current.__context__)
                           if isinstance(link, BaseException))
        raise
