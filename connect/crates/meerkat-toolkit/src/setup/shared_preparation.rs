//! Shared preparation facts in the existing setup owner. A WorkGraph group is
//! supplied by the original invocation producer, never inferred from an agent
//! run ID, a label alone, or a model-selected job. Native grants remain separate.

use super::*;
use meerkat_workgraph::{
    WorkEdgeKind, WorkGraphMachine, WorkGraphSnapshotFilter, WorkItemRef, WorkOwnerKey,
};
mod membership;
pub use membership::{
    SharedPreparationCompletion, SharedPreparationMember, SharedPreparationSelection,
};

/// Exact owner-produced fan-out context. The producer must bind this context
/// and the native Original in one invocation result. Deserialization restores
/// historical facts only; entry must reopen the selected WorkGraph backend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetupWorkContext {
    backend: SetupWorkBackendPin,
    parent: WorkItemRef,
    child: WorkItemRef,
    parent_owner: WorkOwnerKey,
    child_owner: WorkOwnerKey,
    cycle_label: String,
}

impl SetupWorkContext {
    /// The host that actually launched the child supplies this association.
    /// This constructor cannot establish which native invocation ran that child.
    pub fn from_owner(
        backend: SetupWorkBackendPin,
        parent: WorkItemRef,
        child: WorkItemRef,
        parent_owner: WorkOwnerKey,
        child_owner: WorkOwnerKey,
        cycle_label: String,
    ) -> Result<Self, SetupError> {
        let value = Self {
            backend,
            parent,
            child,
            parent_owner,
            child_owner,
            cycle_label,
        };
        value.check_structure()?;
        Ok(value)
    }

    pub fn backend(&self) -> &SetupWorkBackendPin {
        &self.backend
    }
    pub fn parent(&self) -> &WorkItemRef {
        &self.parent
    }
    pub fn child(&self) -> &WorkItemRef {
        &self.child
    }
    pub(crate) fn parent_owner(&self) -> &WorkOwnerKey {
        &self.parent_owner
    }
    pub(crate) fn cycle_label(&self) -> &str {
        &self.cycle_label
    }

    pub(crate) fn check_structure(&self) -> Result<(), SetupError> {
        if self.parent == self.child
            || self.parent.realm_id != self.backend.realm().as_str()
            || self.child.realm_id != self.parent.realm_id
            || &self.parent.namespace != self.backend.namespace()
            || self.child.namespace != self.parent.namespace
            || self.cycle_label.is_empty()
            || self.cycle_label.len() > 128
            || self.cycle_label.trim() != self.cycle_label
            || self.cycle_label.chars().any(char::is_control)
            || WorkOwnerKey::new(self.parent_owner.kind, self.parent_owner.id.clone()).is_err()
            || WorkOwnerKey::new(self.child_owner.kind, self.child_owner.id.clone()).is_err()
        {
            return Err(SetupError::InvalidSelection);
        }
        Ok(())
    }

    /// One native snapshot checks the actual parent, child, ownership and edge.
    /// The cycle label only narrows that read; it supplies no group authority.
    /// Absence, terminality or unavailable storage refuses sharing. If the
    /// bounded result excludes either required row or edge, sharing refuses.
    /// This first live-host slice is bounded to the returned 64 cycle rows;
    /// it does not qualify arbitrary-size fan-out groups.
    /// This read is not an entry lease; native admission and cancellation fencing
    /// still run at the selected effect boundary.
    pub async fn check_current(&self, backend: &SetupWorkBackend) -> Result<(), SetupError> {
        self.check_structure()?;
        if backend.pin() != &self.backend {
            return Err(SetupError::WrongScope);
        }
        let graph = backend
            .service()
            .snapshot(WorkGraphSnapshotFilter {
                realm_id: Some(self.parent.realm_id.clone()),
                namespace: Some(self.parent.namespace.clone()),
                labels: vec![self.cycle_label.clone()],
                include_terminal: true,
                limit: Some(64),
                ..Default::default()
            })
            .await?;
        if graph.realm_id != self.parent.realm_id
            || graph.namespace.as_ref() != Some(&self.parent.namespace)
            || graph.all_namespaces
        {
            return Err(SetupError::WrongScope);
        }
        let parent = graph
            .items
            .iter()
            .find(|item| item.id == self.parent.item_id)
            .ok_or(SetupError::Denied)?;
        let child = graph
            .items
            .iter()
            .find(|item| item.id == self.child.item_id)
            .ok_or(SetupError::Denied)?;
        if parent.owner.as_ref().map(|owner| &owner.key) != Some(&self.parent_owner)
            || child.owner.as_ref().map(|owner| &owner.key) != Some(&self.child_owner)
            || WorkGraphMachine::classify_terminality(parent)?
            || WorkGraphMachine::classify_terminality(child)?
            || !graph.edges.iter().any(|edge| {
                edge.realm_id == self.parent.realm_id
                    && edge.namespace == self.parent.namespace
                    && edge.kind == WorkEdgeKind::Parent
                    && edge.from_id == self.child.item_id
                    && edge.to_id == self.parent.item_id
            })
        {
            return Err(SetupError::Denied);
        }
        Ok(())
    }
}
