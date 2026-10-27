#!/usr/bin/env python3
"""Contract test: the decorator-conformance gate fails on the defect it was
written for (a production decorator missing a defaulted method), ignores test
doubles, respects feature gates, checks every trait in `TRAIT_SPECS`, and only
honours an exemption that names a method the trait still has."""

from __future__ import annotations

import importlib.util
import sys
import unittest
from pathlib import Path

SPEC = importlib.util.spec_from_file_location(
    "gate", Path(__file__).with_name("verify-session-service-decorators.py")
)
gate = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(gate)

# verify-decorator-authority.py carries its own copy of the test-module scan.
AUTHORITY_SPEC = importlib.util.spec_from_file_location(
    "authority", Path(__file__).with_name("verify-decorator-authority.py")
)
authority = importlib.util.module_from_spec(AUTHORITY_SPEC)
assert AUTHORITY_SPEC.loader is not None
AUTHORITY_SPEC.loader.exec_module(authority)

TRAIT_SRC = '''
pub trait MobSessionService:
    Send + Sync + SomeOtherTrait
{
    async fn required_one(&self) -> Result<(), SessionError>;
    async fn defaulted_refusal(&self) -> Result<(), SessionError> {
        Err(SessionError::Unsupported("nope".into()))
    }
    #[cfg(feature = "experimental-gpt-live")]
    async fn live_only(&self) -> Result<(), SessionError> {
        Err(SessionError::Unsupported("nope".into()))
    }
    fn plain_default(&self) -> bool {
        false
    }
}
'''

IMPL_SRC = '''
impl MobSessionService for GoodWrapper {
    async fn required_one(&self) -> Result<(), SessionError> { self.inner.required_one().await }
    async fn defaulted_refusal(&self) -> Result<(), SessionError> { self.inner.defaulted_refusal().await }
    fn plain_default(&self) -> bool { self.inner.plain_default() }
}

impl MobSessionService for ForgetfulWrapper {
    async fn required_one(&self) -> Result<(), SessionError> { self.inner.required_one().await }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    impl MobSessionService for TestDouble {
        async fn required_one(&self) -> Result<(), SessionError> { Ok(()) }
    }
}
'''

VISIBLE_TEST_MODULES_SRC = '''
#[cfg(test)]
pub(crate) mod crate_support {
    impl MobSessionService for CrateDouble {
        async fn required_one(&self) -> Result<(), SessionError> { Ok(()) }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
pub(super) mod super_support {
    impl MobSessionService for SuperDouble {
        async fn required_one(&self) -> Result<(), SessionError> { Ok(()) }
    }
}

#[cfg(test)]
pub(in crate::runtime) mod path_support {
    impl MobSessionService for PathDouble {
        async fn required_one(&self) -> Result<(), SessionError> { Ok(()) }
    }
}

#[cfg(test)]
pub mod public_support {
    impl MobSessionService for PublicDouble {
        async fn required_one(&self) -> Result<(), SessionError> { Ok(()) }
    }
}

pub mod production {
    impl MobSessionService for ForgetfulWrapper {
        async fn required_one(&self) -> Result<(), SessionError> { self.inner.required_one().await }
    }
}
'''

# A test module can open and close on its declaration line, with production
# code right after it: no blank line, no attribute.
ONE_LINE_TEST_MODULES_SRC = '''
#[cfg(test)]
pub(crate) mod empty {}
impl MobSessionService for ForgetfulWrapper {
    async fn required_one(&self) -> Result<(), SessionError> { self.inner.required_one().await }
}
#[cfg(test)]
pub(super) mod also_empty {}
pub mod production {
    impl MobSessionService for ForgetfulInModule {
        async fn required_one(&self) -> Result<(), SessionError> { self.inner.required_one().await }
    }
}

impl MobSessionService for GoodWrapper {
    async fn required_one(&self) -> Result<(), SessionError> { self.inner.required_one().await }
    async fn defaulted_refusal(&self) -> Result<(), SessionError> { self.inner.defaulted_refusal().await }
    fn plain_default(&self) -> bool { self.inner.plain_default() }
}
'''


