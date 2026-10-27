"""Real gateway smoke tests for Python agent-memory helpers.

This covers the Python SDK -> rpc_gateway -> Rust agent-memory provider path
without requiring an LLM provider key. It boots an identity-first runtime,
writes a durable memory record into the bundled SQLite store, recalls it, and
then deletes it.

It also covers the ONE supported path off the retired markdown store: a
pre-migration `.md` file left on disk is imported on first open of the SQLite
store, which is the concrete promise
`AgentMemoryStoreMigration::MarkdownIsImportOnly` makes to a deployment that
still has `store = "markdown"` pinned.

Run:
    scripts/repo-cargo build -p meerkat-meta --bin rpc_gateway
    PYTHONPATH=sdk/python python3 -m pytest sdk/python/tests/test_agent_memory_real_gateway.py -q
"""
from __future__ import annotations

import os
from pathlib import Path
import subprocess

import pytest

from meerkat_meta.builder import Meta
from meerkat_meta.identity_first_models import DurableAgentSpec


def _resolve_gateway_bin() -> str:
    override = os.environ.get("META_GATEWAY_BIN", "").strip()
    if override:
        return override
    repo_root = Path(__file__).resolve().parents[3]
    try:
        result = subprocess.run(
            [str(repo_root / "scripts" / "repo-cargo"), "--print-env", "CARGO_TARGET_DIR"],
            cwd=repo_root,
            check=True,
            capture_output=True,
            text=True,
        )
    except (OSError, subprocess.CalledProcessError):
        return ""
    for line in result.stdout.splitlines():
        if line.startswith("CARGO_TARGET_DIR="):
            target_dir = line.split("=", 1)[1].strip()
            if target_dir:
                return str(Path(target_dir) / "debug" / "rpc_gateway")
    return ""


_GATEWAY_BIN = _resolve_gateway_bin()

_skip_no_binary = pytest.mark.skipif(
    not _GATEWAY_BIN or not os.path.isfile(_GATEWAY_BIN),
    reason=(
        f"Gateway binary not found at {_GATEWAY_BIN} - run: "
        "./scripts/repo-cargo build -p meerkat-meta --bin rpc_gateway"
    ),
)


_MOB_TOML = """\
[mob]
id = "agent-memory-python-smoke"

[profiles.memory_smoke]
model = "gpt-5.5"
external_addressable = true

[profiles.memory_smoke.tools]
comms = true
"""


# A pre-migration markdown file exactly as the retired MarkdownAgentMemoryStore
# rendered it: "# Meta Agent Memory" banner, then per record a "## <title>"
# heading, an HTML-comment metadata line carrying the id/tags/timestamps, the
# body, and the record-end marker.
_SEEDED_MARKDOWN = """\
# Meta Agent Memory

## Imported smoke token
<!-- mobkit-agent-memory {"memory_id":"mem-py-import-1","tags":["python","import"],\
"created_at_ms":1750000000000,"updated_at_ms":1750000000000} -->
The imported markdown smoke token is PY-MEM-19.
<!-- /mobkit-agent-memory -->

"""


class SmokeRoster:
    async def roster(self, context: dict) -> list[DurableAgentSpec]:
        return [
            DurableAgentSpec(
                identity="identity:memory-smoke",
                profile="memory_smoke",
                addressability="addressable",
                labels={"topic": "memory-smoke"},
                additional_instructions=["Keep smoke-test answers terse."],
            )
        ]


