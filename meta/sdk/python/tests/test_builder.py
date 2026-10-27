"""Tests for builder chain."""
import os
from pathlib import Path

import pytest
from meerkat_meta import (
    ExperimentalLiveGatewayConfig,
    OpenAiLiveGatewayConfig,
    ExperimentalLiveExecutionProfileConfig,
    IdentityBootstrapMode,
    LiveAuthBindingRef,
)
from meerkat_meta.builder import Meta, MetaBuilder
from meerkat_meta.runtime import MetaRuntime


class TestBuilderChain:
    def test_builder_returns_builder(self):
        b = Meta.builder()
        assert isinstance(b, MetaBuilder)

    def test_mob_returns_builder(self):
        b = Meta.builder().mob("config/mob.toml")
        assert isinstance(b, MetaBuilder)

    def test_mob_sets_config_path(self):
        b = Meta.builder().mob("config/mob.toml")
        assert b._config.mob_config_path == "config/mob.toml"

    def test_prepared_application_is_carried_opaquely(self):
        b = Meta.builder().prepared_application(b"\x00prepared\xff", "sha256:abc")
        b._validate()

        params = MetaRuntime(b._config)._build_init_params()

        assert params["prepared_application"] == {
            "bundle_base64": "AHByZXBhcmVk/w==",
            "digest": "sha256:abc",
        }
        assert "mob_config" not in params

    def test_prepared_application_excludes_mob_toml(self):
        for b in (
            Meta.builder().mob("config/mob.toml"),
            Meta.builder().mob_inline("[mob]\nid = 'home'\n"),
        ):
            b.prepared_application(b"bundle", "sha256:abc")
            with pytest.raises(ValueError, match="mutually exclusive"):
                b._validate()

    def test_prepared_application_rejects_a_malformed_carrier(self):
        with pytest.raises(TypeError):
            Meta.builder().prepared_application("not-bytes", "sha256:abc")
        with pytest.raises(ValueError):
            Meta.builder().prepared_application(b"bundle", "")

    def test_prepared_application_supplies_a_declared_update(self):
        b = (
            Meta.builder()
            .prepared_application(b"bundle", "sha256:abc")
            .persistent_state("/var/lib/example-app")
            .declare_spec_update(7)
        )
        b._validate()

    def test_definition_update_defaults_to_omitted(self):
        params = MetaRuntime(Meta.builder()._config)._build_init_params()

        assert "declare_spec_update" not in params["runtime_options"]
        assert "mob_composition" not in params["runtime_options"]

    def test_definition_update_reaches_authoritative_runtime_options(self):
        b = (
            Meta.builder()
            .mob_inline("[mob]\nid = 'home'\n")
            .persistent_state("/var/lib/example")
            .declare_spec_update(7)
        )
        b._validate()

        params = MetaRuntime(b._config)._build_init_params()

        assert params["mob_config"] == "[mob]\nid = 'home'\n"
        assert params["persistent_state"] == "/var/lib/example"
        assert params["runtime_options"]["mob_composition"] == {
            "authority": "authoritative"
        }
        assert params["runtime_options"]["declare_spec_update"] == {
            "expected_revision": 7
        }

    @pytest.mark.parametrize("revision", [-1, -7])
    def test_definition_update_rejects_negative_revision(self, revision):
        with pytest.raises(ValueError, match="non-negative integer"):
            Meta.builder().declare_spec_update(revision)

    @pytest.mark.parametrize("revision", [True, 1.5, "7", None])
    def test_definition_update_rejects_non_integer_revision(self, revision):
        with pytest.raises(TypeError, match="non-negative integer"):
            Meta.builder().declare_spec_update(revision)

    def test_definition_update_requires_replacement_definition(self):
        b = (
            Meta.builder()
            .persistent_state("/var/lib/example")
            .declare_spec_update(1)
        )

        with pytest.raises(ValueError, match=r"requires mob\(\), mob_inline\(\) or prepared_application\(\)"):
            b._validate()

    def test_definition_update_requires_durable_state(self):
        b = (
            Meta.builder()
            .mob_inline("[mob]\nid = 'home'\n")
            .declare_spec_update(1)
        )

        with pytest.raises(ValueError, match=r"requires persistent_state\(\)"):
            b._validate()

    def test_identity_bootstrap_mode_defaults_to_omitted(self):
        b = Meta.builder()
        params = MetaRuntime(b._config)._build_init_params()

        assert "identity_bootstrap_mode" not in params["runtime_options"]

    def test_experimental_live_defaults_to_omitted(self):
        b = Meta.builder()
        params = MetaRuntime(b._config)._build_init_params()

        assert "experimental_live" not in params["runtime_options"]
        assert "openai_live" not in params["runtime_options"]
        assert "console_voice" not in params["runtime_options"]

    def test_console_voice_reaches_http_runtime_option_without_enabling_stdio_live(self):
        config = OpenAiLiveGatewayConfig(
            principal="alice@example.com",
            realm="ops",
            auth_binding=LiveAuthBindingRef(realm="ops", binding="openai"),
            voice="marin",
        )
        builder = Meta.builder()
        assert builder.console_voice(config) is builder
        params = MetaRuntime(builder._config)._build_init_params()
        assert params["runtime_options"]["console_voice"] == config.to_dict()
        assert "openai_live" not in params["runtime_options"]
        assert "experimental_live" not in params["runtime_options"]
        with pytest.raises(TypeError, match="OpenAiLiveGatewayConfig"):
            Meta.builder().console_voice({"principal": "alice@example.com"})

    def test_openai_live_reaches_public_runtime_option(self):
        b = Meta.builder().openai_live(
            OpenAiLiveGatewayConfig(
                principal="user:alex",
                realm="example",
                auth_binding=LiveAuthBindingRef(
                    realm="example", binding="openai-api-key", profile="alex"
                ),
                voice="marin",
                session_instructions="You are a test voice agent.",
            )
        )
        params = MetaRuntime(b._config)._build_init_params()

        assert params["runtime_options"]["openai_live"] == {
            "principal": "user:alex",
            "realm": "example",
            "auth_binding": {
                "realm": "example",
                "binding": "openai-api-key",
                "profile": "alex",
            },
            "voice": "marin",
            "session_instructions": "You are a test voice agent.",
        }
        assert "experimental_live" not in params["runtime_options"]
        with pytest.raises(TypeError, match="OpenAiLiveGatewayConfig"):
            Meta.builder().openai_live({"principal": "user:alex"})

    def test_experimental_live_reaches_strict_runtime_option(self):
        b = Meta.builder().experimental_live(
            ExperimentalLiveGatewayConfig(
                principal="user:alex",
                realm="example",
                factory_kind="openai-gpt-live",
                factory_version="v1",
                gate0_qualification="gate0-v1",
                auth_binding=LiveAuthBindingRef(
                    realm="example", binding="chatgpt-oauth", profile="alex"
                ),
                voice="marin",
                execution_profiles=(
                    ExperimentalLiveExecutionProfileConfig(
                        profile_id="example.device.open-room.v1",
                        session_instructions="You are a test voice agent.",
                    ),
                ),
            )
        )
        params = MetaRuntime(b._config)._build_init_params()

        assert params["runtime_options"]["experimental_live"] == {
            "principal": "user:alex",
            "realm": "example",
            "factory_kind": "openai-gpt-live",
            "factory_version": "v1",
            "gate0_qualification": "gate0-v1",
            "auth_binding": {
                "realm": "example",
                "binding": "chatgpt-oauth",
                "profile": "alex",
            },
            "voice": "marin",
            "execution_profiles": [
                {
                    "profile_id": "example.device.open-room.v1",
                    "session_instructions": "You are a test voice agent.",
                }
            ],
        }

    @pytest.mark.parametrize(
        ("mode", "expected"),
        [
            (
                IdentityBootstrapMode.eager_materialize(),
                {"mode": "eager_materialize"},
            ),
            (
                IdentityBootstrapMode.lazy_materialize(),
                {"mode": "lazy_materialize"},
            ),
            (
                IdentityBootstrapMode.lazy_with_background_warm(concurrency=2),
                {"mode": "lazy_with_background_warm", "concurrency": 2},
            ),
        ],
    )
    def test_identity_bootstrap_mode_reaches_strict_runtime_option(
        self, mode, expected
    ):
        b = Meta.builder().identity_bootstrap_mode(mode)
        params = MetaRuntime(b._config)._build_init_params()

        assert params["runtime_options"]["identity_bootstrap_mode"] == expected

    @pytest.mark.parametrize("concurrency", [0, -1, True, 1.5, None])
    def test_background_warm_rejects_invalid_concurrency(self, concurrency):
        with pytest.raises(ValueError, match="positive integer concurrency"):
            IdentityBootstrapMode.lazy_with_background_warm(
                concurrency=concurrency
            )

    def test_non_background_mode_rejects_concurrency(self):
        with pytest.raises(ValueError, match="does not accept concurrency"):
            IdentityBootstrapMode(mode="lazy_materialize", concurrency=2)

    def test_background_warm_rejects_concurrency_above_gateway_cap(self):
        with pytest.raises(ValueError, match="at most 16"):
            IdentityBootstrapMode.lazy_with_background_warm(concurrency=17)

    def test_identity_bootstrap_mode_rejects_untyped_values(self):
        with pytest.raises(TypeError, match="IdentityBootstrapMode"):
            Meta.builder().identity_bootstrap_mode(
                {"mode": "lazy_materialize"}
            )

    @pytest.mark.parametrize(
        "mode",
        [
            IdentityBootstrapMode.eager_materialize(),
            IdentityBootstrapMode.lazy_materialize(),
            IdentityBootstrapMode.lazy_with_background_warm(concurrency=2),
        ],
    )
    def test_every_explicit_identity_bootstrap_mode_requires_roster(self, mode):
        builder = Meta.builder().identity_bootstrap_mode(mode)

        with pytest.raises(ValueError, match=r"identity_bootstrap_mode.*roster"):
            builder._validate()

        builder.roster(object())._validate()

    def test_implicit_delegate_idle_retirement_sets_runtime_option(self):
        b = Meta.builder().implicit_delegate_idle_retirement(30)
        params = MetaRuntime(b._config)._build_init_params()

        assert params["runtime_options"]["implicit_delegate_idle_retire_secs"] == 30

    def test_implicit_delegate_idle_retirement_can_be_disabled(self):
        b = Meta.builder().implicit_delegate_idle_retirement(None)
        params = MetaRuntime(b._config)._build_init_params()

        assert params["runtime_options"]["implicit_delegate_idle_retire_secs"] is None

    def test_implicit_delegate_idle_retirement_rejects_negative_seconds(self):
        with pytest.raises(ValueError):
            Meta.builder().implicit_delegate_idle_retirement(-1)

    def test_console_fetch_timeout_ms_sets_runtime_option(self):
        b = Meta.builder().console_fetch_timeout_ms(120_000)
        params = MetaRuntime(b._config)._build_init_params()

        assert params["runtime_options"]["console_fetch_timeout_ms"] == 120_000

    def test_console_read_only_sets_runtime_option(self):
        b = Meta.builder().console_read_only()
        params = MetaRuntime(b._config)._build_init_params()

        assert params["runtime_options"]["console_read_only"] is True

    def test_console_auth_defaults_to_omitted(self):
        params = MetaRuntime(Meta.builder()._config)._build_init_params()

        assert "console_require_app_auth" not in params["runtime_options"]
        assert "console_config_path" not in params["runtime_options"]

    def test_console_auth_required_false_sets_runtime_option(self):
        b = Meta.builder().console_auth_required(False)
        params = MetaRuntime(b._config)._build_init_params()

        assert params["runtime_options"]["console_require_app_auth"] is False

    def test_console_auth_required_true_sets_runtime_option(self):
        b = Meta.builder().console_auth_required(True)
        params = MetaRuntime(b._config)._build_init_params()

        assert params["runtime_options"]["console_require_app_auth"] is True

    def test_console_config_sets_runtime_option(self, tmp_path):
        console_toml = tmp_path / "console.toml"
        b = Meta.builder().console_config(console_toml)
        params = MetaRuntime(b._config)._build_init_params()

        assert params["runtime_options"]["console_config_path"] == str(console_toml)

    def test_http_exposure_defaults_to_omitted(self):
        params = MetaRuntime(Meta.builder()._config)._build_init_params()

        assert "http_listen" not in params["runtime_options"]
        assert "http_public_base_url" not in params["runtime_options"]
        assert "allow_remote" not in params["runtime_options"]

    def test_http_listen_sets_runtime_option(self):
        b = Meta.builder().http_listen(" 0.0.0.0:8080 ")
        params = MetaRuntime(b._config)._build_init_params()

        assert params["runtime_options"]["http_listen"] == "0.0.0.0:8080"

    def test_http_listen_rejects_empty_value(self):
        with pytest.raises(ValueError):
            Meta.builder().http_listen("  ")

    def test_http_public_base_url_sets_runtime_option(self):
        b = Meta.builder().http_public_base_url("https://mob.example.com")
        params = MetaRuntime(b._config)._build_init_params()

        assert (
            params["runtime_options"]["http_public_base_url"]
            == "https://mob.example.com"
        )

    def test_allow_remote_sets_runtime_option(self):
        b = Meta.builder().allow_remote()
        params = MetaRuntime(b._config)._build_init_params()

        assert params["runtime_options"]["allow_remote"] is True

    def test_allow_remote_false_is_transmitted_not_dropped(self):
        b = Meta.builder().allow_remote(False)
        params = MetaRuntime(b._config)._build_init_params()

        assert params["runtime_options"]["allow_remote"] is False

    def test_shutdown_in_flight_drain_defaults_to_omitted(self):
        params = MetaRuntime(Meta.builder()._config)._build_init_params()

        assert "shutdown_in_flight_drain_ms" not in params

    def test_shutdown_in_flight_drain_ms_sets_top_level_init_param(self):
        b = Meta.builder().shutdown_in_flight_drain_ms(60_000)
        params = MetaRuntime(b._config)._build_init_params()

        assert params["shutdown_in_flight_drain_ms"] == 60_000
        assert "shutdown_in_flight_drain_ms" not in params["runtime_options"]

    @pytest.mark.parametrize("window_ms", [0, 300_000])
    def test_shutdown_in_flight_drain_ms_sends_its_bounds(self, window_ms):
        b = Meta.builder().shutdown_in_flight_drain_ms(window_ms)
        params = MetaRuntime(b._config)._build_init_params()

        assert params["shutdown_in_flight_drain_ms"] == window_ms

    @pytest.mark.parametrize("window_ms", [-1, 300_001, 1.5, True, "60000"])
    def test_shutdown_in_flight_drain_ms_rejects_values_the_gateway_refuses(
        self, window_ms
    ):
        with pytest.raises(ValueError, match="in 0..=300000"):
            Meta.builder().shutdown_in_flight_drain_ms(window_ms)

    def test_meerkat_config_defaults_to_omitted(self):
        params = MetaRuntime(Meta.builder()._config)._build_init_params()

        assert "meerkat_config_path" not in params["runtime_options"]

    def test_meerkat_config_sets_runtime_option(self, tmp_path):
        config_toml = tmp_path / ".rkat" / "config.toml"
        b = Meta.builder().meerkat_config(config_toml)
        params = MetaRuntime(b._config)._build_init_params()

        assert params["runtime_options"]["meerkat_config_path"] == str(config_toml)

    def test_meerkat_config_rejects_empty_path(self):
        with pytest.raises(ValueError, match="must not be empty"):
            Meta.builder().meerkat_config("  ")

    def test_provider_credentials_root_defaults_to_omitted(self):
        params = MetaRuntime(Meta.builder()._config)._build_init_params()

        assert "provider_credentials_root" not in params["runtime_options"]

    def test_provider_credentials_root_sets_runtime_option(self, tmp_path):
        root = tmp_path / "users" / "a" / "credentials"
        b = Meta.builder().provider_credentials_root(root)
        params = MetaRuntime(b._config)._build_init_params()

        assert params["runtime_options"]["provider_credentials_root"] == str(root)

    def test_provider_credentials_root_rejects_empty_path(self):
        with pytest.raises(ValueError, match="must not be empty"):
            Meta.builder().provider_credentials_root("  ")

    def test_workgraph_defaults_to_omitted(self):
        b = Meta.builder()
        params = MetaRuntime(b._config)._build_init_params()

        assert "workgraph" not in params["runtime_options"]

    def test_workgraph_enabled_sets_runtime_option(self):
        b = Meta.builder().workgraph()
        params = MetaRuntime(b._config)._build_init_params()

        assert params["runtime_options"]["workgraph"] is True

    def test_workgraph_disabled_sets_runtime_option(self):
        b = Meta.builder().workgraph(False)
        params = MetaRuntime(b._config)._build_init_params()

        assert params["runtime_options"]["workgraph"] is False

    def test_workgraph_accepts_durable_store_directory_string(self):
        b = Meta.builder().workgraph("/var/lib/mob/workgraph")
        params = MetaRuntime(b._config)._build_init_params()

        assert params["runtime_options"]["workgraph"] == "/var/lib/mob/workgraph"

    def test_fork_instruction_policy_defaults_to_omitted(self):
        b = Meta.builder()
        params = MetaRuntime(b._config)._build_init_params()

        assert "fork_instruction_policy" not in params["runtime_options"]

    def test_fork_instruction_policy_sets_runtime_option(self):
        b = Meta.builder().fork_instruction_policy("reactivate_inherited")
        params = MetaRuntime(b._config)._build_init_params()

        assert params["runtime_options"]["fork_instruction_policy"] == "reactivate_inherited"

    def test_fork_instruction_policy_rejects_unknown_names(self):
        with pytest.raises(ValueError):
            Meta.builder().fork_instruction_policy("inherit")

    def test_console_fetch_timeout_ms_rejects_non_positive_values(self):
        with pytest.raises(ValueError):
            Meta.builder().console_fetch_timeout_ms(0)

    def test_memory_stores_without_gateway_config_is_rejected(self):
        with pytest.raises(ValueError, match="not supported by the Rust gateway"):
            Meta.builder().memory(stores=["main"])

    def test_agent_memory_default_reaches_runtime_options(self):
        b = Meta.builder().agent_memory()
        params = MetaRuntime(b._config)._build_init_params()

        assert params["runtime_options"]["agent_memory"] is True

    def test_agent_memory_options_serialize_to_gateway_wire_keys(self):
        b = Meta.builder().agent_memory(
            realm="example",
            selection="contextual",
            max_entries=3,
            recall_timeout_ms=1200,
            recall_failure_policy="fail",
            instruction_header="Remember",
        )
        params = MetaRuntime(b._config)._build_init_params()

        assert params["runtime_options"]["agent_memory"] == {
            "realm": "example",
            "selection": "contextual",
            "max_entries": 3,
            "recall_timeout_ms": 1200,
            "recall_failure_policy": "fail",
            "instruction_header": "Remember",
        }

    def test_agent_memory_disable_reaches_runtime_options(self):
        b = Meta.builder().agent_memory(False)
        params = MetaRuntime(b._config)._build_init_params()

        assert params["runtime_options"]["agent_memory"] == {"enabled": False}

    def test_agent_memory_taint_knobs_serialize_to_gateway_wire_keys(self):
        b = Meta.builder().agent_memory(
            llm_writes="quarantined",
            recorder_tool=False,
            content_trust={
                "trusted_mcp_servers": ["knowledge_graph"],
                "untrusted_tools": ["scrape_page"],
            },
        )
        params = MetaRuntime(b._config)._build_init_params()

        assert params["runtime_options"]["agent_memory"] == {
            "llm_writes": "quarantined",
            "recorder_tool": False,
            "content_trust": {
                "trusted_mcp_servers": ["knowledge_graph"],
                "untrusted_tools": ["scrape_page"],
            },
        }

    def test_agent_memory_taint_knobs_accept_camel_case(self):
        b = Meta.builder().agent_memory(
            llmWrites="quarantined",
            recorderTool=True,
            contentTrust={"trusted_tools": ["safe_calc"]},
        )
        params = MetaRuntime(b._config)._build_init_params()

        assert params["runtime_options"]["agent_memory"] == {
            "llm_writes": "quarantined",
            "recorder_tool": True,
            "content_trust": {"trusted_tools": ["safe_calc"]},
        }

    def test_agent_memory_selector_keeps_only_off_compatibility(self):
        b = Meta.builder().agent_memory(selector="off")
        params = MetaRuntime(b._config)._build_init_params()

        assert params["runtime_options"]["agent_memory"] == {
            "selector": "off",
        }
        with pytest.raises(ValueError, match="selector is RETIRED"):
            Meta.builder().agent_memory(selector="profile:/tmp/selector.toml")

    def test_agent_memory_distiller_serializes_to_gateway_wire_keys(self):
        b = Meta.builder().agent_memory(
            distiller={
                "enabled": True,
                "runs_per_hour": 6,
                "min_interactions": 5,
                "model": "claude-haiku-4-5",
            },
        )
        params = MetaRuntime(b._config)._build_init_params()

        assert params["runtime_options"]["agent_memory"] == {
            "distiller": {
                "enabled": True,
                "runs_per_hour": 6,
                "min_interactions": 5,
                "model": "claude-haiku-4-5",
            },
        }

    def test_agent_memory_distiller_accepts_camel_case_and_bool(self):
        b = Meta.builder().agent_memory(
            distiller={"runsPerHour": 3, "minInteractions": 2},
        )
        params = MetaRuntime(b._config)._build_init_params()
        assert params["runtime_options"]["agent_memory"] == {
            "distiller": {"runs_per_hour": 3, "min_interactions": 2},
        }

        b = Meta.builder().agent_memory(distiller=True)
        params = MetaRuntime(b._config)._build_init_params()
        assert params["runtime_options"]["agent_memory"] == {"distiller": True}

    def test_agent_memory_steward_serializes_to_gateway_wire_keys(self):
        b = Meta.builder().agent_memory(
            steward={
                "enabled": True,
                "cadence": "*/6h",
                "model": "claude-sonnet-4-6",
                "per_mob": False,
                "runs_per_day": 4,
                "min_signals": 3,
            },
        )
        params = MetaRuntime(b._config)._build_init_params()

        assert params["runtime_options"]["agent_memory"] == {
            "steward": {
                "enabled": True,
                "cadence": "*/6h",
                "model": "claude-sonnet-4-6",
                "per_mob": False,
                "runs_per_day": 4,
                "min_signals": 3,
            },
        }

    def test_agent_memory_steward_accepts_camel_case_and_bool(self):
        b = Meta.builder().agent_memory(
            steward={"runsPerDay": 2, "minSignals": 5, "perMob": True},
        )
        params = MetaRuntime(b._config)._build_init_params()
        assert params["runtime_options"]["agent_memory"] == {
            "steward": {"per_mob": True, "runs_per_day": 2, "min_signals": 5},
        }

        b = Meta.builder().agent_memory(steward=True)
        params = MetaRuntime(b._config)._build_init_params()
        assert params["runtime_options"]["agent_memory"] == {"steward": True}

    def test_agent_memory_operator_scope_serializes_to_gateway_wire_key(self):
        b = Meta.builder().agent_memory(store="sqlite", operator_scope="provisional")
        params = MetaRuntime(b._config)._build_init_params()
        assert params["runtime_options"]["agent_memory"] == {
            "store": "sqlite",
            "operator_scope": "provisional",
        }

    def test_agent_memory_operator_scope_accepts_camel_case(self):
        b = Meta.builder().agent_memory(operatorScope="off")
        params = MetaRuntime(b._config)._build_init_params()
        assert params["runtime_options"]["agent_memory"] == {"operator_scope": "off"}

    def test_agent_memory_hygienist_keeps_only_disabled_compatibility(self):
        b = Meta.builder().agent_memory(
            hygienist={
                "enabled": False,
                "runs_per_day": 3,
                "model": "legacy-model",
                "max_output_tokens": 8192,
            },
        )
        params = MetaRuntime(b._config)._build_init_params()
        assert params["runtime_options"]["agent_memory"] == {
            "hygienist": {
                "enabled": False,
                "runs_per_day": 3,
                "model": "legacy-model",
                "max_output_tokens": 8192,
            },
        }

    def test_agent_memory_hygienist_accepts_disabled_camel_case_and_bool(self):
        b = Meta.builder().agent_memory(
            hygienist={"enabled": False, "runsPerDay": 2}
        )
        params = MetaRuntime(b._config)._build_init_params()
        assert params["runtime_options"]["agent_memory"] == {
            "hygienist": {"enabled": False, "runs_per_day": 2},
        }

        b = Meta.builder().agent_memory(hygienist=False)
        params = MetaRuntime(b._config)._build_init_params()
        assert params["runtime_options"]["agent_memory"] == {"hygienist": False}

        with pytest.raises(ValueError, match="hygienist is PARKED"):
            Meta.builder().agent_memory(hygienist=True)
        with pytest.raises(ValueError, match="hygienist is PARKED"):
            Meta.builder().agent_memory(hygienist={})

    def test_agent_memory_unknown_option_raises_instead_of_silently_dropping(self):
        with pytest.raises(ValueError, match="per_turn_injecton"):
            Meta.builder().agent_memory(per_turn_injecton="budgeted")

    def test_agent_memory_unknown_nested_option_raises(self):
        with pytest.raises(ValueError, match="distiller.*runsperhour_typo"):
            Meta.builder().agent_memory(
                distiller={"runs_per_hour": 2, "runsperhour_typo": 9},
            )
        with pytest.raises(ValueError, match="steward.*cadance"):
            Meta.builder().agent_memory(steward={"cadance": "*/6h"})
        with pytest.raises(ValueError, match="hygienist.*runs_per_dya"):
            Meta.builder().agent_memory(
                hygienist={"enabled": False, "runs_per_dya": 2}
            )


