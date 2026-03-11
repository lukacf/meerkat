//! RuntimeSessionAdapter — wraps a SessionService with per-session RuntimeDrivers.
//!
//! This adapter lives in meerkat-runtime so that meerkat-session doesn't need
//! to depend on meerkat-runtime. Surfaces use this adapter to get v9 runtime
//! capabilities on top of any SessionService implementation.

use std::collections::HashMap;
use std::sync::Arc;

use meerkat_core::lifecycle::InputId;
use meerkat_core::types::SessionId;
use tokio::sync::RwLock;

use crate::accept::AcceptOutcome;
use crate::driver::ephemeral::EphemeralRuntimeDriver;
use crate::driver::persistent::PersistentRuntimeDriver;
use crate::identifiers::LogicalRuntimeId;
use crate::input::Input;
use crate::input_state::InputState;
use crate::runtime_state::RuntimeState;
use crate::service_ext::{RuntimeMode, SessionServiceRuntimeExt};
use crate::store::RuntimeStore;
use crate::traits::{ResetReport, RetireReport, RuntimeDriver, RuntimeDriverError};

/// Per-session runtime driver entry.
enum DriverEntry {
    Ephemeral(EphemeralRuntimeDriver),
    Persistent(PersistentRuntimeDriver),
}

impl DriverEntry {
    fn as_driver(&self) -> &dyn RuntimeDriver {
        match self {
            DriverEntry::Ephemeral(d) => d,
            DriverEntry::Persistent(d) => d,
        }
    }

    fn as_driver_mut(&mut self) -> &mut dyn RuntimeDriver {
        match self {
            DriverEntry::Ephemeral(d) => d,
            DriverEntry::Persistent(d) => d,
        }
    }
}

/// Wraps a SessionService to provide v9 runtime capabilities.
///
/// Maintains a per-session RuntimeDriver registry. Surfaces create this
/// adapter and use it for both standard SessionService operations (delegated)
/// and v9 runtime operations (via SessionServiceRuntimeExt).
pub struct RuntimeSessionAdapter {
    /// Per-session runtime drivers.
    drivers: RwLock<HashMap<SessionId, DriverEntry>>,
    /// Runtime mode.
    mode: RuntimeMode,
    /// Optional RuntimeStore for persistent drivers.
    store: Option<Arc<dyn RuntimeStore>>,
}

impl RuntimeSessionAdapter {
    /// Create an ephemeral adapter (all sessions use EphemeralRuntimeDriver).
    pub fn ephemeral() -> Self {
        Self {
            drivers: RwLock::new(HashMap::new()),
            mode: RuntimeMode::V9Compliant,
            store: None,
        }
    }

    /// Create a persistent adapter with a RuntimeStore.
    pub fn persistent(store: Arc<dyn RuntimeStore>) -> Self {
        Self {
            drivers: RwLock::new(HashMap::new()),
            mode: RuntimeMode::V9Compliant,
            store: Some(store),
        }
    }

    /// Create a legacy/degraded adapter (no runtime capabilities).
    pub fn legacy() -> Self {
        Self {
            drivers: RwLock::new(HashMap::new()),
            mode: RuntimeMode::LegacyDegraded,
            store: None,
        }
    }

    /// Register a runtime driver for a session.
    pub async fn register_session(&self, session_id: SessionId) {
        let runtime_id = LogicalRuntimeId::new(session_id.to_string());
        let entry = match &self.store {
            Some(store) => {
                DriverEntry::Persistent(PersistentRuntimeDriver::new(runtime_id, store.clone()))
            }
            None => DriverEntry::Ephemeral(EphemeralRuntimeDriver::new(runtime_id)),
        };
        self.drivers.write().await.insert(session_id, entry);
    }

    /// Unregister a session's runtime driver.
    pub async fn unregister_session(&self, session_id: &SessionId) {
        self.drivers.write().await.remove(session_id);
    }
}

#[async_trait::async_trait]
impl SessionServiceRuntimeExt for RuntimeSessionAdapter {
    fn runtime_mode(&self) -> RuntimeMode {
        self.mode
    }

    async fn accept_input(
        &self,
        session_id: &SessionId,
        input: Input,
    ) -> Result<AcceptOutcome, RuntimeDriverError> {
        let mut drivers = self.drivers.write().await;
        let entry = drivers
            .get_mut(session_id)
            .ok_or(RuntimeDriverError::NotReady {
                state: RuntimeState::Destroyed,
            })?;
        entry.as_driver_mut().accept_input(input).await
    }