@_skip_no_binary
@pytest.mark.asyncio
@pytest.mark.timeout(60)
async def test_python_agent_memory_imports_legacy_markdown_and_forgets(tmp_path):
    """The migration off `store = "markdown"`, end to end through the gateway.

    `store = "markdown"` no longer boots: the gateway refuses it at init with
    `AgentMemoryStoreMigration::MarkdownIsImportOnly`, whose message promises
    that pointing the SQLite store at the SAME agent-memory directory imports
    every un-imported `.md` file, preserving ids/tags/timestamps, and renames
    the source to `.md.imported` rather than deleting it. The store-level
    behaviour is pinned in Rust by
    `sqlite_store::markdown_import_preserves_ids_and_renames_file`; this is the
    only exercise of it THROUGH THE GATEWAY, and so the only proof that a real
    deployment can follow the refusal message and get its records back.

    Deleting the imported record afterwards keeps `forget_agent_memory` covered
    at the Python level (the sqlite-default test below never calls it).
    """
    mob_toml = tmp_path / "mob.toml"
    mob_toml.write_text(_MOB_TOML)
    state_dir = tmp_path / "state"
    state_dir.mkdir()

    # Seed a pre-migration file where the retired markdown store kept it:
    # <persistent_state>/agent-memory/<realm>/<pct-encoded identity>.md
    markdown_file = (
        state_dir
        / "agent-memory"
        / "default"
        / "identity%3Amemory-smoke.md"
    )
    markdown_file.parent.mkdir(parents=True)
    markdown_file.write_text(_SEEDED_MARKDOWN)

    runtime = await (
        Meta.builder()
        .gateway(_GATEWAY_BIN)
        .mob(str(mob_toml))
        .persistent_state(str(state_dir))
        .roster(SmokeRoster())
        # No `store` key: sqlite is the default, and opening it imports every
        # un-imported .md file sitting in the realm directory. This is the
        # exact one-step migration the refusal message prescribes.
        .agent_memory(selection="contextual", max_entries=4)
        .build()
    )
    try:
        handle = runtime.mob_handle()

        # `always` rather than `contextual`: the assertion under test is that
        # the record was imported at all, not how it scores against a query.
        recalled = await handle.recall_agent_memory(
            "identity:memory-smoke",
            selection="always",
            max_entries=4,
        )
        assert [item.memory_id for item in recalled] == ["mem-py-import-1"], (
            "the seeded markdown record must be recallable from sqlite, "
            "with its original id preserved"
        )
        assert recalled[0].title == "Imported smoke token"
        assert recalled[0].body == "The imported markdown smoke token is PY-MEM-19."
        # Sorted, not seeded order: every sqlite insert - the import included -
        # goes through `insert_record`, which runs `normalize_tags`, and that
        # lowercases and dedups through a BTreeSet. The seeded file says
        # ["python", "import"]; the store returns them collated. Preserving tag
        # CONTENT is the migration promise, not preserving tag order.
        assert recalled[0].tags == ["import", "python"]

        sqlite_file = state_dir / "agent-memory" / "default.sqlite3"
        assert sqlite_file.is_file(), "the import target is the per-realm sqlite database"

        # The source is set aside, never deleted: user-inspectable data survives
        # the migration.
        assert not markdown_file.exists(), "the imported source must be renamed"
        imported_file = markdown_file.parent / (markdown_file.name + ".imported")
        assert imported_file.is_file(), "the markdown source must survive as .md.imported"
        assert "PY-MEM-19" in imported_file.read_text()

        forgotten = await handle.forget_agent_memory(
            "identity:memory-smoke", "mem-py-import-1"
        )
        assert forgotten.memory_id == "mem-py-import-1"
        assert forgotten.deleted is True

        after_forget = await handle.recall_agent_memory(
            "identity:memory-smoke",
            selection="always",
            max_entries=4,
        )
        assert after_forget == []
    finally:
        await runtime.shutdown()


@_skip_no_binary
@pytest.mark.asyncio
@pytest.mark.timeout(60)
async def test_python_agent_memory_defaults_to_sqlite_store(tmp_path):
    mob_toml = tmp_path / "mob.toml"
    mob_toml.write_text(_MOB_TOML)
    state_dir = tmp_path / "state"
    state_dir.mkdir()

    runtime = await (
        Meta.builder()
        .gateway(_GATEWAY_BIN)
        .mob(str(mob_toml))
        .persistent_state(str(state_dir))
        .roster(SmokeRoster())
        .agent_memory(selection="contextual", max_entries=4)
        .build()
    )
    try:
        handle = runtime.mob_handle()
        record = await handle.remember_agent_memory(
            "identity:memory-smoke",
            title="Sqlite smoke token",
            body="The sqlite default-store smoke token is PY-MEM-18.",
            tags=["python", "smoke"],
        )
        assert record.memory_id.startswith("mem-")

        recalled = await handle.recall_agent_memory(
            "identity:memory-smoke",
            selection="contextual",
            query_text="Where is the PY-MEM-18 token?",
            query_terms=["PY-MEM-18"],
            max_entries=4,
        )
        assert [item.memory_id for item in recalled] == [record.memory_id]

        sqlite_file = state_dir / "agent-memory" / "default.sqlite3"
        assert sqlite_file.is_file(), "default store must be the per-realm sqlite database"
        markdown_file = (
            state_dir / "agent-memory" / "default" / "identity%3Amemory-smoke.md"
        )
        assert not markdown_file.exists(), "sqlite default must not write markdown files"
    finally:
        await runtime.shutdown()
