"""Join selected host callbacks to the native-observed application definition.

The trusted runtime supplies its original roster callback context. This reader
does not derive native profile names, change a roster, attach setup tools, or
grant authority. Reusable callbacks retain their selected native declarations.
"""
from __future__ import annotations

from collections.abc import Mapping
from copy import deepcopy
import json
import tomllib

from .connect_installer_caller import CustodyError, ExactFile, _json, _same_member_classes


_SETUP_RESULTS_BUNDLE = "meerkat_toolkit.setup_results"


def _field(value, name):
    return value.get(name) if isinstance(value, Mapping) else getattr(value, name, None)


def _signature(value):
    try:
        return json.dumps(value, ensure_ascii=True, allow_nan=False, sort_keys=True,
                          separators=(",", ":"))
    except (TypeError, ValueError, RecursionError) as error:
        raise CustodyError("Callback observations must be ordinary JSON values.") from error


def _contains(selected, observed):
    """Check explicit source fields, without recreating native default values."""
    if isinstance(selected, dict):
        return (isinstance(observed, dict) and selected.keys() <= observed.keys()
                and all(_contains(value, observed[key]) for key, value in selected.items()))
    if isinstance(selected, list):
        return (isinstance(observed, list) and len(selected) == len(observed)
                and all(_contains(a, b) for a, b in zip(selected, observed)))
    return _signature(selected) == _signature(observed)


def _source_definition(value):
    if not isinstance(value, dict):
        raise CustodyError("Callbacks require the original selected definition.")
    if value.get("format") == "native" and set(value) == {"format", "definition"}:
        definition = value["definition"]
    elif value.get("format") == "toml" and set(value) == {"format", "text"}:
        try:
            definition = tomllib.loads(value["text"])
        except (TypeError, tomllib.TOMLDecodeError) as error:
            raise CustodyError("The original callback definition is invalid TOML.") from error
        mob = definition.pop("mob", None)
        if not isinstance(mob, dict) or set(mob) - {"id", "orchestrator"}:
            raise CustodyError("Callback source requires the selected native mob fields.")
        definition["id"] = mob.get("id")
        if "orchestrator" in mob:
            orchestrator = mob["orchestrator"]
            definition["orchestrator"] = ({"profile": orchestrator}
                                           if isinstance(orchestrator, str) else orchestrator)
    else:
        raise CustodyError("Callbacks require the exact native or TOML source definition.")
    if (not isinstance(definition, dict) or not isinstance(definition.get("id"), str)
            or not isinstance(definition.get("profiles"), dict)):
        raise CustodyError("Callbacks require a finite source profile inventory.")
    return deepcopy(definition)


def _restore_results_bundle(source_tools, projected_tools):
    """Remove only the observed native result bundle addition, if it was new."""
    original = source_tools.get("rust_bundles", [])
    actual = projected_tools.get("rust_bundles")
    if (not isinstance(original, list) or not isinstance(actual, list)
            or any(not isinstance(name, str) for name in original + actual)):
        return False
    expected = original if _SETUP_RESULTS_BUNDLE in original else original + [_SETUP_RESULTS_BUNDLE]
    if _signature(actual) != _signature(expected):
        return False
    if "rust_bundles" in source_tools:
        projected_tools["rust_bundles"] = deepcopy(original)
    else:
        projected_tools.pop("rust_bundles")
    return True


def _setup_projection(source, projected):
    """Recognize the copied native profile with its setup server/result bundle."""
    if not isinstance(source, dict) or not isinstance(projected, dict):
        return False
    source_tools, projected_tools = source.get("tools"), projected.get("tools")
    if not isinstance(source_tools, dict) or not isinstance(projected_tools, dict):
        return False
    original = source_tools.get("mcp_servers", [])
    actual = projected_tools.get("mcp_servers")
    if (not isinstance(original, list) or not isinstance(actual, list)
            or len(actual) != len(original) + 1
            or any(isinstance(row, dict) and row.get("name") == "toolkit-setup" for row in original)
            or _signature(actual[:-1]) != _signature(original)
            or not isinstance(actual[-1], dict) or actual[-1].get("name") != "toolkit-setup"):
        return False
    unchanged = deepcopy(projected)
    if "mcp_servers" in source_tools:
        unchanged["tools"]["mcp_servers"] = deepcopy(original)
    else:
        unchanged["tools"].pop("mcp_servers", None)
    if not _restore_results_bundle(source_tools, unchanged["tools"]):
        return False
    return _signature(unchanged) == _signature(source)


