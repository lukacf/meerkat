"""Tests for the M5 storage durability vocabulary.

Covers the typed config blocks (``config.runtime_store`` /
``config.event_log``), their exact ``runtime_options`` wire forms, the
typed storage census model (``StorageSummary`` / ``StorageSlotSummary``)
parsed from ``meta/status`` / ``meta/capabilities``, and the
``StorageResolutionError`` (-32014) reification for the gateway's
fail-closed init refusals.
"""
import pytest

import meerkat_meta
from meerkat_meta.builder import Meta
from meerkat_meta.config import event_log, runtime_store
from meerkat_meta.errors import (
    STORAGE_RESOLUTION_CODE,
    MetaError,
    RpcError,
    StorageResolutionError,
)
from meerkat_meta.runtime import MetaRuntime, _rpc_error_from_payload
from meerkat_meta.types import (
    CapabilitiesResult,
    StatusResult,
    StorageSlotSummary,
    StorageSummary,
)


class TestRuntimeStoreConfig:
    def test_memory_wire_form(self):
        assert runtime_store.memory().to_dict() == {"storage": "memory"}

    def test_builder_emits_runtime_store_option(self):
        b = Meta.builder().runtime_store(runtime_store.memory())
        params = MetaRuntime(b._config)._build_init_params()
        assert params["runtime_options"]["runtime_store"] == {"storage": "memory"}

    def test_builder_accepts_raw_wire_dict(self):
        b = Meta.builder().runtime_store({"storage": "memory"})
        params = MetaRuntime(b._config)._build_init_params()
        assert params["runtime_options"]["runtime_store"] == {"storage": "memory"}

    def test_omitted_by_default(self):
        params = MetaRuntime(Meta.builder()._config)._build_init_params()
        assert "runtime_store" not in params["runtime_options"]


class TestEventLogConfig:
    def test_memory_wire_form(self):
        assert event_log.memory().to_dict() == {"storage": "memory"}
        assert event_log.memory(batch_size=1, flush_interval_ms=10).to_dict() == {
            "storage": "memory",
            "batch_size": 1,
            "flush_interval_ms": 10,
        }

    def test_null_wire_form(self):
        assert event_log.null().to_dict() == {"storage": "null"}

    def test_memory_rejects_non_positive_flush_interval(self):
        # A zero interval would panic the gateway's ingestion task.
        with pytest.raises(ValueError, match="flush_interval_ms"):
            event_log.memory(flush_interval_ms=0)
        with pytest.raises(ValueError, match="flush_interval_ms"):
            event_log.memory(flush_interval_ms=-5)

    def test_memory_rejects_non_integer_flush_interval(self):
        with pytest.raises(ValueError, match="flush_interval_ms"):
            event_log.memory(flush_interval_ms=10.5)
        with pytest.raises(ValueError, match="flush_interval_ms"):
            event_log.memory(flush_interval_ms=float("nan"))

    def test_builder_accepts_typed_config(self):
        b = Meta.builder().event_log(event_log.null())
        params = MetaRuntime(b._config)._build_init_params()
        assert params["runtime_options"]["event_log"] == {"storage": "null"}

    def test_builder_keyword_form_still_works(self):
        b = Meta.builder().event_log(storage="memory", batch_size=2)
        params = MetaRuntime(b._config)._build_init_params()
        assert params["runtime_options"]["event_log"] == {
            "storage": "memory",
            "batch_size": 2,
        }

    def test_builder_rejects_both_forms(self):
        with pytest.raises(ValueError):
            Meta.builder().event_log(event_log.null(), storage="memory")

    def test_builder_rejects_neither_form(self):
        with pytest.raises(ValueError):
            Meta.builder().event_log()


