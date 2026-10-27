"""Meta builder chain for composing and booting a Meta runtime."""
from __future__ import annotations

import math
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Awaitable, Callable, Iterable, Mapping, Sequence

from .identity_first_models import IdentityBootstrapMode, RoleMigrationDeclaration
from .live import ExperimentalLiveGatewayConfig, OpenAiLiveGatewayConfig
from .models import ToolArtifactRef


@dataclass
class MetaBuilderConfig:
    mob_config_path: str | None = None
    mob_config_inline: str | None = None
    prepared_application_bundle: bytes | None = None
    prepared_application_digest: str | None = None
    session_builder: Any | None = None
    error_callback: Any | None = None
    job_credential_resolver: Any | None = None
    event_log: Any | None = None
    runtime_store: Any | None = None
    console_read_only: bool | None = None
    console_fetch_timeout_ms: int | None = None
    console_require_app_auth: bool | None = None
    console_config_path: str | None = None
    http_listen: str | None = None
    http_public_base_url: str | None = None
    allow_remote: bool | None = None
    gating_config_path: str | None = None
    access_config_path: str | None = None
    meerkat_config_path: str | None = None
    provider_credentials_root: str | None = None
    workgraph_enabled: bool | str | None = None
    # ``runtime_options.mob_announcements``, sent as given; ``None`` sends nothing.
    mob_announcements: dict[str, Any] | None = None
    fork_instruction_policy: str | None = None
    routing_config_path: str | None = None
    host_runnables: list[str] = field(default_factory=list)
    installed_artifacts: list[ToolArtifactRef] | None = None
    memory_config: Any | None = None
    agent_memory_config: Any | None = None
    auth_config: Any | None = None
    implicit_delegate_idle_retire_secs: int | None = None
    implicit_delegate_idle_retire_configured: bool = False
    gateway_bin: str | None = None
    # Caller cap, in seconds, on waiting for an accepted meta/init to
    # settle. None waits until the gateway settles, exits, or the reader
    # fails. Running out raises InitOutcomeUnknownError.
    init_deadline: float | None = None
    gateway_init_timeout: float = 60.0
    # The meta/init parameter shutdown_in_flight_drain_ms; None sends nothing.
    shutdown_in_flight_drain_ms: int | None = None
    modules: list[dict[str, Any]] = field(default_factory=list)
    extra_routes: Any | None = None
    persistent_state: str | None = None
    declare_spec_update_expected_revision: int | None = None
    # Identity-first provider fields (REQ-44)
    roster_provider: Any | None = None
    role_migrations: list[dict[str, str]] | None = None
    application_tool_policies: list[str] | None = None
    child_application_tool_policy: dict[str, str] | None = None
    topology_provider: Any | None = None
    agent_customizer: Any | None = None
    identity_bootstrap_mode: IdentityBootstrapMode | None = None
    experimental_live_config: ExperimentalLiveGatewayConfig | None = None
    openai_live_config: OpenAiLiveGatewayConfig | None = None
    console_voice_config: OpenAiLiveGatewayConfig | None = None