RUNTIME_STORE_TRAIT_SRC = '''
pub trait RuntimeStore: RuntimeSessionAuthorityOps + Send + Sync {
    fn session_authority_ops(&self) -> &dyn RuntimeSessionAuthorityOps;
    async fn commit_prepared_session_boundary_with_fence(
        &self,
        _runtime_id: &LogicalRuntimeId,
    ) -> Result<(), RuntimeStoreError> {
        Err(RuntimeStoreError::Unsupported(
            "commit_prepared_session_boundary_with_fence".to_string(),
        ))
    }
    async fn list_runtime_delivery_authorities(
        &self,
    ) -> Result<Vec<(LogicalRuntimeId, RuntimeDeliveryAuthorityRecord)>, RuntimeStoreError> {
        Err(RuntimeStoreError::Unsupported(
            "list_runtime_delivery_authorities".into(),
        ))
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    fn input_persist_ack_loss_fault_for_test(&self) -> Option<InputPersistAckLossFault> {
        None
    }
}

pub trait RuntimeStoreWriteFence: Send + Sync {
    fn unrelated(&self);
}
'''

RUNTIME_STORE_IMPL_SRC = '''
#[async_trait]
impl meerkat_runtime::RuntimeStore for SessionStoreBackedRuntimeStore {
    fn session_authority_ops(&self) -> &dyn meerkat_runtime::store::RuntimeSessionAuthorityOps {
        self.inner.session_authority_ops()
    }
}
'''


HISTORY_EXT_TRAIT_SRC = '''
pub trait SessionServiceHistoryExt: SessionService {
    async fn read_history(&self, id: &SessionId) -> Result<SessionHistoryPage, SessionError>;
    async fn read_instruction_activation_records(
        &self,
        _id: &SessionId,
        _query: crate::InstructionActivationReadQuery,
    ) -> Result<crate::InstructionActivationReadPage, SessionError> {
        Err(SessionError::Unsupported("read_instruction_activation_records".to_string()))
    }
}
'''

HISTORY_EXT_IMPL_SRC = '''
impl meerkat_core::SessionServiceHistoryExt for ForwardingService {
    async fn read_history(&self, id: &SessionId) -> Result<SessionHistoryPage, SessionError> {
        self.inner.read_history(id).await
    }
}
'''


CONTROL_EXT_TRAIT_SRC = '''
pub trait SessionServiceControlExt: SessionService {
    async fn append_system_context(
        &self,
        id: &SessionId,
        req: AppendSystemContextRequest,
    ) -> Result<AppendSystemContextResult, SessionControlError>;
    async fn append_authenticated_system_context(
        &self,
        control: Arc<SystemContextControlRequest>,
    ) -> Result<AppendSystemContextResult, SessionControlError> {
        let _ = control;
        Err(crate::OperationAuthorizationError::Unavailable.into())
    }
}
'''

CONTROL_EXT_IMPL_SRC = '''
impl meerkat_core::service::SessionServiceControlExt for ForwardingService {
    async fn append_system_context(
        &self,
        id: &SessionId,
        req: AppendSystemContextRequest,
    ) -> Result<AppendSystemContextResult, SessionControlError> {
        self.inner.append_system_context(id, req).await
    }
}
'''


