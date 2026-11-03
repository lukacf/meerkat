"""TDD tests for provider protocols (REQ-42)."""
from meerkat_meta.identity_first_providers import (
    AgentCustomizerProtocol,
    RosterProviderProtocol,
    TopologyProviderProtocol,
)


# ---------------------------------------------------------------------------
# REQ-42: Provider Protocol classes
# ---------------------------------------------------------------------------


class TestRosterProviderProtocol:
    def test_protocol_methods(self):
        import inspect
        members = {name for name, _ in inspect.getmembers(
            RosterProviderProtocol, predicate=inspect.isfunction
        )}
        assert "roster" in members

    def test_mock_implementation(self):
        class MockRoster:
            async def roster(self, context):
                return []

        assert isinstance(MockRoster(), RosterProviderProtocol)


class TestAgentCustomizerProtocol:
    def test_protocol_methods(self):
        import inspect
        members = {name for name, _ in inspect.getmembers(
            AgentCustomizerProtocol, predicate=inspect.isfunction
        )}
        assert "customize_build" in members

    def test_mock_implementation(self):
        class MockCustomizer:
            async def customize_build(self, context, spec, draft):
                pass

            async def after_create(self, identity, session_id, context):
                pass

        assert isinstance(MockCustomizer(), AgentCustomizerProtocol)


class TestTopologyProviderProtocol:
    def test_protocol_methods(self):
        import inspect
        members = {name for name, _ in inspect.getmembers(
            TopologyProviderProtocol, predicate=inspect.isfunction
        )}
        assert "compute_edges" in members

    def test_mock_implementation(self):
        class MockTopology:
            async def compute_edges(self, target_identities, context):
                return []

        assert isinstance(MockTopology(), TopologyProviderProtocol)
