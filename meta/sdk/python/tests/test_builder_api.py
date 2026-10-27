"""TDD tests for the new builder API: persistent_state, after_create, SessionCreatedContext."""
import pytest
from meerkat_meta.builder import Meta, MetaBuilder
from meerkat_meta.agent_builder import CallbackDispatcher, SessionAgentBuilder
from meerkat_meta.models import SessionBuildOptions, SessionCreatedContext


class TestPersistentState:
    """Tests for .persistent_state() on MetaBuilder."""

    def test_persistent_state_returns_builder(self):
        b = Meta.builder().persistent_state("/tmp/test-state")
        assert isinstance(b, MetaBuilder)

    def test_persistent_state_sets_config(self):
        b = Meta.builder().persistent_state("/tmp/test-state")
        assert b._config.persistent_state == "/tmp/test-state"

    def test_persistent_state_default_is_none(self):
        b = Meta.builder()
        assert b._config.persistent_state is None


class TestAfterCreate:
    """Tests for after_create callback on SessionAgentBuilder and CallbackDispatcher."""

    @pytest.mark.asyncio
    async def test_after_create_dispatched(self):
        """callback/after_create routes to builder.after_create()."""
        received = {}

        class Builder:
            async def build_agent(self, opts: SessionBuildOptions) -> None:
                pass

            async def after_create(self, session_id: str, context: dict) -> None:
                received["session_id"] = session_id
                received["context"] = context

        d = CallbackDispatcher()
        d.register_builder(Builder())

        await d.handle_callback("callback/after_create", {
            "session_id": "sid-123",
            "model": "claude-sonnet-4-5",
            "labels": {"agent_type": "lead"},
            "system_prompt": "You are a lead.",
        })

        assert received["session_id"] == "sid-123"
        ctx = received["context"]
        assert isinstance(ctx, SessionCreatedContext)
        assert ctx.model == "claude-sonnet-4-5"
        assert ctx.labels == {"agent_type": "lead"}
        assert ctx.system_prompt == "You are a lead."

    @pytest.mark.asyncio
    async def test_after_create_noop_when_not_defined(self):
        """Builders without after_create don't fail on callback/after_create."""

        class MinimalBuilder:
            async def build_agent(self, opts: SessionBuildOptions) -> None:
                pass

        d = CallbackDispatcher()
        d.register_builder(MinimalBuilder())

        # Should not raise — gracefully ignored.
        await d.handle_callback("callback/after_create", {
            "session_id": "sid-456",
            "model": "test-model",
            "labels": {},
            "system_prompt": None,
        })

    @pytest.mark.asyncio
    async def test_after_create_error_is_logged_not_raised(self):
        """after_create failures are best-effort — logged, not propagated."""

        class FailingBuilder:
            async def build_agent(self, opts: SessionBuildOptions) -> None:
                pass

            async def after_create(self, session_id: str, context: dict) -> None:
                raise RuntimeError("db unavailable")

        d = CallbackDispatcher()
        d.register_builder(FailingBuilder())

        # Should not raise — error is logged.
        await d.handle_callback("callback/after_create", {
            "session_id": "sid-789",
            "model": "test-model",
            "labels": {},
            "system_prompt": None,
        })


class TestSessionCreatedContext:
    """Tests for SessionCreatedContext dataclass."""

    def test_import(self):
        from meerkat_meta import SessionCreatedContext
        ctx = SessionCreatedContext(
            model="claude-sonnet-4-5",
            labels={"agent_type": "lead"},
            system_prompt="You are a lead agent.",
        )
        assert ctx.model == "claude-sonnet-4-5"
        assert ctx.labels == {"agent_type": "lead"}
        assert ctx.system_prompt == "You are a lead agent."

    def test_from_dict(self):
        from meerkat_meta import SessionCreatedContext
        ctx = SessionCreatedContext.from_dict({
            "model": "test-model",
            "labels": {"k": "v"},
            "system_prompt": None,
        })
        assert ctx.model == "test-model"
        assert ctx.labels == {"k": "v"}
        assert ctx.system_prompt is None


class TestBuildAgentErrorPropagation:
    """Tests that build_agent errors propagate (not fallback)."""

    @pytest.mark.asyncio
    async def test_build_agent_error_raises(self):
        """When build_agent throws, the error should propagate up."""

        class FailingBuilder:
            async def build_agent(self, opts: SessionBuildOptions) -> None:
                raise ValueError("hook abort: invalid config")

        d = CallbackDispatcher()
        d.register_builder(FailingBuilder())

        with pytest.raises(ValueError, match="hook abort"):
            await d.handle_callback(
                "callback/build_agent",
                {"options": {"scope_id": "s1"}},
            )