class DecoratorConformanceGate(unittest.TestCase):
    def test_runtime_store_spec_is_checked_and_exemption_needs_a_reason(self) -> None:
        spec = next(s for s in gate.TRAIT_SPECS if s.name == "RuntimeStore")
        methods = gate.trait_methods(RUNTIME_STORE_TRAIT_SRC, spec.name)
        # `\b` keeps `RuntimeStoreWriteFence` out of the RuntimeStore trait body.
        # The fixture declares both exempt methods the way meerkat-runtime does,
        # including the test-support fault seam behind
        # `cfg(any(test, feature = "test-support"))`.
        self.assertEqual(
            set(methods),
            {
                "session_authority_ops",
                "commit_prepared_session_boundary_with_fence",
                "list_runtime_delivery_authorities",
                "input_persist_ack_loss_fault_for_test",
            },
        )
        impls = gate.production_impls(RUNTIME_STORE_IMPL_SRC, spec.name)
        self.assertEqual([t for t, _ in impls], ["SessionStoreBackedRuntimeStore"])
        names = impls[0][1]
        # The 0.8.32 defect: the facade lacked the cross-runtime delivery read.
        # The fenced commit and the fault seam are exempt WITH a reason and must
        # not be reported.
        self.assertEqual(
            gate.missing_methods(methods, set(), names, spec.exempt),
            ["list_runtime_delivery_authorities"],
        )
        self.assertIn("commit_prepared_session_boundary_with_fence", spec.exempt)
        self.assertTrue(spec.exempt["commit_prepared_session_boundary_with_fence"].strip())
        self.assertIn("input_persist_ack_loss_fault_for_test", spec.exempt)
        self.assertTrue(spec.exempt["input_persist_ack_loss_fault_for_test"].strip())
        # Without the exemptions both are reported like any other method.
        self.assertEqual(
            gate.missing_methods(methods, set(), names),
            [
                "commit_prepared_session_boundary_with_fence",
                "input_persist_ack_loss_fault_for_test",
                "list_runtime_delivery_authorities",
            ],
        )
        # An exemption naming a method the trait no longer has is itself a
        # failure, alone or next to the real exemptions.
        self.assertEqual(gate.stale_exemptions(methods, {"gone_method": "why"}), ["gone_method"])
        self.assertEqual(
            gate.stale_exemptions(methods, {**spec.exempt, "gone_method": "why"}), ["gone_method"]
        )
        self.assertEqual(gate.stale_exemptions(methods, spec.exempt), [])

    def test_every_spec_is_addressed_by_name(self) -> None:
        self.assertEqual(
            [s.name for s in gate.TRAIT_SPECS],
            [
                "MobSessionService",
                "SessionService",
                "SessionServiceHistoryExt",
                "SessionServiceCommsExt",
                "SessionServiceControlExt",
                "RuntimeStore",
            ],
        )
        for spec in gate.TRAIT_SPECS:
            if spec.name != "RuntimeStore":
                self.assertEqual(spec.exempt, {}, spec.name)

    def test_control_extension_defaulted_refusal_is_checked(self) -> None:
        # The 2026-10-23 scan: the decorators forwarded `append_system_context`
        # but answered the refusing default of the authenticated append.
        spec = next(s for s in gate.TRAIT_SPECS if s.name == "SessionServiceControlExt")
        methods = gate.trait_methods(CONTROL_EXT_TRAIT_SRC, spec.name)
        self.assertEqual(
            set(methods), {"append_system_context", "append_authenticated_system_context"}
        )
        impls = gate.production_impls(CONTROL_EXT_IMPL_SRC, spec.name)
        self.assertEqual([t for t, _ in impls], ["ForwardingService"])
        self.assertEqual(
            gate.missing_methods(methods, set(), impls[0][1], spec.exempt),
            ["append_authenticated_system_context"],
        )

    def test_base_trait_defaulted_method_is_checked(self) -> None:
        # The 0.8.51 defect: the decorators implemented SessionServiceHistoryExt
        # but not its defaulted `read_instruction_activation_records`, so the
        # member's activation read answered `Unsupported` through Meta.
        spec = next(s for s in gate.TRAIT_SPECS if s.name == "SessionServiceHistoryExt")
        methods = gate.trait_methods(HISTORY_EXT_TRAIT_SRC, spec.name)
        self.assertEqual(set(methods), {"read_history", "read_instruction_activation_records"})
        impls = gate.production_impls(HISTORY_EXT_IMPL_SRC, spec.name)
        self.assertEqual([t for t, _ in impls], ["ForwardingService"])
        self.assertEqual(
            gate.missing_methods(methods, set(), impls[0][1], spec.exempt),
            ["read_instruction_activation_records"],
        )

    def test_trait_methods_and_feature_gates(self) -> None:
        methods = gate.trait_methods(TRAIT_SRC)
        self.assertEqual(
            methods,
            {"required_one": None, "defaulted_refusal": None, "live_only": "experimental-gpt-live", "plain_default": None},
        )

    def test_forgetful_production_decorator_fails_and_test_double_is_ignored(self) -> None:
        methods = gate.trait_methods(TRAIT_SRC)
        impls = gate.production_impls(IMPL_SRC)
        self.assertEqual([t for t, _ in impls], ["GoodWrapper", "ForgetfulWrapper"], "the cfg(test) double must not count")
        by_target = dict(impls)
        self.assertEqual(gate.missing_methods(methods, set(), by_target["GoodWrapper"]), [])
        self.assertEqual(
            gate.missing_methods(methods, set(), by_target["ForgetfulWrapper"]),
            ["defaulted_refusal", "plain_default"],
        )

    def test_test_double_in_a_test_module_with_a_visibility_is_ignored(self) -> None:
        # Shared test support is declared `pub(crate) mod ... {`; only
        # `#[cfg(test)]` makes a module a test region, so a plain `pub mod`
        # stays production.
        impls = gate.production_impls(VISIBLE_TEST_MODULES_SRC)
        self.assertEqual([t for t, _ in impls], ["ForgetfulWrapper"], "no cfg(test) double may count")
        methods = gate.trait_methods(TRAIT_SRC)
        self.assertEqual(
            gate.missing_methods(methods, set(), impls[0][1]),
            ["defaulted_refusal", "plain_default"],
        )

    def test_one_line_test_module_does_not_swallow_the_production_code_after_it(self) -> None:
        # A module that closes on its declaration line ends its test region
        # there. Otherwise the incomplete wrapper and the public module right
        # after it would count as test code, and the valid wrapper alone would
        # still satisfy the production-impl count.
        impls = gate.production_impls(ONE_LINE_TEST_MODULES_SRC)
        self.assertEqual(
            [t for t, _ in impls], ["ForgetfulWrapper", "ForgetfulInModule", "GoodWrapper"]
        )
        methods = gate.trait_methods(TRAIT_SRC)
        by_target = dict(impls)
        for target in ("ForgetfulWrapper", "ForgetfulInModule"):
            self.assertEqual(
                gate.missing_methods(methods, set(), by_target[target]),
                ["defaulted_refusal", "plain_default"],
                target,
            )
        self.assertEqual(gate.missing_methods(methods, set(), by_target["GoodWrapper"]), [])
        # Both scanners end each region on the module's own line.
        lines = ONE_LINE_TEST_MODULES_SRC.splitlines()
        for scanner in (gate, authority):
            self.assertEqual(
                [lines[a:b] for a, b in scanner.test_regions(lines)],
                [
                    ["#[cfg(test)]", "pub(crate) mod empty {}"],
                    ["#[cfg(test)]", "pub(super) mod also_empty {}"],
                ],
                scanner.__name__,
            )

    def test_feature_gated_method_counts_only_when_enabled(self) -> None:
        methods = gate.trait_methods(TRAIT_SRC)
        names = {"required_one", "defaulted_refusal", "plain_default"}
        self.assertEqual(gate.missing_methods(methods, set(), names), [])
        self.assertEqual(gate.missing_methods(methods, {"experimental-gpt-live"}, names), ["live_only"])


if __name__ == "__main__":
    unittest.main()