class MetaBuilder:
    """Chainable builder for Meta runtime configuration.

    Usage:
        runtime = await (
            Meta.builder()
            .mob("config/mob.toml")
            .session_service(builder)
            .roster(my_roster)
            .build()
        )
    """

    def __init__(self) -> None:
        self._config = MetaBuilderConfig()

    def mob(self, config_path: str) -> MetaBuilder:
        """Set the mob definition from a TOML file path."""
        self._config.mob_config_path = config_path
        return self

    def mob_inline(self, toml_content: str) -> MetaBuilder:
        """Set the mob definition from an inline TOML string.

        Mutually exclusive with mob(config_path).
        """
        self._config.mob_config_inline = toml_content
        return self

    def prepared_application(self, bundle: bytes, digest: str) -> MetaBuilder:
        """Start from an authenticated prepared application instead of mob TOML.

        ``bundle`` is the prepared application's bytes exactly as prepared,
        and ``digest`` the digest declared for them. Both are transport only:
        the gateway's native host composition authenticates them before any
        definition, storage, identity or roster step, and refuses the start
        when it cannot. Nothing here parses or reconstructs the bundle.

        Mutually exclusive with mob() and mob_inline().
        """
        if not isinstance(bundle, (bytes, bytearray)):
            raise TypeError("prepared_application bundle must be bytes")
        if not isinstance(digest, str) or not digest:
            raise ValueError("prepared_application digest must be a non-empty string")
        self._config.prepared_application_bundle = bytes(bundle)
        self._config.prepared_application_digest = digest
        return self

    def session_service(self, builder: Any) -> MetaBuilder:
        self._config.session_builder = builder
        return self

    def event_log(self, config: Any = None, *, storage: Any = None, **kwargs: Any) -> MetaBuilder:
        """Configure the operational event log.

        Preferred form — a typed declaration from
        :mod:`meerkat_meta.config.event_log`::

            from meerkat_meta.config import event_log
            Meta.builder().event_log(event_log.memory(batch_size=64))
            Meta.builder().event_log(event_log.null())

        The gateway accepts only the declared ephemeral storage kinds
        (``"memory"`` — bounded queryable in-process store; ``"null"`` —
        events dropped) and rejects anything else at startup. Without this
        call no events are ingested at all.

        Args:
            config: A typed config (``event_log.memory()`` / ``event_log.null()``)
                or a raw wire dict.
            storage: Legacy keyword form: an EventLogStore implementation or
                a storage string; mutually exclusive with ``config``.
            **kwargs: Additional wire fields for the keyword form
                (batch_size, flush_interval_ms, filter).
        """
        if config is not None and storage is not None:
            raise ValueError("event_log accepts either config or storage=..., not both")
        if config is not None:
            if kwargs:
                raise ValueError("event_log keyword options apply to the storage=... form only")
            self._config.event_log = config
            return self
        if storage is None:
            raise ValueError("event_log requires a config or storage=...")
        self._config.event_log = {"storage": storage, **kwargs}
        return self

    def runtime_store(self, config: Any) -> MetaBuilder:
        """Declare the runtime store's durability explicitly.

        The gateway's runtime store (session resume/archive/retire) is
        persistent SQLite by default; a failed open is a startup error, not
        a silent in-memory fallback. The only declarable alternative is the
        explicit in-memory form::

            from meerkat_meta.config import runtime_store
            Meta.builder().runtime_store(runtime_store.memory())

        Sessions then do not survive gateway restart, and the choice shows
        up in the storage census (``meta/status`` → ``storage.slots``).
        Accepts the typed config or the raw wire dict
        ``{"storage": "memory"}``.
        """
        self._config.runtime_store = config
        return self

    def on_error(self, callback: Callable[..., Any] | Callable[..., Awaitable[Any]]) -> MetaBuilder:
        """Register an error hook for operational alerting.

        The callback receives an ``ErrorEvent`` and is fire-and-forget.
        It can be sync or async::

            async def on_error(event: ErrorEvent):
                await slack.post(f"[{event.category}] {event.message}")

            rt = await Meta.builder().mob("mob.toml").on_error(on_error).build()
        """
        self._config.error_callback = callback
        return self

    def job_credentials(
        self,
        resolver: Callable[
            [str | None, tuple[str, ...]],
            Any | Awaitable[Any],
        ],
    ) -> MetaBuilder:
        """Resolve detached-job credentials afresh for each committed attempt.

        The resolver receives the runner's single bound profile name and the
        required scopes. It must return an ephemeral mapping; Meta passes it
        only to the host runner context and never serializes it to job RPC.
        """
        if not callable(resolver):
            raise TypeError(
                f"job credential resolver must be callable, got {type(resolver).__name__}"
            )
        self._config.job_credential_resolver = resolver
        return self

    def gating(self, config_path: str) -> MetaBuilder:
        """Load gating tiers and escalation from an existing config file.

        Conventionally ``config/gating.toml`` (auto-discovered when present).
        The gateway reads and validates the file at startup: a missing path,
        a directory, or an unreadable or invalid file refuses initialization,
        and an explicit empty path is rejected here. Without this, gating
        stays off only when nothing is at ``config/gating.toml`` (a missing or
        dangling parent directory counts as nothing there). Any other entry
        there (even a directory or a dangling ``gating.toml`` link) and any
        lookup error other than "not found" is forwarded to the gateway's
        strict loader, which refuses initialization when it cannot open or
        read the file or rejects its contents. The loader does not check the
        file type, so a FIFO there blocks startup instead of refusing it.
        """
        if config_path == "":
            raise ValueError(
                "gating requires a non-empty path: since Meta 0.9.0 an empty path no longer "
                "switches gating off. To keep gating off, do not call gating(), and keep "
                "nothing at config/gating.toml under the working directory, which the "
                "builder loads when present"
            )
        self._config.gating_config_path = config_path
        return self

    def access_control(self, config_path: str) -> MetaBuilder:
        """Enable ABAC access control backed by an existing TOML file.

        Conventionally ``config/access.toml`` (auto-discovered when present).
        The gateway reads and validates the file at startup: a missing path, a
        directory, or an unreadable or invalid file refuses initialization,
        and an explicit empty path is rejected here. Name the first admin in
        the file; a configuration without admins grants administration to
        nobody. Console admin edits persist back to the same path. Without
        this, access control stays off only when nothing is at
        ``config/access.toml`` (a missing or dangling parent directory counts
        as nothing there). Any other entry there (even a directory or a
        dangling ``access.toml`` link) and any lookup error other than "not
        found" is forwarded to the gateway's strict loader, which refuses
        initialization when it cannot open, read, parse or validate the file.
        The loader does not check the file type, so a FIFO there blocks
        startup instead of refusing it.
        """
        if config_path == "":
            raise ValueError(
                "access_control requires a non-empty path: since Meta 0.9.0 an empty path "
                "no longer switches access control off. To keep access control off, do not "
                "call access_control(), and keep nothing at config/access.toml under the "
                "working directory, which the builder loads when present"
            )
        self._config.access_config_path = config_path
        return self

    def meerkat_config(self, path: str | Path) -> MetaBuilder:
        """Load a meerkat host ``config.toml`` into every agent the gateway builds.

        Emitted as ``runtime_options.meerkat_config_path``. This is the only door
        for the tables meerkat keeps in its host config rather than in
        ``mob.toml``: ``[self_hosted]`` (servers and model aliases) and
        ``[realm]`` (backend, auth and binding profiles). A host-config
        ``[models]`` table reaches the factory too, but a profile's ``model``
        must be catalogued, provider-annotated or defined under ``mob.toml``'s
        own ``[models.<id>]`` to pass the init model check. The gateway reads
        the file once at init and refuses a missing, malformed or
        non-validating file (meerkat's ``Config::validate``); it also
        refuses a ``mob.toml`` that carries ``[self_hosted]`` or ``[realm]`` at
        top level, so a table in the wrong file is an error rather than a silent
        drop. Opt-in: without this call the gateway builds from meerkat's default
        config as before. Mirrors the TypeScript builder's ``meerkatConfig``.
        """
        text = str(path)
        if not text.strip():
            raise ValueError("meerkat_config path must not be empty")
        self._config.meerkat_config_path = text
        return self

    def provider_credentials_root(self, path: str | Path) -> MetaBuilder:
        """Keep every agent's provider credentials under ``path``.

        Emitted as ``runtime_options.provider_credentials_root``. ``path`` is
        the credentials directory itself, an absolute path: the gateway opens
        it once at init (creating it when missing), and every agent it builds
        (members, the agents they create, live sessions and the agent memory
        engines) resolves and persists provider logins and tokens there
        instead of the process default ``<config dir>/meerkat/credentials``,
        which every process of the OS user shares. A relative path, or a root
        the gateway cannot open, refuses init; it never falls back to the
        default. Runtimes that share a root share provider logins, so give
        each user their own, and pass the same root on every reopen of that
        user's state. A host that kept users apart with a per-process
        ``XDG_CONFIG_HOME=<C>`` passes ``<C>/meerkat/credentials`` to keep
        their existing logins. Opt-in: without this call the gateway keeps the
        process default as before. Mirrors the TypeScript builder's
        ``providerCredentialsRoot``.
        """
        text = str(path)
        if not text.strip():
            raise ValueError("provider_credentials_root path must not be empty")
        self._config.provider_credentials_root = text
        return self

    def openai_live(self, config: OpenAiLiveGatewayConfig) -> MetaBuilder:
        """Install the public OpenAI Live (``gpt-live-1``) host registration.

        Serialized as ``runtime_options.openai_live``. Disabled by default and
        requires a gateway binary compiled with ``openai-live``. It exposes
        stdio RPC only and does not mount an HTTP live route. Mutually
        exclusive with the deprecated :meth:`experimental_live`.
        """
        if not isinstance(config, OpenAiLiveGatewayConfig):
            raise TypeError("openai_live requires OpenAiLiveGatewayConfig")
        config.to_dict()
        self._config.openai_live_config = config
        return self

    def console_voice(self, config: OpenAiLiveGatewayConfig) -> MetaBuilder:
        """Register authenticated console voice via ``runtime_options.console_voice``.

        Requires persistent sessions, console auth, and an ``openai-live``
        gateway. The principal must match the authenticated console subject.
        """
        if not isinstance(config, OpenAiLiveGatewayConfig):
            raise TypeError("console_voice requires OpenAiLiveGatewayConfig")
        config.to_dict()
        self._config.console_voice_config = config
        return self

    def experimental_live(
        self, config: ExperimentalLiveGatewayConfig
    ) -> MetaBuilder:
        """DEPRECATED: install the pre-release GPT Live host registration.

        Prefer :meth:`openai_live`. This is disabled by default and requires a
        gateway binary compiled with ``experimental-gpt-live`` plus matching
        Gate0 build evidence. It exposes stdio RPC only and does not mount an
        HTTP live route.
        """
        if not isinstance(config, ExperimentalLiveGatewayConfig):
            raise TypeError("experimental_live requires ExperimentalLiveGatewayConfig")
        config.to_dict()
        self._config.experimental_live_config = config
        return self

    def workgraph(self, enabled: bool | str = True) -> MetaBuilder:
        """Enable or disable the WorkGraph subsystem (default: enabled).

        Wired into ``runtime_options.workgraph``. Omitted entirely when never
        called, so a gateway with its own default is left alone. A string is
        an explicit durable-store DIRECTORY (``workgraph.sqlite3`` created
        inside) — for launches without a state dir that would otherwise be
        memory-backed.
        """
        self._config.workgraph_enabled = enabled if isinstance(enabled, str) else bool(enabled)
        return self

    def mob_announcements(self, config: Mapping[str, Any] | bool = True) -> MetaBuilder:
        """Enable mob announcements (``runtime_options.mob_announcements``).

        ``True`` sends ``{"enabled": true}`` and ``False`` sends
        ``{"enabled": false}``. A mapping is sent unchanged, for the batch,
        retention and limit options (``{"enabled": true, "batch": {...},
        "retention_ms": ...}``; see the configuration reference). The gateway
        enables them only on a persistent, identity-first launch, and refuses
        the init otherwise. Omitted entirely when never called.
        """
        if isinstance(config, bool):
            self._config.mob_announcements = {"enabled": config}
        else:
            self._config.mob_announcements = dict(config)
        return self

    def fork_instruction_policy(self, policy: str) -> MetaBuilder:
        """Set what a fork of a member with activated host instructions does.

        Emitted as ``runtime_options.fork_instruction_policy``:
        ``"require_explicit_activation"`` (the gateway default when unset),
        ``"reactivate_inherited"`` (the fork re-activates, in the child, every
        key that was effective in the source at the fork) or ``"refuse"``.
        """
        if policy not in ("require_explicit_activation", "reactivate_inherited", "refuse"):
            raise ValueError(
                "fork_instruction_policy must be 'require_explicit_activation', "
                "'reactivate_inherited' or 'refuse'"
            )
        self._config.fork_instruction_policy = policy
        return self

    def routing(self, config_path: str) -> MetaBuilder:
        self._config.routing_config_path = config_path
        return self

    def host_runnables(self, names: Sequence[str]) -> MetaBuilder:
        """Register named deterministic (non-LLM) schedule targets.

        Wired into ``runtime_options.host_runnables``. Each name becomes a
        schedule-host runnable: a durable schedule with target
        ``{"target_kind": "host_runnable", "runnable": "<name>"}`` fires as
        ``callback/schedule_fire`` into this process, dispatched to the
        handler registered via :meth:`MetaRuntime.on_schedule_fire`.
        """
        names = list(names)
        for name in names:
            if not isinstance(name, str) or not name.strip():
                raise ValueError(f"host runnable names must be non-empty strings, got {name!r}")
        self._config.host_runnables = names
        return self

    def installed_artifacts(self, artifacts: Iterable[ToolArtifactRef]) -> MetaBuilder:
        """Name the artifacts this host installed (extension packages, plugins).

        Wired into ``runtime_options.installed_artifacts`` at the trusted
        bootstrap. With a set, every callback tool must name a
        :class:`ToolArtifactRef` from it (``register_tool(..., artifact=...)``)
        whose digest matches; an unknown, mismatched or missing reference
        refuses the build before any tool is published. Without a set,
        callback tools stay unassociated. Attribution only, never authority.
        """
        artifacts = list(artifacts)
        installed: dict[str, str] = {}
        for artifact in artifacts:
            if not isinstance(artifact, ToolArtifactRef):
                raise TypeError(
                    "installed artifacts must be ToolArtifactRef, got "
                    f"{type(artifact).__name__}: {artifact!r}"
                )
            if installed.setdefault(artifact.artifact_id, artifact.digest) != artifact.digest:
                raise ValueError(
                    f"installed artifact {artifact.artifact_id!r} is listed with two different "
                    "digests"
                )
        self._config.installed_artifacts = artifacts
        return self

    def memory(self, config: Any = None, *, stores: list[str] | None = None) -> MetaBuilder:
        """Set memory config. Accepts config object or stores=["knowledge_graph", ...]."""
        if config is None and stores is not None:
            raise ValueError(
                "memory(stores=...) is not supported by the Rust gateway; "
                "pass memory.local_json()"
            )
        self._config.memory_config = config
        return self

    def agent_memory(self, config: Any = True, **kwargs: Any) -> MetaBuilder:
        """Configure identity-scoped durable agent memory.

        With no arguments this enables the gateway default. Keyword arguments
        use Python names and serialize to the Rust gateway's snake_case wire
        keys, for example ``agent_memory(selection="contextual", max_entries=3)``.

        ``store`` accepts only ``"sqlite"`` (the default). ``store="markdown"``
        is retired as a live store: the gateway refuses it at init (-32014) and
        names the migration, which is one lossless step - point the default
        sqlite store at the SAME agent-memory directory and it imports every
        un-imported ``.md`` file on first open, preserving ids, tags and
        timestamps and renaming each source ``.md.imported``. Tag CONTENT is
        preserved; the store collates tags, so order is not.

        ``selector`` is RETIRED. The LLM recall-selector stage was removed
        unactivated, so recall is the deterministic lexical path on every turn.
        The gateway accepts only absence or ``"off"`` for migration
        compatibility and refuses every activation-shaped value.

        ``hygienist`` is PARKED. Only ``False`` or a mapping with
        ``enabled=False`` is accepted for migration compatibility. The
        internal Rust engine remains available for validation, but the public
        SDK/gateway path has no supported activation or provider proof.
        """
        if kwargs:
            if config is not True:
                raise ValueError("agent_memory accepts either config or keyword options")
            config = kwargs
        if config is False or config is None:
            self._config.agent_memory_config = {"enabled": False}
            return self
        if config is True:
            self._config.agent_memory_config = True
            return self
        if not isinstance(config, dict):
            self._config.agent_memory_config = config
            return self

        # Fail loud on unknown keys instead of silently dropping them: a
        # typo'd option would otherwise be scrubbed here and the gateway's
        # own unknown-field rejection would never fire.
        known = {
            "enabled",
            "realm",
            "selection",
            "max_entries",
            "maxEntries",
            "recall_timeout_ms",
            "recallTimeoutMs",
            "recall_failure_policy",
            "recallFailurePolicy",
            "instruction_header",
            "instructionHeader",
            "per_turn_injection",
            "perTurnInjection",
            "defang_inbound",
            "defangInbound",
            "store",
            "llm_writes",
            "llmWrites",
            "recorder_tool",
            "recorderTool",
            "content_trust",
            "contentTrust",
            "selector",
            "distiller",
            "steward",
            "operator_scope",
            "operatorScope",
            "hygienist",
        }
        unknown = sorted(set(config) - known)
        if unknown:
            raise ValueError(
                "agent_memory got unsupported option(s): " + ", ".join(unknown)
            )

        wire: dict[str, Any] = {}
        if "enabled" in config:
            wire["enabled"] = config["enabled"]
        if "realm" in config:
            wire["realm"] = config["realm"]
        if "selection" in config:
            wire["selection"] = config["selection"]
        if "max_entries" in config:
            wire["max_entries"] = config["max_entries"]
        elif "maxEntries" in config:
            wire["max_entries"] = config["maxEntries"]
        if "recall_timeout_ms" in config:
            wire["recall_timeout_ms"] = config["recall_timeout_ms"]
        elif "recallTimeoutMs" in config:
            wire["recall_timeout_ms"] = config["recallTimeoutMs"]
        if "recall_failure_policy" in config:
            wire["recall_failure_policy"] = config["recall_failure_policy"]
        elif "recallFailurePolicy" in config:
            wire["recall_failure_policy"] = config["recallFailurePolicy"]
        if "instruction_header" in config:
            wire["instruction_header"] = config["instruction_header"]
        elif "instructionHeader" in config:
            wire["instruction_header"] = config["instructionHeader"]
        if "per_turn_injection" in config:
            wire["per_turn_injection"] = config["per_turn_injection"]
        elif "perTurnInjection" in config:
            wire["per_turn_injection"] = config["perTurnInjection"]
        if "defang_inbound" in config:
            wire["defang_inbound"] = config["defang_inbound"]
        elif "defangInbound" in config:
            wire["defang_inbound"] = config["defangInbound"]
        # Forwarded verbatim on purpose. `store="markdown"` is retired and the
        # gateway refuses it with a typed migration verdict; re-deciding that
        # here would be a second definition of one policy, and this SDK would
        # be the copy that goes stale.
        if "store" in config:
            wire["store"] = config["store"]
        if "llm_writes" in config:
            wire["llm_writes"] = config["llm_writes"]
        elif "llmWrites" in config:
            wire["llm_writes"] = config["llmWrites"]
        if "recorder_tool" in config:
            wire["recorder_tool"] = config["recorder_tool"]
        elif "recorderTool" in config:
            wire["recorder_tool"] = config["recorderTool"]
        if "content_trust" in config:
            wire["content_trust"] = config["content_trust"]
        elif "contentTrust" in config:
            wire["content_trust"] = config["contentTrust"]
        if "selector" in config:
            if config["selector"] != "off":
                raise ValueError(
                    "agent_memory selector is RETIRED; remove it or set selector='off'"
                )
            wire["selector"] = config["selector"]
        if "operator_scope" in config:
            wire["operator_scope"] = config["operator_scope"]
        elif "operatorScope" in config:
            wire["operator_scope"] = config["operatorScope"]
        distiller = config.get("distiller")
        if distiller is not None:
            if isinstance(distiller, dict):
                unknown = sorted(
                    set(distiller)
                    - {
                        "enabled",
                        "runs_per_hour",
                        "runsPerHour",
                        "min_interactions",
                        "minInteractions",
                        "max_output_tokens",
                        "maxOutputTokens",
                        "minInteractions",
                        "model",
                    }
                )
                if unknown:
                    raise ValueError(
                        "agent_memory distiller got unsupported option(s): "
                        + ", ".join(unknown)
                    )
                distiller_wire: dict[str, Any] = {}
                if "enabled" in distiller:
                    distiller_wire["enabled"] = distiller["enabled"]
                if "max_output_tokens" in distiller:
                    distiller_wire["max_output_tokens"] = distiller["max_output_tokens"]
                elif "maxOutputTokens" in distiller:
                    distiller_wire["max_output_tokens"] = distiller["maxOutputTokens"]
                if "runs_per_hour" in distiller:
                    distiller_wire["runs_per_hour"] = distiller["runs_per_hour"]
                elif "runsPerHour" in distiller:
                    distiller_wire["runs_per_hour"] = distiller["runsPerHour"]
                if "min_interactions" in distiller:
                    distiller_wire["min_interactions"] = distiller["min_interactions"]
                elif "minInteractions" in distiller:
                    distiller_wire["min_interactions"] = distiller["minInteractions"]
                if "model" in distiller:
                    distiller_wire["model"] = distiller["model"]
                wire["distiller"] = distiller_wire
            else:
                wire["distiller"] = distiller
        steward = config.get("steward")
        if steward is not None:
            if isinstance(steward, dict):
                unknown = sorted(
                    set(steward)
                    - {
                        "enabled",
                        "cadence",
                        "model",
                        "per_mob",
                        "perMob",
                        "runs_per_day",
                        "runsPerDay",
                        "min_signals",
                        "minSignals",
                        "max_output_tokens",
                        "maxOutputTokens",
                    }
                )
                if unknown:
                    raise ValueError(
                        "agent_memory steward got unsupported option(s): "
                        + ", ".join(unknown)
                    )
                steward_wire: dict[str, Any] = {}
                if "enabled" in steward:
                    steward_wire["enabled"] = steward["enabled"]
                if "cadence" in steward:
                    steward_wire["cadence"] = steward["cadence"]
                if "model" in steward:
                    steward_wire["model"] = steward["model"]
                if "per_mob" in steward:
                    steward_wire["per_mob"] = steward["per_mob"]
                elif "perMob" in steward:
                    steward_wire["per_mob"] = steward["perMob"]
                if "runs_per_day" in steward:
                    steward_wire["runs_per_day"] = steward["runs_per_day"]
                elif "runsPerDay" in steward:
                    steward_wire["runs_per_day"] = steward["runsPerDay"]
                # Exposed because a hard-wired ceiling once left a
                # reasoning-model steward committing zero ops with no
                # reachable knob. A field the SDK will not forward is
                # unreachable no matter what Rust does.
                if "max_output_tokens" in steward:
                    steward_wire["max_output_tokens"] = steward["max_output_tokens"]
                elif "maxOutputTokens" in steward:
                    steward_wire["max_output_tokens"] = steward["maxOutputTokens"]
                if "min_signals" in steward:
                    steward_wire["min_signals"] = steward["min_signals"]
                elif "minSignals" in steward:
                    steward_wire["min_signals"] = steward["minSignals"]
                wire["steward"] = steward_wire
            else:
                wire["steward"] = steward
        hygienist = config.get("hygienist")
        if hygienist is not None:
            if hygienist is not False and not (
                isinstance(hygienist, dict) and hygienist.get("enabled") is False
            ):
                raise ValueError(
                    "agent_memory hygienist is PARKED and cannot be enabled; "
                    "remove it, use False, or use {'enabled': False}"
                )
            if isinstance(hygienist, dict):
                unknown = sorted(
                    set(hygienist)
                    - {
                        "enabled",
                        "runs_per_day",
                        "runsPerDay",
                        "model",
                        "max_output_tokens",
                        "maxOutputTokens",
                    }
                )
                if unknown:
                    raise ValueError(
                        "agent_memory hygienist got unsupported option(s): "
                        + ", ".join(unknown)
                    )
                hygienist_wire: dict[str, Any] = {}
                if "enabled" in hygienist:
                    hygienist_wire["enabled"] = hygienist["enabled"]
                if "max_output_tokens" in hygienist:
                    hygienist_wire["max_output_tokens"] = hygienist["max_output_tokens"]
                elif "maxOutputTokens" in hygienist:
                    hygienist_wire["max_output_tokens"] = hygienist["maxOutputTokens"]
                if "runs_per_day" in hygienist:
                    hygienist_wire["runs_per_day"] = hygienist["runs_per_day"]
                elif "runsPerDay" in hygienist:
                    hygienist_wire["runs_per_day"] = hygienist["runsPerDay"]
                if "model" in hygienist:
                    hygienist_wire["model"] = hygienist["model"]
                wire["hygienist"] = hygienist_wire
            else:
                wire["hygienist"] = hygienist
        self._config.agent_memory_config = wire
        return self

    def auth(self, config: Any) -> MetaBuilder:
        self._config.auth_config = config
        return self

    def console_fetch_timeout_ms(self, timeout_ms: int) -> MetaBuilder:
        if not isinstance(timeout_ms, int) or timeout_ms <= 0:
            raise ValueError("console_fetch_timeout_ms must be a positive integer")
        self._config.console_fetch_timeout_ms = timeout_ms
        return self

    def console_read_only(self, read_only: bool = True) -> MetaBuilder:
        self._config.console_read_only = bool(read_only)
        return self

    def console_auth_required(self, required: bool) -> MetaBuilder:
        """Set whether console routes require app authentication.

        Emitted as ``runtime_options.console_require_app_auth``. The gateway
        default is fail-closed: without ``.auth(...)`` the console trusts no
        signing key and refuses every request with 401. Pass ``False`` only
        for an explicitly open local or host-protected console. Mirrors the
        TypeScript builder's ``consoleAuthRequired``.
        """
        self._config.console_require_app_auth = bool(required)
        return self

    def console_config(self, path: str | Path) -> MetaBuilder:
        """Console UI TOML (``ConsoleUiConfig``) for the bundled console.

        Emitted as ``runtime_options.console_config_path`` and projected
        through ``/console/experience`` as ``console_config``. Mirrors the
        TypeScript builder's ``consoleConfig``.
        """
        self._config.console_config_path = str(path)
        return self

    def http_listen(self, listen: str) -> MetaBuilder:
        """Bind the gateway HTTP listener to ``listen`` (``HOST:PORT``).

        Emitted as ``runtime_options.http_listen``. The default is
        ``127.0.0.1:0``: loopback, ephemeral port, the only bind every earlier
        release had. A non-loopback address (``0.0.0.0:8080``) is refused at
        init unless the console enforces app auth (``.auth(...)``) or the
        launch acknowledges the exposure with ``.allow_remote()``; the gateway
        logs a WARN line for every non-loopback bind. IP literals only, no
        hostnames. The init result's ``http_base_url`` keeps the same-host
        form (``127.0.0.1`` for a wildcard bind). Mirrors the TypeScript
        builder's ``httpListen``.
        """
        if not isinstance(listen, str) or not listen.strip():
            raise ValueError("http_listen must be a non-empty HOST:PORT string")
        self._config.http_listen = listen.strip()
        return self

    def http_public_base_url(self, url: str) -> MetaBuilder:
        """Advertise the base URL clients reach the gateway at through a proxy.

        Emitted as ``runtime_options.http_public_base_url`` and reported back
        in the init result; the gateway never binds it. Read it after
        ``connect()`` as ``runtime.rust_http_public_base_url``. Mirrors the
        TypeScript builder's ``httpPublicBaseUrl``.
        """
        if not isinstance(url, str) or not url.strip():
            raise ValueError("http_public_base_url must be a non-empty URL string")
        self._config.http_public_base_url = url.strip()
        return self

    def allow_remote(self, allow: bool = True) -> MetaBuilder:
        """Acknowledge that ``http_listen`` exposes the listener beyond this host.

        Emitted as ``runtime_options.allow_remote``. Same word and rule as
        ``--allow-remote`` on ``rkat-rpc --tcp``: an exposure acknowledgement,
        not an auth mechanism. Pass it only with an authenticating proxy in
        front of the listener. Mirrors the TypeScript builder's ``allowRemote``.
        """
        self._config.allow_remote = bool(allow)
        return self

    def shutdown_in_flight_drain_ms(self, window_ms: int) -> MetaBuilder:
        """Let in-flight member turns finish on a graceful stop, for up to ``window_ms``.

        Emitted as the ``meta/init`` parameter ``shutdown_in_flight_drain_ms``.
        On this SDK's shutdown handshake or a SIGTERM, the gateway stops the
        mob with its members' current runs left running and waits for them,
        up to the window, before it shuts down. A turn still running at the
        bound is settled by the shutdown, as without a drain: a durable
        member's turn and its eligible queued input are kept for the member's
        next start, and any other turn is cancelled. The gateway's
        advertised shutdown horizon grows by the window plus 10 seconds for
        the drain's own Stop, and this transport waits that much longer on
        close. 0 (the default) drains nothing; the most is 300000 (5 minutes).
        Mirrors the TypeScript builder's ``shutdownInFlightDrainMs``.
        """
        if (
            not isinstance(window_ms, int)
            or isinstance(window_ms, bool)
            or not 0 <= window_ms <= 300_000
        ):
            raise ValueError(
                "shutdown_in_flight_drain_ms must be an integer of milliseconds "
                "in 0..=300000"
            )
        self._config.shutdown_in_flight_drain_ms = window_ms
        return self

    def implicit_delegate_idle_retirement(
        self, seconds: int | None
    ) -> MetaBuilder:
        """Configure idle auto-retirement for implicit delegation members.

        The default is owned by the gateway. Pass ``None`` to disable automatic
        retirement for runtimes that intentionally keep implicit delegates warm.
        """
        if seconds is not None and seconds < 0:
            raise ValueError(
                "implicit delegate idle retirement seconds must be non-negative or None"
            )
        self._config.implicit_delegate_idle_retire_secs = seconds
        self._config.implicit_delegate_idle_retire_configured = True
        return self

    def gateway(self, bin_path: str) -> MetaBuilder:
        self._config.gateway_bin = bin_path
        return self

    def gateway_init_timeout(self, seconds: float) -> MetaBuilder:
        """Cap how long ``connect()`` waits for the gateway to answer ``meta/init``.

        The answer is the gateway's ``accepted`` acknowledgement, or an older
        gateway's single final response. The default is 60 seconds, the
        transport's request timeout. A host whose gateway answers late (slow
        startup before init is accepted) can raise it: it bounds that wait,
        it does not make the startup faster. Running out raises
        ``InitOutcomeUnknownError`` and cleans the gateway up, as before.

        It applies to that one request only. Ordinary RPCs and provider
        callback deadlines keep their own timeouts, and the wait for an
        accepted init to settle is :meth:`init_deadline`.
        """
        if (
            isinstance(seconds, bool)
            or not isinstance(seconds, (int, float))
            or not math.isfinite(seconds)
            or seconds <= 0
        ):
            raise ValueError("gateway_init_timeout must be a positive finite number of seconds")
        self._config.gateway_init_timeout = float(seconds)
        return self

    def init_deadline(self, seconds: float | None) -> MetaBuilder:
        """Cap how long ``connect()`` waits for an accepted ``meta/init``.

        A gateway that accepted init keeps working through its startup
        phases (provider callbacks, restore, prewarm) and then settles ready
        or failed. By default the SDK waits for that settlement, the gateway
        exiting, or the reader failing. Set a deadline only when the host has
        its own startup budget: running out raises
        ``InitOutcomeUnknownError``, because the gateway may already have
        changed native state, and the gateway is asked to shut down.
        """
        if seconds is not None and (
            isinstance(seconds, bool)
            or not isinstance(seconds, (int, float))
            or not math.isfinite(seconds)
            or seconds <= 0
        ):
            raise ValueError("init_deadline must be a positive finite number of seconds or None")
        self._config.init_deadline = None if seconds is None else float(seconds)
        return self

    def modules(self, module_specs: list[dict[str, Any]]) -> MetaBuilder:
        self._config.modules = module_specs
        return self

    def persistent_state(self, path: str) -> MetaBuilder:
        """Enable persistent state at the given path.

        When set, the gateway creates SQLite-backed session/runtime state,
        canonical mob definition and events, Meta metadata, console logs,
        and binary blob storage under this directory. When not set, the
        gateway uses ephemeral stores.
        """
        self._config.persistent_state = path
        return self

    def declare_spec_update(self, expected_revision: int) -> MetaBuilder:
        """Apply this boot's mob definition as a one-shot revision-pinned update.

        The definition supplied by :meth:`mob` or :meth:`mob_inline` is compared
        and swapped against the canonical definition in :meth:`persistent_state`.
        ``expected_revision`` must be the revision observed before constructing
        the replacement definition. Exact replay converges; a stale revision or
        different content at the successor revision raises the typed
        ``StorageResolutionError`` during ``build()``.

        Use this only on the activation that is authorized to change the
        definition. Later ordinary restarts should omit the declaration.
        """
        if isinstance(expected_revision, bool) or not isinstance(expected_revision, int):
            raise TypeError("expected_revision must be a non-negative integer")
        if expected_revision < 0:
            raise ValueError("expected_revision must be a non-negative integer")
        self._config.declare_spec_update_expected_revision = expected_revision
        return self

    def role_migrations(
        self, declarations: Iterable[RoleMigrationDeclaration | dict[str, str]]
    ) -> MetaBuilder:
        """Authorize member role migrations for THIS boot only.

        A durable member whose role changed refuses to resume until it is named
        here. Pass the exact identity and the role it is migrating FROM::

            .role_migrations([
                RoleMigrationDeclaration(
                    identity="domain:automation", from_role="domain"
                )
            ])

        Never persisted: drop it from the next boot and the authority is gone.
        A malformed entry refuses the boot rather than silently arming nothing.
        """
        normalized = [
            declaration
            if isinstance(declaration, RoleMigrationDeclaration)
            else RoleMigrationDeclaration.from_dict(declaration)
            for declaration in declarations
        ]
        seen: dict[str, str] = {}
        for declaration in normalized:
            existing = seen.get(declaration.identity)
            if existing is not None and existing != declaration.from_role:
                # Two conflicting declarations for one identity have no
                # defensible resolution, and the gateway would otherwise let
                # dict order decide which predecessor role becomes migration
                # authority. An identical repeat is unambiguous and allowed.
                raise ValueError(
                    f"role_migrations declares {declaration.identity!r} twice with "
                    f"different predecessor roles ({existing!r} and "
                    f"{declaration.from_role!r}); migration authority cannot be "
                    "resolved by order"
                )
            seen[declaration.identity] = declaration.from_role
        self._config.role_migrations = [
            declaration.to_dict() for declaration in normalized
        ]
        return self

    def application_tool_policies(
        self, policies: Iterable[str | bytes]
    ) -> MetaBuilder:
        """Serve compiled application tool policies to governed members.

        Each entry is the EXACT canonical JSON of one compiled policy, as
        produced by your policy compiler::

            .application_tool_policies([compiled_team_tools_bytes])

        Pass the compiler's bytes through unchanged. Parsing verifies the
        policy digest against the bytes it is given, so re-serialising the
        JSON here would have Meta check a digest against bytes it had just
        produced itself rather than the ones you compiled.

        The provider identity comes from the artifact: each policy declares its
        own author, and members bind to it by that carried id. Meta registers
        one provider per distinct author and names none itself.

        A malformed entry, or one whose digest does not match its bytes,
        refuses the boot rather than silently arming nothing.
        """
        normalized: list[str] = []
        for index, policy in enumerate(policies):
            if isinstance(policy, bytes):
                try:
                    decoded = policy.decode("utf-8")
                except UnicodeDecodeError as error:
                    raise ValueError(
                        f"application_tool_policies[{index}] is not valid UTF-8: {error}"
                    ) from error
            elif isinstance(policy, str):
                decoded = policy
            else:
                raise TypeError(
                    f"application_tool_policies[{index}] must be str or bytes, "
                    f"got {type(policy).__name__}"
                )
            if not decoded.strip():
                # An empty entry cannot be a compiled policy, and passing it on
                # would surface as a parse error at boot with nothing pointing
                # at which entry was blank.
                raise ValueError(
                    f"application_tool_policies[{index}] is empty; each entry must be "
                    "the canonical JSON of one compiled policy"
                )
            normalized.append(decoded)
        self._config.application_tool_policies = normalized
        return self

    def child_application_tool_policy(
        self, binding: Mapping[str, str]
    ) -> MetaBuilder:
        """Choose the application tool policy child mob members are built with.

        Child mobs are the mobs a governed member creates with ``mob_create``
        and the implicit mob its ``delegate`` helpers run in. ``binding`` is an
        ``ApplicationToolPolicyBinding``::

            .child_application_tool_policy(
                {"kind": "provider", "provider_id": "example", "policy_id": "child-tools"}
            )

        or the explicit opt-out ``{"kind": "unmanaged"}``, which lets child
        members run without the host's tool policy.

        Once ``application_tool_policies`` are served, Meerkat refuses
        ``mob_create`` and ``delegate`` with a typed tool error
        (``child_tool_policy_required``) until a child policy is chosen; there
        is no implicit default. Same-mob ``fork_off``, ``mob_spawn_member`` and
        councils are not affected. The gateway refuses the boot when a provider
        binding names a provider or policy no served policy carries.
        """
        if not isinstance(binding, Mapping):
            raise TypeError(
                "child_application_tool_policy must be a mapping, "
                f"got {type(binding).__name__}"
            )
        kind = binding.get("kind")
        if kind == "unmanaged":
            if set(binding) != {"kind"}:
                raise ValueError(
                    "an unmanaged child_application_tool_policy takes no other keys"
                )
        elif kind == "provider":
            if set(binding) != {"kind", "provider_id", "policy_id"}:
                raise ValueError(
                    "a provider child_application_tool_policy needs exactly "
                    "kind, provider_id and policy_id"
                )
            for key in ("provider_id", "policy_id"):
                value = binding[key]
                if not isinstance(value, str) or not value.strip():
                    raise ValueError(
                        f"child_application_tool_policy {key} must be a non-empty string"
                    )
        else:
            raise ValueError(
                "child_application_tool_policy kind must be 'provider' or 'unmanaged' "
                f"(inherit is not valid for child mobs), got {kind!r}"
            )
        self._config.child_application_tool_policy = dict(binding)
        return self

    def roster(self, provider: Any) -> MetaBuilder:
        """Set the identity-first roster provider."""
        self._config.roster_provider = provider
        return self

    def topology_provider(self, provider: Any) -> MetaBuilder:
        """Set the identity-first topology provider."""
        self._config.topology_provider = provider
        return self

    def agent_customizer(self, customizer: Any) -> MetaBuilder:
        """Set the identity-first agent customizer."""
        self._config.agent_customizer = customizer
        return self

    def identity_bootstrap_mode(
        self, mode: IdentityBootstrapMode
    ) -> MetaBuilder:
        """Set the identity materialization policy used during gateway init.

        Calling this method declares identity-first intent and therefore
        requires ``roster(...)``, including for explicit eager mode. The option
        is omitted unless this method is called, preserving the classic gateway
        when no roster is configured and the eager default when one is.
        """
        if not isinstance(mode, IdentityBootstrapMode):
            raise TypeError("mode must be an IdentityBootstrapMode")
        self._config.identity_bootstrap_mode = mode
        return self

    async def build(self) -> MetaRuntime:
        """Validate the configuration and return a connected runtime.

        A failed startup returns no runtime. When the gateway transport's stop
        then fails too, ``GatewayStartupCleanupError.cleanup`` reaches the
        runtime that keeps that transport; for a cancelled or interrupted
        startup, the propagating exception carries it as ``gateway_cleanup``
        (see ``MetaRuntime.connect()``).
        """
        self._validate()
        self._apply_convention_defaults()
        from .runtime import MetaRuntime
        return await MetaRuntime._create(self._config)

    def _validate(self) -> None:
        if self._config.mob_config_path and self._config.mob_config_inline:
            raise ValueError(
                "mob() and mob_inline() are mutually exclusive"
            )
        if self._config.prepared_application_bundle is not None and (
            self._config.mob_config_path or self._config.mob_config_inline
        ):
            raise ValueError(
                "prepared_application() is mutually exclusive with mob() and "
                "mob_inline()"
            )
        if self._config.declare_spec_update_expected_revision is not None:
            if not (
                self._config.mob_config_path
                or self._config.mob_config_inline
                or self._config.prepared_application_bundle is not None
            ):
                raise ValueError(
                    "declare_spec_update() requires mob(), mob_inline() or "
                    "prepared_application() to supply the replacement definition"
                )
            if not self._config.persistent_state:
                raise ValueError(
                    "declare_spec_update() requires persistent_state() because "
                    "ephemeral mob storage has no durable definition to update"
                )
        if (
            self._config.identity_bootstrap_mode is not None
            and self._config.roster_provider is None
        ):
            raise ValueError(
                "identity_bootstrap_mode() requires roster() because every "
                "explicit bootstrap mode is identity-first"
            )

    def _apply_convention_defaults(self) -> None:
        """Fill in conventional config paths when not explicitly set.

        Convention (relative to cwd):
        - config/gating.toml → gating config
        - config/access.toml → ABAC access control config
        - deployment/routing.toml → routing config

        Only checks when the corresponding builder method was NOT called.
        Explicit paths always win. Files that don't exist are skipped. For
        config/access.toml and config/gating.toml only a "not found" lookup
        (including a missing or dangling parent) is skipped: any other entry,
        or any other lookup error, is forwarded to the gateway's strict loader
        instead of access control or gating silently staying off.
        """
        if self._config.gating_config_path is None:
            candidate = Path("config/gating.toml")
            if _entry_present(candidate):
                self._config.gating_config_path = str(candidate)

        if self._config.access_config_path is None:
            candidate = Path("config/access.toml")
            if _entry_present(candidate):
                self._config.access_config_path = str(candidate)

        if self._config.routing_config_path is None:
            candidate = Path("deployment/routing.toml")
            if candidate.is_file():
                self._config.routing_config_path = str(candidate)


def _entry_present(path: Path) -> bool:
    """Whether the lookup of ``path`` finds an entry or fails other than "not found".

    Discovery for a file whose presence opts into a security control: only
    ``FileNotFoundError`` (nothing there, including a missing or dangling
    parent) means "off". A directory, a dangling final link,
    ``NotADirectoryError`` or any other ``OSError`` is reported so the strict
    loader handles it.
    """
    try:
        path.lstat()
    except FileNotFoundError:
        return False
    except OSError:
        return True
    return True


class Meta:
    @staticmethod
    def builder() -> MetaBuilder:
        return MetaBuilder()