class TestConventionDefaults:
    def test_gating_discovered(self, tmp_path, monkeypatch):
        monkeypatch.chdir(tmp_path)
        (tmp_path / "config").mkdir()
        (tmp_path / "config" / "gating.toml").write_text("[[rules]]")

        b = Meta.builder().mob("config/mob.toml")
        b._apply_convention_defaults()
        assert b._config.gating_config_path == "config/gating.toml"

    def test_access_discovered(self, tmp_path, monkeypatch):
        monkeypatch.chdir(tmp_path)
        (tmp_path / "config").mkdir()
        (tmp_path / "config" / "access.toml").write_text("enabled = false")

        b = Meta.builder().mob("config/mob.toml")
        b._apply_convention_defaults()
        assert b._config.access_config_path == "config/access.toml"

    def test_access_directory_is_forwarded_for_refusal(self, tmp_path, monkeypatch):
        # Only a genuinely absent config/access.toml leaves access control
        # off; a directory there is forwarded so the gateway refuses it.
        monkeypatch.chdir(tmp_path)
        (tmp_path / "config" / "access.toml").mkdir(parents=True)

        b = Meta.builder().mob("config/mob.toml")
        b._apply_convention_defaults()
        assert b._config.access_config_path == "config/access.toml"

    @pytest.mark.skipif(os.name != "posix", reason="symlinks and ENOTDIR are POSIX fixtures")
    def test_dangling_or_unobservable_access_entry_is_forwarded_for_refusal(
        self, tmp_path, monkeypatch
    ):
        monkeypatch.chdir(tmp_path)
        (tmp_path / "config").mkdir()
        (tmp_path / "config" / "access.toml").symlink_to(tmp_path / "missing.toml")

        dangling = Meta.builder().mob("config/mob.toml")
        dangling._apply_convention_defaults()
        assert dangling._config.access_config_path == "config/access.toml"

        # A lookup that fails other than by absence (config is a file, so it
        # reports "not a directory") is forwarded too.
        (tmp_path / "config" / "access.toml").unlink()
        (tmp_path / "config").rmdir()
        (tmp_path / "config").write_text("")

        unobservable = Meta.builder().mob("config/mob.toml")
        unobservable._apply_convention_defaults()
        assert unobservable._config.access_config_path == "config/access.toml"

    def test_gating_directory_is_forwarded_for_refusal(self, tmp_path, monkeypatch):
        # Like access.toml, only a "not found" config/gating.toml leaves gating
        # off; a directory there is forwarded so the gateway refuses it.
        monkeypatch.chdir(tmp_path)
        (tmp_path / "config" / "gating.toml").mkdir(parents=True)

        b = Meta.builder().mob("config/mob.toml")
        b._apply_convention_defaults()
        assert b._config.gating_config_path == "config/gating.toml"

    @pytest.mark.skipif(os.name != "posix", reason="symlinks and ENOTDIR are POSIX fixtures")
    def test_dangling_or_unobservable_gating_entry_is_forwarded_for_refusal(
        self, tmp_path, monkeypatch
    ):
        monkeypatch.chdir(tmp_path)
        (tmp_path / "config").mkdir()
        (tmp_path / "config" / "gating.toml").symlink_to(tmp_path / "missing.toml")

        dangling = Meta.builder().mob("config/mob.toml")
        dangling._apply_convention_defaults()
        assert dangling._config.gating_config_path == "config/gating.toml"

        (tmp_path / "config" / "gating.toml").unlink()
        (tmp_path / "config").rmdir()
        (tmp_path / "config").write_text("")

        unobservable = Meta.builder().mob("config/mob.toml")
        unobservable._apply_convention_defaults()
        assert unobservable._config.gating_config_path == "config/gating.toml"

    @pytest.mark.skipif(os.name != "posix", reason="symlinks are POSIX fixtures")
    def test_opt_in_files_under_a_dangling_parent_are_omitted(self, tmp_path, monkeypatch):
        # "Not found" also covers a dangling config directory link, so the
        # opt-in files are omitted there exactly like a missing file.
        monkeypatch.chdir(tmp_path)
        (tmp_path / "config").symlink_to(tmp_path / "missing-dir")

        b = Meta.builder().mob("config/mob.toml")
        b._apply_convention_defaults()
        assert b._config.access_config_path is None
        assert b._config.gating_config_path is None

    def test_explicit_empty_gating_path_is_rejected(self):
        with pytest.raises(ValueError, match="non-empty"):
            Meta.builder().gating("")

    def test_explicit_empty_gating_path_names_how_to_keep_gating_off(self):
        with pytest.raises(ValueError) as excinfo:
            Meta.builder().gating("")
        assert (
            "an empty path no longer switches gating off. To keep gating off, do not call "
            "gating(), and keep nothing at config/gating.toml"
        ) in str(excinfo.value)

    def test_explicit_gating_path_is_forwarded_never_dropped(self, tmp_path, monkeypatch):
        # An explicit gating path, even an empty one set directly, reaches the
        # gateway, which refuses it; only an absent path is omitted.
        monkeypatch.chdir(tmp_path)

        b = Meta.builder().mob_inline("[mob]\nid = \"t\"")
        assert "gating_config_path" not in MetaRuntime(b._config)._build_init_params()[
            "runtime_options"
        ]
        b._config.gating_config_path = ""
        params = MetaRuntime(b._config)._build_init_params()
        assert params["runtime_options"]["gating_config_path"] == ""

    def test_explicit_access_overrides_convention(self, tmp_path, monkeypatch):
        monkeypatch.chdir(tmp_path)
        (tmp_path / "config").mkdir()
        (tmp_path / "config" / "access.toml").write_text("conventional")

        b = Meta.builder().mob("config/mob.toml").access_control("custom/access.toml")
        b._apply_convention_defaults()
        assert b._config.access_config_path == "custom/access.toml"

    def test_access_config_path_reaches_runtime_options(self, tmp_path, monkeypatch):
        monkeypatch.chdir(tmp_path)

        b = Meta.builder().mob_inline("[mob]\nid = \"t\"").access_control("config/access.toml")
        params = MetaRuntime(b._config)._build_init_params()
        assert params["runtime_options"]["access_config_path"] == "config/access.toml"

    def test_explicit_empty_access_path_is_rejected(self):
        with pytest.raises(ValueError, match="non-empty"):
            Meta.builder().access_control("")

    def test_explicit_empty_access_path_names_how_to_keep_access_control_off(self):
        with pytest.raises(ValueError) as excinfo:
            Meta.builder().access_control("")
        assert (
            "an empty path no longer switches access control off. To keep access control "
            "off, do not call access_control(), and keep nothing at config/access.toml"
        ) in str(excinfo.value)

    def test_explicit_access_path_is_forwarded_never_dropped(self, tmp_path, monkeypatch):
        # An explicit path, even an empty one set directly, reaches the gateway,
        # which refuses it; only an absent path is omitted.
        monkeypatch.chdir(tmp_path)

        b = Meta.builder().mob_inline("[mob]\nid = \"t\"")
        assert "access_config_path" not in MetaRuntime(b._config)._build_init_params()[
            "runtime_options"
        ]
        b._config.access_config_path = ""
        params = MetaRuntime(b._config)._build_init_params()
        assert params["runtime_options"]["access_config_path"] == ""

    def test_routing_discovered(self, tmp_path, monkeypatch):
        monkeypatch.chdir(tmp_path)
        (tmp_path / "deployment").mkdir()
        (tmp_path / "deployment" / "routing.toml").write_text("[[routes]]")

        b = Meta.builder().mob("config/mob.toml")
        b._apply_convention_defaults()
        assert b._config.routing_config_path == "deployment/routing.toml"

    def test_missing_files_skipped(self, tmp_path, monkeypatch):
        monkeypatch.chdir(tmp_path)

        b = Meta.builder().mob("config/mob.toml")
        b._apply_convention_defaults()
        assert b._config.gating_config_path is None
        assert b._config.access_config_path is None
        assert b._config.routing_config_path is None

    def test_explicit_overrides_convention(self, tmp_path, monkeypatch):
        monkeypatch.chdir(tmp_path)
        (tmp_path / "config").mkdir()
        (tmp_path / "config" / "gating.toml").write_text("conventional")

        b = Meta.builder().mob("config/mob.toml").gating("custom/gating.toml")
        b._apply_convention_defaults()
        assert b._config.gating_config_path == "custom/gating.toml"


class TestRemovedBootKnobs:
    """`.discovery()` / `.pre_spawn()` were stored and never transmitted (dead
    since v0.2). A call must now refuse loudly instead of being accepted and
    dropped, and the config carries no slot for them."""

    def test_discovery_is_not_a_builder_method(self):
        with pytest.raises(AttributeError, match="discovery"):
            Meta.builder().discovery(lambda ctx: [])

    def test_pre_spawn_is_not_a_builder_method(self):
        with pytest.raises(AttributeError, match="pre_spawn"):
            Meta.builder().pre_spawn(lambda: {})

    def test_config_has_no_dead_callback_slots(self):
        from dataclasses import fields

        names = {f.name for f in fields(Meta.builder()._config)}
        assert "discovery_callback" not in names
        assert "pre_spawn_callback" not in names
        # The live boot-membership knob is still there.
        assert "roster_provider" in names
