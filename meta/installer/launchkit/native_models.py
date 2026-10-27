"""Model declarations for exact supported native release tuples."""
from __future__ import annotations

import json
from dataclasses import asdict, is_dataclass
from .bootstrap import CORE_VERSION, VERSION
from .sdk_wheel_integrity import SUPPORTED_META_VERSIONS
from .native_definition_continuity import (LEGACY_MODEL_METADATA,
    _exact as _exact, is_legacy_registry_removal, legacy_registry_successor)
_CORE_FOR_RELEASE = object()


_SUPPLEMENTAL_MODELS = {("openai", "gpt-6.1-sol"): dict(LEGACY_MODEL_METADATA)}


def _selected_core(native_version, core_version):
    if core_version is _CORE_FOR_RELEASE:
        return "0.8.51" if native_version in {"0.8.46", "0.8.47", "0.8.48"} else None
    return core_version


def _supplements(native_version, core_version):
    # Core 0.8.51 owns this model, including the same capability metadata.
    # Re-declaring its ID as custom prevents native member construction.
    if (native_version, core_version) in {("0.8.46", "0.8.51"), ("0.8.47", "0.8.51"), ("0.8.48", "0.8.51")}:
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


def _checked_guide_runtime(selected):
    from .bootstrap import selected_guide_runtime
    if selected is not selected_guide_runtime():
        raise ValueError("The guide catalog needs its current launcher-selected Meta runtime.")
    selected.recheck()
    return selected


def _project_meta_catalog(value, *, raw):
    """Project 0.9 capabilities from full entries, never compact summaries."""
    public = json.loads(json.dumps(value, allow_nan=False))
    full = {}
    for entry in public["models"]:
        if not isinstance(entry, dict):
            raise ValueError("The native model catalog entry is invalid.")
        key = entry.get("provider"), entry.get("id")
        if any(type(part) is not str or not part for part in key) or key in full:
            raise ValueError("The native model catalog identity is invalid.")
        if raw:
            profile = entry.get("profile")
            if (not isinstance(profile, dict) or any(type(profile.get(name)) is not bool
                    for name in ("image_input", "image_tool_results"))):
                raise ValueError("The native model catalog profile metadata is invalid.")
            # `vision` is Forge's existing image-input capability projection.
            entry["vision"] = profile["image_input"]
            entry["image_tool_results"] = profile["image_tool_results"]
        elif any(type(entry.get(name)) is not bool for name in ("vision", "image_tool_results")):
            raise ValueError("The native SDK catalog profile metadata is invalid.")
        full[key] = entry
    for group in public["provider_defaults"]:
        if (not isinstance(group, dict) or type(group.get("provider")) is not str
                or not isinstance(group.get("models"), list)
                or (group["provider"], group.get("default_model_id")) not in full):
            raise ValueError("The native provider defaults have an invalid identity.")
        for summary in group["models"]:
            if not isinstance(summary, dict):
                raise ValueError("The native provider model summary is invalid.")
            key = summary.get("provider"), summary.get("id")
            if key not in full or key[0] != group["provider"]:
                raise ValueError("The native provider model lacks its canonical full entry.")
            entry = full[key]
            for field in ("id", "provider", "display_name", "tier", "context_window", "max_output_tokens"):
                if summary.get(field) != entry.get(field):
                    raise ValueError("The native provider summary differs from its full model entry.")
            summary["vision"] = entry["vision"]
            summary["image_tool_results"] = entry["image_tool_results"]
    return public


def model_catalog_context(catalog=None, *, issue=None, native_version=VERSION, core_version=_CORE_FOR_RELEASE,
                          runtime_target=None, verified_artifacts=None, guide_runtime=None):
    """Project the pinned SDK's public catalog, never guessed provider support."""
    product = None
    if runtime_target is not None:
        from .runtime_selection import MetaToolkitRuntimeTarget, selected_runtime_artifacts
        if type(runtime_target) is not MetaToolkitRuntimeTarget:
            raise ValueError("An explicit native product catalog requires its selected product.")
        product = selected_runtime_artifacts(runtime_target, verified_artifacts=verified_artifacts)
        native_version, core_version = runtime_target.meta.version, None
    elif guide_runtime is not None:
        guide_runtime = _checked_guide_runtime(guide_runtime)
        native_version, core_version = guide_runtime.version, guide_runtime.core_version
    else:
        core_version = _selected_core(native_version, core_version)
    result = {
        "status": "unknown", "source": "native_runtime_curated_catalog",
        "method": "runtime.mob_handle().models_catalog()", "native_version": native_version,
        "core_version": core_version,
        "models": [], "provider_defaults": [],
        "supplemental_models": ([] if product is not None or guide_runtime is not None else
            supplemental_model_metadata(native_version=native_version, core_version=core_version)),
        "selection_guidance": "Choose a provider/model pair from the observed native catalog or the explicitly compiler-supported supplements. Unlisted pairs are unknown, not confirmed unsupported. Supplements are exact local registry declarations, not native catalog entries. Do not invent model support when the catalog cannot be read.",
        "availability_scope": "This is the pinned runtime's curated model metadata, not an exhaustive effective custom registry. It does not verify destination provider credentials, remote model access or successful agent startup.",
    }
    if product is not None and catalog is None:
        value = product.catalog
        result.update(source="selected_native_product_observed_catalog",
            source_manifest_sha256=product.candidate["source_manifest_sha256"],
            resources_sha256=product.candidate["resources_sha256"],
            catalog_sha256=runtime_target.model_catalog.sha256)
    elif catalog is None:
        result["issue"] = issue or "native_catalog_unavailable"
        return result
    else:
        if not is_dataclass(catalog):
            raise ValueError("The native model catalog must use the pinned SDK result contract.")
        value = asdict(catalog)
    if not isinstance(value.get("models"), list) or not isinstance(value.get("provider_defaults"), list):
        raise ValueError("The native model catalog has an invalid shape.")
    if native_version in SUPPORTED_META_VERSIONS:
        value = _project_meta_catalog(value, raw=product is not None and catalog is None)
        result["capability_projection"] = (
            "Full Meta model image-input and image-tool-result capabilities; "
            "provider summaries join canonical full entries by provider and model ID.")
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


def model_registry_lines(selections, *, native_version=VERSION, core_version=_CORE_FOR_RELEASE,
                         runtime_target=None, verified_artifacts=None, guide_runtime=None,
                         public_runtime=False):
    """Declare missing selected models once; never duplicate native builtins."""
    if public_runtime:
        # The public Meta release's native registry owns these models. Use the
        # same allowlist rule as a selected product, against its recorded catalog.
        from .public_meta_runtime import models_unlisted_reason, unlisted_models
        missing = unlisted_models(selections)
        if missing:
            raise ValueError(models_unlisted_reason(missing))
        return []
    if runtime_target is not None:
        context = model_catalog_context(runtime_target=runtime_target, verified_artifacts=verified_artifacts)
        known = {(entry["provider"], entry["id"]) for entry in context["models"]}
        known.update((entry["provider"], entry["id"]) for defaults in context["provider_defaults"]
            for entry in defaults["models"])
        if any(selection not in known for selection in selections):
            raise ValueError("The selected application model is absent from this exact native product catalog.")
        return []
    if guide_runtime is not None:
        _checked_guide_runtime(guide_runtime)
        # Meta's native model registry owns this selection. Init resolves it
        # against the actual gateway/config; no custom declarations are made
        # from a version string, provider preference or guessed capability.
        return []
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