class TestResumeSessionId:
    """Tests for resume_session_id on SessionBuildOptions."""

    def test_field_exists_default_none(self):
        opts = SessionBuildOptions()
        assert opts.resume_session_id is None

    def test_field_set(self):
        opts = SessionBuildOptions()
        opts.resume_session_id = "sid-resume-123"
        assert opts.resume_session_id == "sid-resume-123"

    def test_to_dict_includes_when_set(self):
        opts = SessionBuildOptions()
        opts.resume_session_id = "sid-resume-456"
        d = opts.to_dict()
        assert d["resume_session_id"] == "sid-resume-456"

    def test_to_dict_omits_when_none(self):
        opts = SessionBuildOptions()
        d = opts.to_dict()
        assert "resume_session_id" not in d

    @pytest.mark.asyncio
    async def test_build_agent_sets_resume_session_id(self):
        """build_agent can set resume_session_id and it is serialized in to_dict().

        Note: actual session resume flows through DiscoverySpec.resume_session_id
        at the mob spawn layer, not through the build_agent callback. This field
        is informational for the build_agent result.
        """

        class ResumeBuilder:
            async def build_agent(self, opts: SessionBuildOptions) -> None:
                opts.resume_session_id = "sid-owner-789"

        d = CallbackDispatcher()
        d.register_builder(ResumeBuilder())

        result = await d.handle_callback(
            "callback/build_agent",
            {"options": {"scope_id": "s1"}},
        )
        assert result["resume_session_id"] == "sid-owner-789"


class TestForkSource:
    """fork_source on callback/build_agent (meerkat 0.8.45+): typed on receipt,
    tolerant to fields a newer gateway adds, and never sent back."""

    SOURCE_SESSION_ID = "0192f5c4-7a3e-7d21-9b0e-4c1d2e3f4a5b"

    def _fork_source_wire(self) -> dict:
        return {
            "source_member": {
                "mob_id": "example",
                "role": "domain",
                "member": "mk--domain_ccalendar",
                "future_member_field": True,
            },
            "source_session_id": self.SOURCE_SESSION_ID,
            "future_source_field": {"nested": [1, 2]},
        }

    @pytest.mark.asyncio
    async def test_fork_build_receives_typed_source_and_identity(self):
        from meerkat_meta import ForkBuildSource, MobMemberBinding

        seen: dict = {}

        class ForkAwareBuilder:
            async def build_agent(self, opts: SessionBuildOptions) -> None:
                seen["opts"] = opts

        d = CallbackDispatcher()
        d.register_builder(ForkAwareBuilder())
        await d.handle_callback(
            "callback/build_agent",
            {
                "options": {
                    "scope_id": "s1",
                    "session_id": "child-session",
                    "resume_session_id": "child-session",
                    "profile_name": "domain",
                    "labels": {"session_id": "child-session"},
                    "fork_source": self._fork_source_wire(),
                    "fork_source_identity": "domain:calendar",
                    "future_top_level_field": 1,
                }
            },
        )
        opts = seen["opts"]
        assert opts.fork_source == ForkBuildSource(
            source_member=MobMemberBinding(
                mob_id="example", role="domain", member="mk--domain_ccalendar"
            ),
            source_session_id=self.SOURCE_SESSION_ID,
        )
        assert opts.fork_source_identity == "domain:calendar"
        # The child keeps its own identity and session.
        assert opts.session_id == "child-session"
        assert opts.labels == {"session_id": "child-session"}

    @pytest.mark.asyncio
    async def test_fork_source_is_never_sent_back(self):
        from meerkat_meta import ForkBuildSource, MobMemberBinding

        class MintingBuilder:
            async def build_agent(self, opts: SessionBuildOptions) -> None:
                opts.fork_source = ForkBuildSource(
                    source_member=MobMemberBinding(mob_id="x", role="y", member="z"),
                    source_session_id="spoofed",
                )
                opts.fork_source_identity = "spoofed"

        d = CallbackDispatcher()
        d.register_builder(MintingBuilder())
        for options in (
            {"scope_id": "s1"},
            {
                "scope_id": "s2",
                "fork_source": self._fork_source_wire(),
                "fork_source_identity": "domain:calendar",
            },
        ):
            result = await d.handle_callback("callback/build_agent", {"options": options})
            assert "fork_source" not in result
            assert "fork_source_identity" not in result

    @pytest.mark.asyncio
    async def test_ordinary_build_has_no_fork_source(self):
        seen: list = []

        class Builder:
            async def build_agent(self, opts: SessionBuildOptions) -> None:
                seen.append(opts)

        d = CallbackDispatcher()
        d.register_builder(Builder())
        await d.handle_callback("callback/build_agent", {"options": {"scope_id": "s1"}})
        await d.handle_callback(
            "callback/build_agent",
            {
                "options": {
                    "scope_id": "s2",
                    "fork_source": None,
                    "fork_source_identity": None,
                }
            },
        )
        assert [(o.fork_source, o.fork_source_identity) for o in seen] == [
            (None, None),
            (None, None),
        ]

    @pytest.mark.asyncio
    async def test_malformed_fork_source_fails_the_build(self):
        class Builder:
            async def build_agent(self, opts: SessionBuildOptions) -> None:
                raise AssertionError("must not be called")

        d = CallbackDispatcher()
        d.register_builder(Builder())
        with pytest.raises(TypeError, match="source_session_id"):
            await d.handle_callback(
                "callback/build_agent",
                {
                    "options": {
                        "scope_id": "s1",
                        "fork_source": {
                            "source_member": {"mob_id": "example", "role": "domain", "member": "m"}
                        },
                    }
                },
            )
