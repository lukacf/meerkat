//! §11 InputScope — scoping for queue filtering and input targeting.

use serde::{Deserialize, Serialize};

use crate::identifiers::LogicalRuntimeId;
use meerkat_core::lifecycle::InputId;
use meerkat_core::ops::OperationId;

/// Scope for filtering inputs in the queue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "scope_type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum InputScope {
    /// All inputs for a specific runtime.
    Runtime { runtime_id: LogicalRuntimeId },
    /// A specific input by ID.
    Specific { input_id: InputId },
    /// All lifecycle notices for one operation.
    Operation { operation_id: OperationId },
    /// All inputs (global scope).
    All,
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn input_scope_runtime_serde() {
        let scope = InputScope::Runtime {
            runtime_id: LogicalRuntimeId::new("agent-1"),
        };
        let json = serde_json::to_value(&scope).unwrap();
        assert_eq!(json["scope_type"], "runtime");
        let parsed: InputScope = serde_json::from_value(json).unwrap();
        assert_eq!(scope, parsed);
    }

    #[test]
    fn input_scope_specific_serde() {
        let scope = InputScope::Specific {
            input_id: InputId::new(),
        };
        let json = serde_json::to_value(&scope).unwrap();
        assert_eq!(json["scope_type"], "specific");
        let parsed: InputScope = serde_json::from_value(json).unwrap();
        assert_eq!(scope, parsed);
    }

    #[test]
    fn input_scope_all_serde() {
        let scope = InputScope::All;
        let json = serde_json::to_value(&scope).unwrap();
        assert_eq!(json["scope_type"], "all");
        let parsed: InputScope = serde_json::from_value(json).unwrap();
        assert_eq!(scope, parsed);
    }

    #[test]
    fn input_scope_operation_serde() {
        let scope = InputScope::Operation {
            operation_id: OperationId::new(),
        };
        let json = serde_json::to_value(&scope).unwrap();
        assert_eq!(json["scope_type"], "operation");
        let parsed: InputScope = serde_json::from_value(json).unwrap();
        assert_eq!(scope, parsed);
    }
}