class SelectedApplicationCallbacks:
    """Original seed and class selections, joined to the native runtime's build."""

    def __init__(self, plan: ExactFile):
        if not isinstance(plan, ExactFile):
            raise TypeError("Application callbacks require the original exact plan pin.")
        self.plan = plan
        self._plan_bytes = plan.read()
        if len(self._plan_bytes) > 256 * 1024:
            raise CustodyError("The selected callback preparation plan exceeds its native bound.")
        value = _json(self._plan_bytes)
        application = value.get("application") if isinstance(value, dict) else None
        if not isinstance(application, dict) or application.get("schema") != 4:
            raise CustodyError("Callbacks require the selected schema4 application plan.")
        self._source = _source_definition(application.get("definition"))
        self._strategy = application.get("helper_profile")
        has_helper = bool({"helper", "helper_profile", "helper_tools"} & application.keys())
        if has_helper and self._strategy not in {"dedicated", "exclusive_application_role"}:
            raise CustodyError("Callbacks require the explicit helper profile strategy.")
        roster, members = application.get("roster"), application.get("members")
        helper = _field(application.get("helper"), "member")
        if (not isinstance(roster, list) or not isinstance(members, list)
                or len(roster) != len(members) or has_helper and not isinstance(helper, dict)):
            raise CustodyError("Callbacks require the same finite roster and native member selection.")
        self._fixed = {}
        self._member_bindings = {}
        selected = []
        fixed_profiles = set()
        native_members = set()
        for row, member in zip(roster, members):
            binding = _field(member, "member")
            if (not isinstance(row, dict) or not isinstance(binding, dict)
                    or not isinstance(row.get("identity"), str) or not row["identity"]
                    or not isinstance(row.get("profile"), str) or not row["profile"]
                    or set(binding) != {"mob_id", "role", "member"}
                    or not isinstance(binding.get("member"), str) or not binding["member"]
                    or binding.get("mob_id") != self._source["id"]
                    or binding.get("role") != row["profile"]
                    or row["identity"] in self._fixed or row["profile"] in fixed_profiles
                    or binding["member"] in native_members):
                raise CustodyError("Callback roster rows must retain their original native profile bindings.")
            self._fixed[row["identity"]] = row["profile"]
            self._member_bindings[row["identity"]] = deepcopy(binding)
            fixed_profiles.add(row["profile"])
            native_members.add(binding["member"])
            if _signature(binding) == _signature(helper):
                selected.append(row)
        self.helper_identity = self.source_profile = None
        if has_helper:
            if len(selected) != 1:
                raise CustodyError("Callback helper selection must identify one exact original roster row.")
            self.helper_identity, self.source_profile = selected[0]["identity"], selected[0]["profile"]
            if self.source_profile not in self._source["profiles"]:
                raise CustodyError("The selected helper has no original inline source profile.")
            source = self._source["profiles"][self.source_profile]
            if not isinstance(source, dict) or "realm_profile" in source:
                raise CustodyError("The selected helper requires its original inline source profile.")
        self._member_classes = {}
        declarations = application.get("member_classes", [])
        if not isinstance(declarations, list):
            raise CustodyError("Member classes require the original native declaration list.")
        profiles = set()
        for item in declarations:
            required = {"id", "profile", "factory", "callbacks", "grants"}
            if (not isinstance(item, dict) or not required <= item.keys()
                    or item.keys() - required - {"purpose", "callback_account"}
                    or any(type(item[key]) is not str or not item[key] or item[key].strip() != item[key]
                           for key in ("id", "profile", "factory"))
                    or item["id"] in self._member_classes or item["profile"] in profiles
                    or item.get("purpose", "ordinary") not in {"ordinary", "shared_workflow"}
                    or not isinstance(item["callbacks"], list) or not isinstance(item["grants"], list)):
                raise CustodyError("Member classes must retain their exact native selection.")
            names = []
            for callback in item["callbacks"]:
                if (not isinstance(callback, dict) or set(callback) != {"name", "description", "input_schema"}
                        or type(callback["name"]) is not str or not callback["name"]
                        or callback["name"].strip() != callback["name"]
                        or type(callback["description"]) is not str
                        or not isinstance(callback["input_schema"], dict)):
                    raise CustodyError("Member class callbacks require their exact declared definitions.")
                names.append(callback["name"])
            if names != sorted(set(names)):
                raise CustodyError("Member class callbacks must be distinct and in native declaration order.")
            _signature(item)
            self._member_classes[item["id"]] = deepcopy(item)
            profiles.add(item["profile"])
        if not roster and not self._member_classes:
            raise CustodyError("An empty callback roster requires selected native member classes.")
        self._definition = None
        self._profile = None
        self._invalidated = False

    def _current(self):
        if self._invalidated or self.plan.read() != self._plan_bytes:
            raise CustodyError("The original application callback selection is no longer current.")

    @property
    def member_classes(self):
        """Selected native declaration data, never an admission or runtime fact."""
        self._current()
        return deepcopy(self._member_classes)

    def require_completed_classes(self, setup_bytes):
        """Bind selected callbacks to the retained native owner's class list.

        Restore supplies its exact completed bytes before constructing a runtime.
        Fresh completion uses the same comparison in the existing finalizer join.
        """
        self._current()
        setup = _json(setup_bytes)
        owner = setup.get("native_owner") if isinstance(setup, dict) else None
        native = owner.get("native_host") if isinstance(owner, dict) else None
        if native is None and not self._member_classes:
            # Class-free legacy carriers have no callback class join. Native
            # still validates their complete owner and installed policy.
            return
        if (not isinstance(native, dict)
                or not _same_member_classes(list(self._member_classes.values()), native.get("member_classes", []))):
            raise CustodyError("The completed native owner differs from the selected callback classes.")
        self._current()

    def dynamic_catalog(self, deployment):
        """Bind compiled class callbacks without creating a template identity."""
        self._current()
        if not isinstance(deployment, dict):
            raise CustodyError("Dynamic composition requires the original application deployment.")
        profiles = deployment.get("applicationProfiles")
        classes = deployment.get("classes")
        if (not isinstance(profiles, dict) or not isinstance(classes, list)
                or any(not isinstance(row, dict) or type(row.get("id")) is not str for row in classes)):
            raise CustodyError("Dynamic composition requires compiled reusable application profiles.")
        ids = [row["id"] for row in classes]
        if len(set(ids)) != len(ids):
            raise CustodyError("Dynamic composition has repeated application classes.")
        result = {}
        for name, item in self._member_classes.items():
            if item.get("purpose", "ordinary") != "ordinary":
                # Shared workflow construction has its own native owner. Its
                # declaration does not become an application callback class.
                continue
            profile = self._source["profiles"].get(item["profile"])
            if (name not in ids
                    or profiles.get(name) != item["profile"] or item["profile"] in self._fixed.values()
                    or not isinstance(profile, dict) or "realm_profile" in profile):
                raise CustodyError("Dynamic classes must match the selected ordinary inline application profiles.")
            result[name] = {row["name"]: deepcopy(row["input_schema"]) for row in item["callbacks"]}
        if not result:
            raise CustodyError("Dynamic composition requires selected native member classes.")
        return result

    def project_identity_spec(self, expected):
        """Permit only Core's observed helper profile change on the exact row."""
        self._current()
        projected = deepcopy(expected)
        identity, profile = projected.get("identity"), projected.get("profile")
        original = self._fixed.get(identity)
        if original is not None:
            if profile != original:
                raise CustodyError("The dynamic seed changed its selected profile.")
            if identity == self.helper_identity:
                projected["profile"] = self.helper_profile
        elif not any(item["profile"] == profile and item.get("purpose", "ordinary") == "ordinary"
                     for item in self._member_classes.values()):
            raise CustodyError("The dynamic identity has no selected native member class.")
        return projected

    def require_member_callbacks(self, agent, callbacks):
        """Compare actual class implementation; native still owns admission."""
        self._current()
        if agent["identity"] in self._fixed:
            if self._fixed[agent["identity"]] != agent["profile"]:
                raise CustodyError("The original seed callback profile changed.")
            return
        item = self._member_classes.get(agent["classId"])
        if (item is None or item.get("purpose", "ordinary") != "ordinary"
                or agent["profile"] != item["profile"]):
            raise CustodyError("The dynamic callback identity has no exact selected class.")
        actual = [{"name": row["name"], "description": row["description"], "input_schema": row["schema"]}
                  for row in callbacks]
        if _signature(sorted(actual, key=lambda row: row["name"])) != _signature(item["callbacks"]):
            raise CustodyError("Actual member callbacks differ from the selected native class declaration.")

    @property
    def native_definition(self):
        self._current()
        if self._definition is None:
            raise CustodyError("The native roster context has not supplied its projected definition.")
        return deepcopy(self._definition)

    @property
    def helper_profile(self):
        self._current()
        if self.helper_identity is None:
            return None
        if self._profile is None:
            raise CustodyError("The native roster context has not supplied its helper profile.")
        return self._profile

    def observe_roster_context(self, context):
        self._current()
        definition = _field(context, "mob_definition")
        if (not isinstance(definition, dict) or definition.get("id") != self._source["id"]
                or not isinstance(definition.get("profiles"), dict)):
            if self._definition is not None:
                self._invalidated = True
            raise CustodyError("The native roster context must retain the selected mob definition.")
        try:
            signature = _signature(definition)
        except CustodyError:
            if self._definition is not None:
                self._invalidated = True
            raise
        if self._definition is not None:
            if signature != _signature(self._definition):
                self._invalidated = True
                raise CustodyError("The native projected callback definition changed.")
            return
        profiles = definition["profiles"]
        source_profiles = self._source["profiles"]
        if not source_profiles.keys() <= profiles.keys():
            raise CustodyError("The native definition lost an original callback profile.")
        if self.helper_identity is None:
            if profiles.keys() != source_profiles.keys() or not _contains(self._source, definition):
                raise CustodyError("The native callback definition changed explicit original configuration.")
            self._definition = deepcopy(definition)
            return
        if self._strategy == "dedicated":
            source = profiles[self.source_profile]
            candidates = [name for name, profile in profiles.items()
                          if name != self.source_profile and _setup_projection(source, profile)]
            if len(candidates) != 1:
                raise CustodyError("The native definition has no unambiguous selected helper projection.")
            profile = candidates[0]
        else:
            profile = self.source_profile
            # The native profile retains explicit original fields and selects
            # one setup server; native still owns its exact typed composition.
            servers = _field(_field(profiles[profile], "tools"), "mcp_servers")
            if (not isinstance(servers, list)
                    or any(not isinstance(row, dict) for row in servers)
                    or sum(isinstance(row, dict) and row.get("name") == "toolkit-setup"
                           for row in servers) != 1):
                raise CustodyError("The native exclusive helper profile lacks its selected setup server.")
        if profiles.keys() - source_profiles.keys() - {profile}:
            raise CustodyError("The native callback definition added an unrelated profile.")
        unchanged = deepcopy(definition)
        if self._strategy == "exclusive_application_role":
            original_tools = source_profiles[profile].get("tools", {})
            tools = unchanged["profiles"][profile]["tools"]
            servers = [row for row in tools["mcp_servers"] if row.get("name") != "toolkit-setup"]
            if servers or "mcp_servers" in original_tools:
                tools["mcp_servers"] = servers
            else:
                tools.pop("mcp_servers")
            if not _restore_results_bundle(original_tools, tools):
                raise CustodyError("The native exclusive helper result bundle changed original selections.")
        if not _contains(self._source, unchanged):
            raise CustodyError("The native callback definition changed explicit original configuration.")
        if any(original == profile and identity != self.helper_identity
               for identity, original in self._fixed.items()):
            raise CustodyError("The observed setup profile belongs to another fixed identity.")
        self._definition, self._profile = deepcopy(definition), profile

    def helper_agent(self, draft, by_profile):
        """Return the original helper row, or None for an ordinary fixed build."""
        self._current()
        if not isinstance(by_profile, Mapping):
            raise TypeError("Callbacks require the original fixed profile map.")
        source = None
        if self.helper_identity is not None:
            source = by_profile.get(self.source_profile)
            if (not isinstance(source, dict) or source.get("identity") != self.helper_identity
                    or source.get("profile") != self.source_profile):
                raise CustodyError("The original fixed helper callback row changed.")
        profile, labels = _field(draft, "profile_name"), _field(draft, "labels")
        # Core's reserved labels identify the native member, not the logical
        # identity in the application roster. Keep the selected opaque binding;
        # no member encoding or spelling is reconstructed here.
        member = _field(labels, "agent_identity")
        if (not isinstance(labels, Mapping) or not isinstance(profile, str)
                or labels.get("mob_id") != self._source["id"]
                or labels.get("role") != profile
                or ("profile_name" in labels and labels["profile_name"] != profile)
                or ("meerkat_id" in labels and labels["meerkat_id"] != member)):
            raise CustodyError("The callback changed its selected native member labels.")
        helper_member = (None if self.helper_identity is None else
            self._member_bindings[self.helper_identity]["member"])
        if self.helper_identity is not None and (member == helper_member or profile == self._profile):
            if (self._profile is None or member != helper_member
                    or profile != self._profile):
                raise CustodyError("The callback does not match the native-observed selected helper.")
            return source
        if profile in {item["profile"] for item in self._member_classes.values()
                       if item.get("purpose", "ordinary") == "ordinary"}:
            if (self._definition is None or profile in self._fixed.values()
                    or type(member) is not str or not member
                    or member in {row["member"] for row in self._member_bindings.values()}):
                raise CustodyError("A reusable profile cannot replace an original native member.")
            # No callback is attached by this profile match. Meta's exact
            # identity customizer and native class admission must compose it.
            return None
        ordinary = by_profile.get(profile)
        binding = (self._member_bindings.get(ordinary.get("identity"))
                   if isinstance(ordinary, dict) else None)
        if (not isinstance(ordinary, dict) or ordinary.get("profile") != profile
                or self._fixed.get(ordinary.get("identity")) != profile
                or binding is None or binding["role"] != profile or binding["member"] != member
                or ordinary.get("identity") == self.helper_identity):
            raise CustodyError("An unknown or foreign fixed profile requested application callbacks.")
        return None