class TestStorageCensusModel:
    WIRE = {
        "blob_durability": "persistent_disk",
        "blob_store_persistent": True,
        "session_store_incremental": False,
        "slots": [
            {
                "domain": "runtime",
                "class": "durable",
                "resolution": "persistent",
                "backend": "SqliteRuntimeStore",
                "degraded": False,
            },
            {
                "domain": "schedule",
                "class": "durable",
                "resolution": "non_persistent",
                "backend": "disabled",
                "degraded": True,
                "detail": "schedule store failed to open",
            },
            {
                "domain": "gating_audit",
                "class": "scratch",
                "resolution": "declared_ephemeral",
                "backend": "in-process ring buffer",
                "degraded": False,
                "detail": "drop-oldest retention (512 entries)",
            },
        ],
    }

    def test_parses_m4_wire_shape(self):
        summary = StorageSummary.from_dict(self.WIRE)
        assert summary.blob_durability == "persistent_disk"
        assert summary.blob_store_persistent is True
        assert summary.session_store_incremental is False
        assert len(summary.slots) == 3
        runtime_slot = summary.slot("runtime")
        assert isinstance(runtime_slot, StorageSlotSummary)
        assert runtime_slot.durability_class == "durable"
        assert runtime_slot.resolution == "persistent"
        assert runtime_slot.backend == "SqliteRuntimeStore"
        assert runtime_slot.degraded is False
        assert runtime_slot.detail is None
        schedule_slot = summary.slot("schedule")
        assert schedule_slot.degraded is True
        assert schedule_slot.detail == "schedule store failed to open"
        assert summary.slot("nonexistent") is None

    def test_forward_tolerant_parsing(self):
        summary = StorageSummary.from_dict(
            {
                "blob_durability": "some_future_kind",
                "blob_store_persistent": True,
                "slots": [
                    {
                        "domain": "future",
                        "class": "warm_tier",
                        "resolution": "replicated",
                        "backend": "S3",
                        "future_field": {"nested": True},
                    }
                ],
                "future_top_level": 7,
            }
        )
        assert summary.blob_durability == "some_future_kind"
        assert summary.session_store_incremental is None
        assert summary.slots[0].durability_class == "warm_tier"
        assert summary.slots[0].resolution == "replicated"

    def test_pre_census_shape_parses_with_empty_slots(self):
        summary = StorageSummary.from_dict(
            {"blob_durability": "declared_ephemeral", "blob_store_persistent": False}
        )
        assert summary.slots == []

    def test_status_result_carries_storage(self):
        raw = {
            "contract_version": "0.5.0",
            "running": True,
            "loaded_modules": [],
            "storage": self.WIRE,
        }
        status = StatusResult.from_dict(raw)
        assert isinstance(status.storage, StorageSummary)
        assert status.storage.slot("runtime").backend == "SqliteRuntimeStore"

    def test_status_result_ignores_fields_it_does_not_model(self):
        status = StatusResult.from_dict(
            {
                "contract_version": "0.5.0",
                "running": True,
                "loaded_modules": [],
                "reserved_identity_members": [
                    {
                        "member_id": "worker-1",
                        "runtime_id": "worker-1:0",
                        "identity": "_system",
                    }
                ],
            }
        )
        assert status.contract_version == "0.5.0"
        assert status.running is True
        assert status.storage is None

    def test_status_result_ignores_non_expiring_console_principals(self):
        status = StatusResult.from_dict(
            {
                "contract_version": "0.5.0",
                "running": True,
                "loaded_modules": [],
                "non_expiring_console_principals": [
                    {"principal": "svc:reporter", "non_expiring": True}
                ],
            }
        )
        assert status.contract_version == "0.5.0"
        assert status.running is True
        assert status.storage is None

    def test_status_result_ignores_the_mob_failure(self):
        status = StatusResult.from_dict(
            {
                "contract_version": "0.5.0",
                "running": True,
                "loaded_modules": [],
                "mob_state": "Some(Running)",
                "mob_failure": "the mob's actor stopped",
            }
        )
        assert status.contract_version == "0.5.0"
        assert status.running is True
        assert status.storage is None

    def test_status_result_without_storage_is_none(self):
        status = StatusResult.from_dict(
            {"contract_version": "0.5.0", "running": True, "loaded_modules": []}
        )
        assert status.storage is None

    def test_capabilities_result_carries_storage(self):
        raw = {
            "contract_version": "0.5.0",
            "methods": ["meta/status"],
            "loaded_modules": [],
            "storage": self.WIRE,
        }
        capabilities = CapabilitiesResult.from_dict(raw)
        assert isinstance(capabilities.storage, StorageSummary)
        assert capabilities.storage.blob_durability == "persistent_disk"


class TestStorageResolutionError:
    def test_is_a_typed_rpc_error(self):
        assert issubclass(StorageResolutionError, RpcError)
        assert issubclass(StorageResolutionError, MetaError)
        assert STORAGE_RESOLUTION_CODE == -32014

    def test_reified_from_wire_payload(self):
        err = _rpc_error_from_payload(
            {
                "code": STORAGE_RESOLUTION_CODE,
                "message": (
                    "file-name twins for the sessions store: ... run the storage "
                    "doctor (meta/storage/doctor)"
                ),
            },
            request_id="init:1",
            method="meta/init",
        )
        assert isinstance(err, StorageResolutionError)
        assert err.code == STORAGE_RESOLUTION_CODE
        assert "meta/storage/doctor" in str(err)

    def test_other_codes_do_not_reify(self):
        err = _rpc_error_from_payload(
            {"code": -32603, "message": "internal"},
            request_id="init:1",
            method="meta/init",
        )
        assert not isinstance(err, StorageResolutionError)


class TestTopLevelExports:
    def test_durability_surface_is_exported(self):
        assert meerkat_meta.runtime_store is runtime_store
        assert meerkat_meta.event_log is event_log
        assert meerkat_meta.StorageResolutionError is StorageResolutionError
        assert meerkat_meta.STORAGE_RESOLUTION_CODE == -32014
        assert meerkat_meta.StorageSummary is StorageSummary
        assert meerkat_meta.StorageSlotSummary is StorageSlotSummary
