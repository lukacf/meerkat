//! Internal bridge-session plumbing for cross-crate access.
//!
//! These functions forward to `pub(crate)` methods on [`MobDefinition`].
//! They are `#[doc(hidden)]` and not part of the public API.

use crate::definition::MobDefinition;

/// Create a minimal implicit delegation mob indexed to the given bridge session.
pub fn implicit_definition(bridge_session_id: &str, model: &str) -> MobDefinition {
    MobDefinition::implicit(bridge_session_id, model)
}

/// Get the owner bridge session index, if set.
pub fn owner_bridge_session_index(def: &MobDefinition) -> Option<&str> {
    def.owner_bridge_session_index()
}

/// Assign bridge-session lookup ownership without changing cleanup semantics.
pub fn set_owner_bridge_session_lookup_index(
    def: &mut MobDefinition,
    bridge_session_id: impl Into<String>,
) {
    def.set_owner_bridge_session_lookup_index(bridge_session_id);
}

/// Clear any bridge-session lookup ownership without changing other flags.
pub fn clear_owner_bridge_session_lookup_index(def: &mut MobDefinition) {
    def.clear_owner_bridge_session_lookup_index();
}

/// Check if the definition has a specific owner bridge session index.
pub fn has_owner_bridge_session_index(def: &MobDefinition, bridge_session_id: &str) -> bool {
    def.has_owner_bridge_session_index(bridge_session_id)
}

/// Check if the definition is indexed to a specific owner bridge session.
pub fn is_indexed_to_owner_bridge_session(def: &MobDefinition, bridge_session_id: &str) -> bool {
    def.is_indexed_to_owner_bridge_session(bridge_session_id)
}

/// Check if the definition is cleanup-scoped to a specific owner bridge session.
pub fn is_cleanup_scoped_to_owner_bridge_session(
    def: &MobDefinition,
    bridge_session_id: &str,
) -> bool {
    def.is_cleanup_scoped_to_owner_bridge_session(bridge_session_id)
}

/// Mark the definition as indexed to and cleanup-scoped by a bridge session.
pub fn mark_owner_bridge_session_indexed(def: &mut MobDefinition, bridge_session_id: &str) {
    def.mark_owner_bridge_session_indexed(bridge_session_id);
}

/// Check if the definition is owned by a specific bridge session.
pub fn is_owned_by_bridge_session(def: &MobDefinition, bridge_session_id: &str) -> bool {
    def.is_owned_by_bridge_session(bridge_session_id)
}

/// Check if the definition is bridge-session-scoped to a specific session.
pub fn is_bridge_session_scoped_to(def: &MobDefinition, bridge_session_id: &str) -> bool {
    def.is_bridge_session_scoped_to(bridge_session_id)
}

/// Mark the definition as bridge-session-scoped.
pub fn mark_bridge_session_scoped(def: &mut MobDefinition, bridge_session_id: &str) {
    def.mark_bridge_session_scoped(bridge_session_id);
}

/// Clear internal lifecycle flags (implicit flag and cleanup policy).
pub fn clear_internal_lifecycle_flags(def: &mut MobDefinition) {
    def.clear_internal_lifecycle_flags();
}
