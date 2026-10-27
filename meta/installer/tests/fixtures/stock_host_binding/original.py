"""Original stock-guide source binding for the later trusted launcher factory.

This module selects no runtime, native identity, private path or authentication.
The existing installation owner must supply its actual admitted product target
and normal GuideStore. Preparation precedes its retained private Fresh/Restore
supplier. Nothing executes or instantiates a host on import. A later trusted
factory can close those actual arguments for the existing launcher.
"""
from __future__ import annotations

from pathlib import Path

from launchkit.blueprint import Blueprint
from launchkit.connect_installer_caller import CustodyError, ExactFile, _json, _private_parent
from launchkit.guide_store import GuideStore
from launchkit.runtime_selection import MetaToolkitRuntimeTarget


STOCK_GUIDE_ID = "b1301764-2559-41c4-99de-d2a0f2bae3af"
_ARTIFACTS = Path("/Users/luka/.codex/worktrees/58af/meerkat-mobkit/installer/.artifacts")
_STOCK = _ARTIFACTS / "release09-stock-console-20261010"
_ACQUISITION = _STOCK / "acquisition-selection-v2"
_VIEWER = _STOCK / "viewer-policy-selection-v2"
_BLUEPRINT_PATH = _ACQUISITION / "blueprint.json"

# Original selected bytes, not a new input manifest or native admission record.
_SOURCE_PINS = {
    "guide_origin": (_ARTIFACTS / "forge-backend-refresh-20261011/STOCK-GUIDE-REQUEST.json",
        "a8a0892bb4273714bd781a98455b86deade2871c9b5e0ea3bdca05762a54f45f", 5782),
    "acquisition": (_ACQUISITION / "source-selection.json",
        "98cc8f549793a4c600ff6abc1d0ca4d55dc8ae38ea1fdfc85f4961287dcaa904", 3317),
    "viewer": (_VIEWER / "APPLICATION-SELECTION.json",
        "806eee07021faafe2b2938df8182a1e7599420a3311aba41fafdc252f1e556d4", 4292),
    "blueprint": (_BLUEPRINT_PATH,
        "8f84cc9e389d83ea55380323adb76917e076da65f0eacfd091374f1b44f85d06", 3397),
    "definition": (_ACQUISITION / "mob.toml",
        "176feaa1fe35dda14e628621ddf98d48fc9ff6432920cb9339737c6e1025076c", 5033),
    "original_inventory": (_STOCK / "application-inventory-input.json",
        "f3b64693618b7c5443ed54c974f5218c376f67209634fe7e077c24a77b809a39", 754),
    "source_policy": (_ACQUISITION / "tool-policy.json",
        "fa70d94e5c81062e8f3ec585f9e505e417a851f19b683747e7ccd0718f557623", 677),
    "application/console.json": (_VIEWER / "source/application/console.json",
        "966af98fd32f743c5ba631035dcf240f559a0c2936695db336a9501cb2ab4e16", 41),
    "application/configuration.json": (_VIEWER / "source/application/configuration.json",
        "4578e5d457b66ab134c0c4ccb9c740203edebd5860212690b1e552d1f5f355ff", 614),
    "config/access.toml": (_VIEWER / "source/config/access.toml",
        "5c9d54a1a71aaf322011a51e26fb8bc17f8cad06e416334663d419ee81ce8bdf", 75),
}


def _normalized(value):
    try:
        return Blueprint.model_validate(value).model_dump(mode="json")
    except (TypeError, ValueError):
        raise CustodyError("The selected stock guide has no valid complete plan.") from None


def _read_source_pins():
    contents = {}
    for name, (path, digest, size) in _SOURCE_PINS.items():
        _private_parent(path)
        raw = ExactFile(path, digest).read()
        if len(raw) != size:
            raise CustodyError("An original stock source byte count changed.")
        contents[name] = raw
    return contents


def _guide_snapshot(guide_store):
    state = guide_store.get(STOCK_GUIDE_ID)
    operation = guide_store.operation(STOCK_GUIDE_ID)
    if (not isinstance(state, dict) or state.get("id") != STOCK_GUIDE_ID
            or type(state.get("version")) is not int or state["version"] < 1):
        raise CustodyError("The original stock guide is missing or changed identity.")
    if operation is not None and not isinstance(operation, dict):
        raise CustodyError("The original stock guide operation is unavailable.")
    if operation is not None and operation.get("status") == "running":
        raise CustodyError("Wait for the original stock guide's changes to finish.")
    selected = state.get("proposal") if state.get("proposal") is not None else state.get("blueprint")
    return state["version"], _normalized(selected)


def _require_stock_current(guide_store):
    before = _guide_snapshot(guide_store)
    contents = _read_source_pins()
    expected = _normalized(_json(contents["blueprint"]))
    origin = _json(contents["guide_origin"])
    if (origin.get("guide_id") != STOCK_GUIDE_ID
            or origin.get("source_path") != str(_BLUEPRINT_PATH)
            or origin.get("source_sha256") != _SOURCE_PINS["blueprint"][1]
            or not isinstance(origin.get("submitted_body"), dict)
            or origin["submitted_body"].get("action") != "manual"
            or _normalized(origin["submitted_body"].get("blueprint")) != expected):
        raise CustodyError("The stock guide lost its original imported-source binding.")
    after = _guide_snapshot(guide_store)
    if before[0] != after[0]:
        raise CustodyError("The stock guide changed while checking its original source.")
    if before[1] != expected or after[1] != expected:
        raise CustodyError("The current stock proposal differs from its original selected plan.")
    _read_source_pins()


def build_stock_host(*, guide_store, runtime_target, startup_selection=None):
    """Return the existing host only with actual explicitly supplied future inputs.

    Guide acceptance may advance its version and move an unchanged proposal into
    blueprint. The original currentness callback checks that selected normalized
    plan, not a frozen version/status. Product qualification and the exact private
    installation envelope remain with their existing owners. Preparation may
    precede that envelope; the host retains its existing unavailable Configure
    behavior until an actual startup supplier is selected.
    """
    if (type(guide_store) is not GuideStore or type(runtime_target) is not MetaToolkitRuntimeTarget
            or startup_selection is not None and not callable(startup_selection)):
        raise CustodyError("Supply the existing guide store, actual product and any selected startup supplier.")
    from launchkit.forge_application_host import ForgeApplicationHost

    def require_current():
        _require_stock_current(guide_store)

    require_current()
    acquisition, viewer = (_SOURCE_PINS[name] for name in ("acquisition", "viewer"))
    return ForgeApplicationHost(guide_id=STOCK_GUIDE_ID, runtime_target=runtime_target,
        acquisition_selection=ExactFile(acquisition[0], acquisition[1]),
        viewer_selection=ExactFile(viewer[0], viewer[1]), require_current=require_current,
        startup_selection=startup_selection)
