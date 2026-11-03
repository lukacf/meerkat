"""Private host credentials for the existing native provider environment path."""
from __future__ import annotations

from contextlib import closing, contextmanager
import json
import os
from pathlib import Path
import sqlite3
import stat
from uuid import uuid4

from .planner import ENV_KEYS

MAX_KEY_CHARACTERS = 4096
MAX_SETTINGS_BYTES = 32 * 1024
_FILENAME = "provider-settings.json"
_LOAD_ERROR = "Saved provider settings could not be read safely. Existing environment settings remain available."
# Exact 0.8.42 CredentialSourceSpec::Env order: each primary/fallback
# accepts its RKAT_ override before the ordinary variable.
_NATIVE_ENV_KEYS = {
    "openai": ("RKAT_OPENAI_API_KEY", "OPENAI_API_KEY"),
    "anthropic": ("RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"),
    "gemini": ("RKAT_GEMINI_API_KEY", "GEMINI_API_KEY", "RKAT_GOOGLE_API_KEY", "GOOGLE_API_KEY"),
}
_SPEC_ERROR = "The saved setup provider could not be verified. Preserve its native state before changing provider settings."
_SETUP_PROFILES = {"architect", "connectors", "reviewer", "visualizer"}
_MAX_SPEC_BYTES = 2 * 1024 * 1024


class ProviderSettingsUnavailable(ValueError):
    pass


def canonical_setup_settings(runtime_root: Path):
    """Project provider and compaction tuning from the persisted native spec.

    The caller supplies architect.setup_runtime_root; this does not inspect
    sessions, events, authentication records or a duplicate host mob config.
    Pinned 0.8.42 SqliteMobSpecStore owns mob_specs and its StoredSpec envelope.
    """
    root = Path(runtime_root).absolute()
    state = root / ".mobkit" / "state"
    database = state / "event_log.sqlite3"
    try:
        for directory in (root, root / ".mobkit", state):
            try:
                info = directory.stat(follow_symlinks=False)
            except FileNotFoundError:
                return None
            if not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid():
                raise ProviderSettingsUnavailable(_SPEC_ERROR)
        try:
            info = database.stat(follow_symlinks=False)
        except FileNotFoundError:
            if not any(state.iterdir()):
                return None
            raise ProviderSettingsUnavailable(_SPEC_ERROR) from None
        if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_nlink != 1:
            raise ProviderSettingsUnavailable(_SPEC_ERROR)
        for suffix in ("-wal", "-shm"):
            sidecar = database.with_name(database.name + suffix)
            try:
                sidecar_info = sidecar.stat(follow_symlinks=False)
            except FileNotFoundError:
                continue
            if (not stat.S_ISREG(sidecar_info.st_mode) or sidecar_info.st_uid != os.getuid()
                    or sidecar_info.st_nlink != 1):
                raise ProviderSettingsUnavailable(_SPEC_ERROR)
        with closing(sqlite3.connect(database.as_uri() + "?mode=ro", uri=True, timeout=1)) as connection:
            connection.setlimit(sqlite3.SQLITE_LIMIT_LENGTH, _MAX_SPEC_BYTES + 1024)
            connection.execute("PRAGMA query_only=ON")
            # Read only one exact canonical spec, never the event/history rows.
            rows = connection.execute(
                "SELECT substr(spec_json, 1, ?) FROM mob_specs WHERE mob_id = ? LIMIT 2",
                (_MAX_SPEC_BYTES + 1, "launch-architect"),
            ).fetchall()
        if len(rows) != 1 or type(rows[0][0]) not in (str, bytes) or len(rows[0][0]) > _MAX_SPEC_BYTES:
            raise ProviderSettingsUnavailable(_SPEC_ERROR)
        value = json.loads(rows[0][0])
        if (type(value) is not dict or type(value.get("revision")) is not int
                or value["revision"] < 1 or type(value.get("definition")) is not dict):
            raise ProviderSettingsUnavailable(_SPEC_ERROR)
        definition = value["definition"]
        profiles = definition.get("profiles")
        if definition.get("id") != "launch-architect" or type(profiles) is not dict or set(profiles) != _SETUP_PROFILES:
            raise ProviderSettingsUnavailable(_SPEC_ERROR)
        providers = set()
        thresholds = {}
        for name, profile in profiles.items():
            if (type(profile) is not dict or "realm_profile" in profile
                    or type(profile.get("provider")) is not str or profile["provider"] not in ENV_KEYS):
                raise ProviderSettingsUnavailable(_SPEC_ERROR)
            providers.add(profile["provider"])
            threshold = profile.get("auto_compact_threshold")
            # Native Profile owns Option<NonZeroU64>; the ordinary compiler's
            # TOML integer representation must also preserve the exact value.
            if threshold is not None and (type(threshold) is not int or not 1 <= threshold <= (1 << 63) - 1):
                raise ProviderSettingsUnavailable(_SPEC_ERROR)
            thresholds[name] = threshold
        if len(providers) != 1:
            raise ProviderSettingsUnavailable(_SPEC_ERROR)
        after = database.stat(follow_symlinks=False)
        if (after.st_dev, after.st_ino, after.st_uid, after.st_nlink) != (info.st_dev, info.st_ino, info.st_uid, 1):
            raise ProviderSettingsUnavailable(_SPEC_ERROR)
        return {"provider": providers.pop(), "auto_compact_thresholds": thresholds}
    except ProviderSettingsUnavailable:
        raise
    except (OSError, sqlite3.Error, ValueError, TypeError, UnicodeError, RecursionError):
        raise ProviderSettingsUnavailable(_SPEC_ERROR) from None