    async fn runtime_state(
        &self,
        session_id: &SessionId,
    ) -> Result<RuntimeState, RuntimeDriverError> {
        let drivers = self.drivers.read().await;
        let entry = drivers
            .get(session_id)
            .ok_or(RuntimeDriverError::NotReady {
                state: RuntimeState::Destroyed,
            })?;
        Ok(entry.as_driver().runtime_state())
    }

    async fn retire_runtime(
        &self,
        session_id: &SessionId,
    ) -> Result<RetireReport, RuntimeDriverError> {
        let mut drivers = self.drivers.write().await;
        let entry = drivers
            .get_mut(session_id)
            .ok_or(RuntimeDriverError::NotReady {
                state: RuntimeState::Destroyed,
            })?;
        match entry {
            DriverEntry::Ephemeral(d) => d.retire(),
            DriverEntry::Persistent(_) => {
                // For persistent, we need to delegate through the trait
                // but retire() is on EphemeralRuntimeDriver, not the trait
                Err(RuntimeDriverError::Internal(
                    "retire not yet implemented for persistent driver".into(),
                ))
            }
        }
    }

    async fn reset_runtime(
        &self,
        session_id: &SessionId,
    ) -> Result<ResetReport, RuntimeDriverError> {
        let mut drivers = self.drivers.write().await;
        let entry = drivers
            .get_mut(session_id)
            .ok_or(RuntimeDriverError::NotReady {
                state: RuntimeState::Destroyed,
            })?;
        match entry {
            DriverEntry::Ephemeral(d) => d.reset(),
            DriverEntry::Persistent(_) => Err(RuntimeDriverError::Internal(
                "reset not yet implemented for persistent driver".into(),
            )),
        }
    }

    async fn input_state(
        &self,
        session_id: &SessionId,
        input_id: &InputId,
    ) -> Result<Option<InputState>, RuntimeDriverError> {
        let drivers = self.drivers.read().await;
        let entry = drivers
            .get(session_id)
            .ok_or(RuntimeDriverError::NotReady {
                state: RuntimeState::Destroyed,
            })?;
        Ok(entry.as_driver().input_state(input_id).cloned())
    }

    async fn list_active_inputs(
        &self,
        session_id: &SessionId,
    ) -> Result<Vec<InputId>, RuntimeDriverError> {
        let drivers = self.drivers.read().await;
        let entry = drivers
            .get(session_id)
            .ok_or(RuntimeDriverError::NotReady {
                state: RuntimeState::Destroyed,
            })?;
        Ok(entry.as_driver().active_input_ids())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::input::*;
    use chrono::Utc;

    fn make_prompt(text: &str) -> Input {
        Input::Prompt(PromptInput {
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Operator,
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            text: text.into(),
        })
    }

    #[tokio::test]
    async fn ephemeral_adapter_accept_and_query() {
        let adapter = RuntimeSessionAdapter::ephemeral();
        let sid = SessionId::new();
        adapter.register_session(sid.clone()).await;

        let input = make_prompt("hello");
        let outcome = adapter.accept_input(&sid, input).await.unwrap();
        assert!(outcome.is_accepted());

        let state = adapter.runtime_state(&sid).await.unwrap();
        assert_eq!(state, RuntimeState::Idle);

        let active = adapter.list_active_inputs(&sid).await.unwrap();
        assert_eq!(active.len(), 1);
    }

    #[tokio::test]
    async fn persistent_adapter_accept() {
        let store = Arc::new(crate::store::InMemoryRuntimeStore::new());
        let adapter = RuntimeSessionAdapter::persistent(store);
        let sid = SessionId::new();
        adapter.register_session(sid.clone()).await;

        let input = make_prompt("hello");
        let outcome = adapter.accept_input(&sid, input).await.unwrap();
        assert!(outcome.is_accepted());
    }

    #[tokio::test]
    async fn legacy_mode() {
        let adapter = RuntimeSessionAdapter::legacy();
        assert_eq!(adapter.runtime_mode(), RuntimeMode::LegacyDegraded);
    }

    #[tokio::test]
    async fn unregistered_session_errors() {
        let adapter = RuntimeSessionAdapter::ephemeral();
        let sid = SessionId::new();
        let result = adapter.accept_input(&sid, make_prompt("hi")).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn unregister_removes_driver() {
        let adapter = RuntimeSessionAdapter::ephemeral();
        let sid = SessionId::new();
        adapter.register_session(sid.clone()).await;
        adapter.unregister_session(&sid).await;

        let result = adapter.runtime_state(&sid).await;
        assert!(result.is_err());
    }
}
