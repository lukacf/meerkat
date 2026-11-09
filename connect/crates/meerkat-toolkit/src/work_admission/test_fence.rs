//! Test authority only. Production has no default allow implementation.
use super::*;
use meerkat_workgraph::{WorkAdmission, WorkGraphAdmissionFence};
use std::sync::RwLock;

pub(crate) struct TestAdmissionFence {
    pub(crate) allowed: Arc<RwLock<bool>>,
    pub(crate) scope: HostWorkGraphScope,
    pub(crate) admission: WorkAdmission,
}
impl WorkGraphAdmissionFence for TestAdmissionFence {
    fn execute_if_current(
        &self,
        realm: &str,
        namespace: &WorkNamespace,
        admission: &WorkAdmission,
        operation: Box<dyn FnOnce() -> Result<(), WorkGraphError> + '_>,
    ) -> Result<(), WorkGraphError> {
        let allowed = self.allowed.read().unwrap();
        if !*allowed
            || realm != self.scope.realm.as_str()
            || namespace != &self.scope.namespace
            || admission != &self.admission
        {
            return Err(WorkGraphError::Conflict(
                "fixture authority revoked or substituted".into(),
            ));
        }
        operation()
    }
}