def canonical_setup_provider(runtime_root: Path):
    """Retain the provider-only projection used by credential settings."""
    settings = canonical_setup_settings(runtime_root)
    return settings["provider"] if settings is not None else None


def _checked_key(value):
    if (type(value) is not str or not value.strip() or len(value) > MAX_KEY_CHARACTERS
            or any(ord(character) < 32 or ord(character) == 127 for character in value)
            or len(value.encode("utf-8")) > 8192):
        raise ValueError("Enter a nonempty provider API key of at most 4096 characters without control characters.")
    return value.strip()


def _identity(info):
    return (info.st_dev, info.st_ino, info.st_mode, info.st_uid, info.st_nlink,
            info.st_size, info.st_mtime_ns)


class ProviderSettings:
    """Inherited keys win; saved keys are never placed in guide or model data.

    The caller must hold its startup lock and check all native owners before
    save. No coroutine suspension occurs during the file/environment update.
    """

    def __init__(self, root: Path, *, environ=None):
        self.root = Path(root)
        self.environ = os.environ if environ is None else environ
        self._applied = {}
        self.inherited = {}
        for provider in ENV_KEYS:
            external, value = self._external(provider)
            if external:
                self.inherited[provider] = value

    def _external(self, provider):
        if provider in self.inherited:
            return True, self.inherited[provider]
        canonical = ENV_KEYS[provider]
        for name in _NATIVE_ENV_KEYS[provider]:
            if name in self.environ:
                value = self.environ[name]
                if name == canonical and (not value or value == self._applied.get(provider)):
                    return False, None
                return True, value
        return False, None

    @contextmanager
    def _directory(self, *, create=False):
        if create:
            self.root.mkdir(parents=True, exist_ok=True, mode=0o700)
        descriptor = os.open(self.root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
        try:
            info = os.fstat(descriptor)
            if (not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid()
                    or info.st_mode & 0o077
                    or _identity(self.root.stat(follow_symlinks=False)) != _identity(info)):
                raise ProviderSettingsUnavailable("Provider settings need an owned private local state directory.")
            yield descriptor
        finally:
            os.close(descriptor)

    @staticmethod
    def _read(directory):
        try:
            descriptor = os.open(_FILENAME, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK,
                                 dir_fd=directory)
        except FileNotFoundError:
            return {}, None
        try:
            info = os.fstat(descriptor)
            if (not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid()
                    or info.st_nlink != 1 or info.st_mode & 0o077
                    or info.st_size > MAX_SETTINGS_BYTES):
                raise ProviderSettingsUnavailable(_LOAD_ERROR)
            with os.fdopen(descriptor, "rb", closefd=False) as stream:
                content = stream.read(MAX_SETTINGS_BYTES + 1)
            if (len(content) > MAX_SETTINGS_BYTES
                    or _identity(os.fstat(descriptor)) != _identity(info)
                    or _identity(os.stat(_FILENAME, dir_fd=directory, follow_symlinks=False)) != _identity(info)):
                raise ProviderSettingsUnavailable(_LOAD_ERROR)
        finally:
            os.close(descriptor)
        try:
            data = json.loads(content)
        except RecursionError:
            raise ProviderSettingsUnavailable(_LOAD_ERROR) from None
        if (type(data) is not dict or set(data) != {"version", "keys"}
                or type(data["version"]) is not int or data["version"] != 1
                or type(data["keys"]) is not dict or set(data["keys"]) - set(ENV_KEYS)):
            raise ProviderSettingsUnavailable(_LOAD_ERROR)
        keys = {provider: _checked_key(value) for provider, value in data["keys"].items()}
        return keys, (content, _identity(info))

    def _snapshot(self):
        try:
            with self._directory() as directory:
                return self._read(directory), None
        except FileNotFoundError:
            # An absent state directory has no saved settings. A broken root
            # symlink is not an absent directory.
            if not self.root.is_symlink() and not self.root.exists():
                return ({}, None), None
            return ({}, None), _LOAD_ERROR
        except (OSError, ValueError, UnicodeError):
            return ({}, None), _LOAD_ERROR

    def _apply(self, keys):
        for provider, name in ENV_KEYS.items():
            external, _ = self._external(provider)
            if external:
                continue
            if provider in keys:
                self.environ[name] = keys[provider]
                self._applied[provider] = keys[provider]

    def restore(self):
        """Load once before native startup; damaged saved input is nonfatal."""
        for provider, value in self.inherited.items():
            if value and not self.environ.get(ENV_KEYS[provider]):
                # Alias-only inherited credentials remain native-owned while
                # the existing planner sees the same available provider.
                self.environ[ENV_KEYS[provider]] = value
        (keys, _), error = self._snapshot()
        if error is None:
            self._apply(keys)
        return self._public(keys, error, False)

    def _public(self, keys, error, runtime_active):
        providers = []
        for provider, name in ENV_KEYS.items():
            value = self.environ.get(name)
            external, inherited_value = self._external(provider)
            source = "environment" if external else "saved" if provider in keys or value else None
            providers.append({"provider": provider, "configured": bool(inherited_value) if external else bool(value or provider in keys),
                              "source": source, "editable": not bool(external) and not runtime_active})
        effective = next((row['provider'] for row in providers if row['configured']), None)
        return {"providers": providers, "effective_provider": effective,
                "runtime_active": bool(runtime_active), "load_error": error}

    def status(self, *, runtime_active=False):
        (keys, _), error = self._snapshot()
        return self._public(keys, error, runtime_active)

    def save(self, provider, api_key, *, runtime_active=False):
        if runtime_active:
            raise ProviderSettingsUnavailable("Stop active setup and application work before changing provider settings.")
        if type(provider) is not str or provider not in ENV_KEYS:
            raise ValueError("Choose OpenAI, Anthropic or Gemini.")
        key = _checked_key(api_key)
        if self._external(provider)[0]:
            raise ProviderSettingsUnavailable("This provider is configured by the server environment and cannot be replaced here.")
        temporary = ".provider-settings-" + uuid4().hex + ".pending"
        try:
            with self._directory(create=True) as directory:
                keys, expected = self._read(directory)
                keys[provider] = key
                content = json.dumps({"version": 1, "keys": keys}, sort_keys=True, ensure_ascii=True).encode("utf-8")
                if len(content) > MAX_SETTINGS_BYTES:
                    raise ProviderSettingsUnavailable("Provider settings exceed the private file size limit.")
                descriptor = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW,
                                     0o600, dir_fd=directory)
                try:
                    with os.fdopen(descriptor, "wb") as stream:
                        stream.write(content)
                        stream.flush()
                        os.fsync(stream.fileno())
                    if self._read(directory)[1] != expected:
                        raise ProviderSettingsUnavailable("Provider settings changed during setup. Reload before saving.")
                    os.replace(temporary, _FILENAME, src_dir_fd=directory, dst_dir_fd=directory)
                    os.fsync(directory)
                finally:
                    try:
                        os.unlink(temporary, dir_fd=directory)
                    except FileNotFoundError:
                        pass
        except ProviderSettingsUnavailable:
            raise
        except (OSError, ValueError, UnicodeError):
            raise ProviderSettingsUnavailable("Saving provider settings was not confirmed. They may have been saved; check settings before retrying. No new key was applied to the current process.") from None
        self._apply(keys)
        return self._public(keys, None, False)
