//! Parent tool scope snapshot for inheriting tool visibility into mob children.

use meerkat_core::tool_scope::ToolFilter;
use meerkat_core::types::ToolDef;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::Arc;

/// Snapshot of a parent agent's visible tools at child spawn time.
///
/// Captures the tool names and definitions that were visible to the parent,
/// allowing child sessions to inherit an appropriate base filter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParentToolScopeSnapshot {
    /// Names of tools that were visible to the parent at capture time.
    pub visible_tool_names: HashSet<String>,
    /// Full definitions of visible tools (for child reference/filtering).
    pub visible_tool_defs: Vec<ToolDef>,
    /// When this snapshot was taken.
    pub captured_at: chrono::DateTime<chrono::Utc>,
}

impl ParentToolScopeSnapshot {
    /// Convert this snapshot into a `ToolFilter::Allow` containing the visible tool names.
    pub fn to_tool_filter(&self) -> ToolFilter {
        ToolFilter::Allow(self.visible_tool_names.clone())
    }

    /// Build a tool filter with additional allow/deny overlays applied on top
    /// of the snapshot's visible set.
    ///
    /// - `allow_overlay`: if `Some`, intersects with the snapshot's visible set
    /// - `deny_overlay`: names to remove from visibility
    pub fn with_overlays(
        &self,
        allow_overlay: Option<&HashSet<String>>,
        deny_overlay: Option<&HashSet<String>>,
    ) -> ToolFilter {
        let mut names = self.visible_tool_names.clone();

        if let Some(allow) = allow_overlay {
            names = names.intersection(allow).cloned().collect();
        }

        if let Some(deny) = deny_overlay {
            for name in deny {
                names.remove(name);
            }
        }

        ToolFilter::Allow(names)
    }

    /// Create a snapshot from a set of visible tool definitions.
    pub fn from_tools(tools: &[Arc<ToolDef>]) -> Self {
        Self {
            visible_tool_names: tools.iter().map(|t| t.name.clone()).collect(),
            visible_tool_defs: tools.iter().map(|t| (**t).clone()).collect(),
            captured_at: chrono::Utc::now(),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use serde_json;
    use std::collections::HashSet;

    fn make_tool(name: &str) -> Arc<ToolDef> {
        Arc::new(ToolDef {
            name: name.to_string(),
            description: format!("{name} tool"),
            input_schema: serde_json::json!({"type": "object"}),
            provenance: None,
        })
    }

    #[test]
    fn roundtrip_serialization() {
        let tools = vec![make_tool("read"), make_tool("write")];
        let snapshot = ParentToolScopeSnapshot::from_tools(&tools);

        let json = serde_json::to_string(&snapshot).unwrap();
        let parsed: ParentToolScopeSnapshot = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.visible_tool_names, snapshot.visible_tool_names);
        assert_eq!(parsed.visible_tool_defs.len(), 2);
    }

    #[test]
    fn to_tool_filter_produces_allow() {
        let tools = vec![make_tool("a"), make_tool("b")];
        let snapshot = ParentToolScopeSnapshot::from_tools(&tools);
        let filter = snapshot.to_tool_filter();

        match filter {
            ToolFilter::Allow(names) => {
                assert!(names.contains("a"));
                assert!(names.contains("b"));
                assert_eq!(names.len(), 2);
            }
            other => panic!("expected Allow, got {other:?}"),
        }
    }

    #[test]
    fn with_overlays_intersects_allow() {
        let tools = vec![make_tool("a"), make_tool("b"), make_tool("c")];
        let snapshot = ParentToolScopeSnapshot::from_tools(&tools);

        let allow: HashSet<String> = ["b", "c", "d"].iter().copied().map(String::from).collect();
        let filter = snapshot.with_overlays(Some(&allow), None);

        match filter {
            ToolFilter::Allow(names) => {
                assert!(names.contains("b"));
                assert!(names.contains("c"));
                assert!(!names.contains("a")); // not in overlay
                assert!(!names.contains("d")); // not in snapshot
            }
            other => panic!("expected Allow, got {other:?}"),
        }
    }

    #[test]
    fn with_overlays_removes_deny() {
        let tools = vec![make_tool("a"), make_tool("b"), make_tool("c")];
        let snapshot = ParentToolScopeSnapshot::from_tools(&tools);

        let deny: HashSet<String> = ["b"].iter().copied().map(String::from).collect();
        let filter = snapshot.with_overlays(None, Some(&deny));

        match filter {
            ToolFilter::Allow(names) => {
                assert!(names.contains("a"));
                assert!(names.contains("c"));
                assert!(!names.contains("b"));
            }
            other => panic!("expected Allow, got {other:?}"),
        }
    }

    #[test]
    fn with_overlays_both() {
        let tools = vec![make_tool("a"), make_tool("b"), make_tool("c")];
        let snapshot = ParentToolScopeSnapshot::from_tools(&tools);

        let allow: HashSet<String> = ["a", "b"].iter().copied().map(String::from).collect();
        let deny: HashSet<String> = ["b"].iter().copied().map(String::from).collect();
        let filter = snapshot.with_overlays(Some(&allow), Some(&deny));

        match filter {
            ToolFilter::Allow(names) => {
                assert!(names.contains("a"));
                assert!(!names.contains("b"));
                assert!(!names.contains("c"));
                assert_eq!(names.len(), 1);
            }
            other => panic!("expected Allow, got {other:?}"),
        }
    }

    #[test]
    fn from_tools_captures_names_and_defs() {
        let tools = vec![make_tool("x"), make_tool("y")];
        let snapshot = ParentToolScopeSnapshot::from_tools(&tools);

        assert_eq!(snapshot.visible_tool_names.len(), 2);
        assert_eq!(snapshot.visible_tool_defs.len(), 2);
        assert!(snapshot.visible_tool_names.contains("x"));
        assert!(snapshot.visible_tool_names.contains("y"));
    }
}
