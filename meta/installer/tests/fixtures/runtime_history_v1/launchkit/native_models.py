"""Model declarations for exact supported native release tuples."""
from __future__ import annotations

import json
from dataclasses import asdict, is_dataclass
from .bootstrap import CORE_VERSION, VERSION
from .native_definition_continuity import (LEGACY_MODEL_METADATA,
    _exact as _exact, is_legacy_registry_removal, legacy_registry_successor)
_CORE_FOR_RELEASE = object()


_SUPPLEMENTAL_MODELS = {("openai", "gpt-6.1-sol"): dict(LEGACY_MODEL_METADATA)}


def _selected_core(native_version, core_version):
    if core_version is _CORE_FOR_RELEASE:
        return CORE_VERSION if native_version == VERSION else None
    return core_version


def _supplements(native_version, core_version):
    # Core 0.8.51 owns this model, including the same capability metadata.
    # Re-declaring its ID as custom prevents native member construction.
    if (native_version, core_version) == ("0.8.46", "0.8.51"):
        return {}
    if (native_version, core_version) == ("0.8.42", None):
        return _SUPPLEMENTAL_MODELS
    raise ValueError("Model declarations need an exact supported MobKit/Core release tuple.")


def supplemental_model_metadata(*, native_version=VERSION, core_version=_CORE_FOR_RELEASE):
    """Expose declarations required by the selected exact release tuple."""
    return [{"id": model, "provider": provider,
             "source": "compiler_explicit_model_registry",
             "metadata": dict(metadata)}
            for (provider, model), metadata in sorted(_supplements(native_version, _selected_core(native_version, core_version)).items())]


def model_catalog_context(catalog=None, *, issue=None, native_version=VERSION, core_version=_CORE_FOR_RELEASE):
    """Project the pinned SDK's public catalog, never guessed provider support."""
    core_version = _selected_core(native_version, core_version)
    result = {
        "status": "unknown", "source": "native_runtime_curated_catalog",
        "method": "runtime.mob_handle().models_catalog()", "native_version": native_version,
        "core_version": core_version,
        "models": [], "provider_defaults": [],
        "supplemental_models": supplemental_model_metadata(native_version=native_version, core_version=core_version),
        "selection_guidance": "Choose a provider/model pair from the observed native catalog or the explicitly compiler-supported supplements. Unlisted pairs are unknown, not confirmed unsupported. Supplements are exact local registry declarations, not native catalog entries. Do not invent model support when the catalog cannot be read.",
        "availability_scope": "This is the pinned runtime's curated model metadata, not an exhaustive effective custom registry. It does not verify destination provider credentials, remote model access or successful agent startup.",
    }
    if catalog is None:
        result["issue"] = issue or "native_catalog_unavailable"
        return result
    if not is_dataclass(catalog):
        raise ValueError("The native model catalog must use the pinned SDK result contract.")
    value = asdict(catalog)
    if not isinstance(value.get("models"), list) or not isinstance(value.get("provider_defaults"), list):
        raise ValueError("The native model catalog has an invalid shape.")
    entries = list(value["models"])
    for defaults in value["provider_defaults"]:
        if (not isinstance(defaults, dict) or not isinstance(defaults.get("provider"), str)
                or not defaults["provider"] or not isinstance(defaults.get("default_model_id"), str)
                or not defaults["default_model_id"] or not isinstance(defaults.get("models"), list)):
            raise ValueError("The native provider defaults have an invalid shape.")
        entries.extend(defaults["models"])
    for entry in entries:
        if (not isinstance(entry, dict) or any(not isinstance(entry.get(key), str)
                or not entry[key] for key in ("id", "provider", "display_name", "tier"))):
            raise ValueError("The native model catalog entry is invalid.")
        for name in ("context_window", "max_output_tokens"):
            if entry.get(name) is not None and (type(entry[name]) is not int or entry[name] < 1):
                raise ValueError("The native model catalog token metadata is invalid.")
        if any(type(entry.get(name)) is not bool for name in ("vision", "image_tool_results")):
            raise ValueError("The native model catalog profile metadata is invalid.")
    # JSON-copy public SDK metadata so neither callers nor tools mutate its cache.
    public = json.loads(json.dumps(value, allow_nan=False))
    result.update(status="observed", models=public["models"], provider_defaults=public["provider_defaults"])
    return result


def model_registry_lines(selections, *, native_version=VERSION, core_version=_CORE_FOR_RELEASE):
    """Declare missing selected models once; never duplicate native builtins."""
    supplements = _supplements(native_version, _selected_core(native_version, core_version))
    lines, seen = [], set()
    for provider, model in selections:
        selection = (provider, model)
        if selection in seen or selection not in supplements:
            continue
        seen.add(selection)
        lines += ["", f"[models.{json.dumps(model)}]"]
        for key, value in supplements[selection].items():
            lines.append(f"{key} = {json.dumps(value)}")
    if lines:
        lines.append("")
    return lines
