//! Tool gateway for composing multiple tool dispatchers
//!
//! The [`ToolGateway`] combines multiple tool dispatchers into a single unified
//! dispatcher. This enables composing core tool dispatchers (shell, task, MCP)
//! with infrastructure-provided tools (comms) without coupling them together.
//!
//! ## Availability
//!
//! Tools can have dynamic availability based on runtime conditions. For example,
//! comms tools are only available when peers are configured. This is controlled
//! via the [`Availability`] type.
//!
//! # Example
//!
//! ```ignore
//! use meerkat_core::{ToolGateway, ToolGatewayBuilder, AgentToolDispatcher, Availability};
//!
//! // Compose base dispatcher with conditionally-available comms
//! let gateway = ToolGatewayBuilder::new()
//!     .add_dispatcher(base_dispatcher)
//!     .add_dispatcher_with_availability(
//!         comms_dispatcher,
//!         Availability::when(
//!             "no peers configured",
//!             Arc::new(move || peers_check.try_read().map(|g| g.has_peers()).unwrap_or(false))
//!         )
//!     )
//!     .build()?;
//! ```

use crate::AgentToolDispatcher;
use crate::error::ToolError;
use crate::types::ToolDef;
use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

/// Predicate function type for availability checks.
///
/// Returns `true` if tools should be available, `false` otherwise.
/// Must be `Send + Sync` for use across threads.
///
/// **Important requirements**:
/// - Must be **fast** (no blocking I/O, no heavy computation)
/// - Must be **non-blocking** (use `try_read()` not `read()` for locks)
/// - Should be **deterministic** within a short time window
///
/// Predicates are called multiple times per agent turn (once in `tools()`,
/// once in `dispatch()`), so they must be cheap to evaluate.
pub type AvailabilityCheck = Arc<dyn Fn() -> bool + Send + Sync>;

/// Controls when a set of tools is visible and callable.
///
/// - `Always`: Tools are always available (default for most tools)
/// - `When`: Tools are only available when a predicate returns true
#[derive(Clone, Default)]
pub enum Availability {
    /// Tools are always available.
    #[default]
    Always,
    /// Tools are available when the check returns true.
    When {
        /// The predicate that determines availability.
        check: AvailabilityCheck,
        /// Human-readable reason shown when tools are unavailable.
        reason: String,
    },
}

impl std::fmt::Debug for Availability {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Availability::Always => write!(f, "Availability::Always"),
            Availability::When { reason, .. } => {
                write!(f, "Availability::When {{ reason: {:?} }}", reason)
            }
        }
    }
}

impl Availability {
    /// Create an availability that depends on a runtime check.
    ///
    /// # Arguments
    /// * `reason` - Human-readable reason shown when unavailable (e.g., "no peers configured")
    /// * `check` - Predicate that returns true when tools should be available
    pub fn when(reason: impl Into<String>, check: AvailabilityCheck) -> Self {
        Availability::When {
            check,
            reason: reason.into(),
        }
    }

    /// Returns true if tools are currently available.
    pub fn is_available(&self) -> bool {
        match self {
            Availability::Always => true,
            Availability::When { check, .. } => check(),
        }
    }

    /// Returns the unavailability reason, if tools are unavailable.
    pub fn unavailable_reason(&self) -> Option<&str> {
        match self {
            Availability::Always => None,
            Availability::When { check, reason } => {
                if check() {
                    None
                } else {
                    Some(reason)
                }
            }
        }
    }
}

/// Entry for a dispatcher in the gateway.
struct DispatcherEntry {
    dispatcher: Arc<dyn AgentToolDispatcher>,
    availability: Availability,
}

/// A tool dispatcher that composes multiple dispatchers into one.
///
/// The gateway builds a routing table at construction time, mapping each tool
/// name to its owning dispatcher. This provides O(1) dispatch and catches
/// name collisions early.
///
/// ## Dynamic Visibility
///
/// Some tools may have dynamic availability based on runtime conditions.
/// The gateway handles this by:
/// - Only returning available tools from `tools()`
/// - Returning `ToolError::Unavailable` for hidden tools on dispatch
pub struct ToolGateway {
    /// All registered tool definitions (for collision detection)
    all_tools: Vec<ToolDef>,
    /// Routing table: tool name -> dispatcher entry index
    route: HashMap<String, usize>,
    /// Dispatcher entries with their availability
    entries: Vec<DispatcherEntry>,
}

impl std::fmt::Debug for ToolGateway {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolGateway")
            .field(
                "all_tools",
                &self.all_tools.iter().map(|t| &t.name).collect::<Vec<_>>(),
            )
            .field("routes", &self.route.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl ToolGateway {
    /// Create a new gateway with a base dispatcher and optional overlay.
    ///
    /// Both dispatchers use `Availability::Always`.
    /// For conditional availability, use [`ToolGatewayBuilder`].
    pub fn new(
        base: Arc<dyn AgentToolDispatcher>,
        overlay: Option<Arc<dyn AgentToolDispatcher>>,
    ) -> Result<Self, ToolError> {
        let mut builder = ToolGatewayBuilder::new().add_dispatcher(base);
        if let Some(o) = overlay {
            builder = builder.add_dispatcher(o);
        }
        builder.build()
    }
}

/// Builder for constructing a [`ToolGateway`].
///
/// Use this when you need to compose more than two dispatchers or want
/// explicit control over availability conditions.
pub struct ToolGatewayBuilder {
    dispatchers: Vec<(Arc<dyn AgentToolDispatcher>, Availability)>,
}

impl Default for ToolGatewayBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolGatewayBuilder {
    /// Create a new empty builder.
    pub fn new() -> Self {
        Self {
            dispatchers: Vec::new(),
        }
    }

    /// Add a dispatcher with default availability (always).
    pub fn add_dispatcher(self, dispatcher: Arc<dyn AgentToolDispatcher>) -> Self {
        self.add_dispatcher_with_availability(dispatcher, Availability::Always)
    }

    /// Add a dispatcher with custom availability.
    pub fn add_dispatcher_with_availability(
        mut self,
        dispatcher: Arc<dyn AgentToolDispatcher>,
        availability: Availability,
    ) -> Self {
        self.dispatchers.push((dispatcher, availability));
        self
    }

    /// Optionally add a dispatcher if present.
    pub fn maybe_add_dispatcher(self, dispatcher: Option<Arc<dyn AgentToolDispatcher>>) -> Self {
        match dispatcher {
            Some(d) => self.add_dispatcher(d),
            None => self,
        }
    }

    /// Optionally add a dispatcher with availability if present.
    pub fn maybe_add_dispatcher_with_availability(
        self,
        dispatcher: Option<Arc<dyn AgentToolDispatcher>>,
        availability: Availability,
    ) -> Self {
        match dispatcher {
            Some(d) => self.add_dispatcher_with_availability(d, availability),
            None => self,
        }
    }

    /// Build the gateway, validating that there are no tool name collisions.
    ///
    /// Returns an error if any two dispatchers provide tools with the same name.
    /// All tools are checked for collisions regardless of their availability.
    pub fn build(self) -> Result<ToolGateway, ToolError> {
        let mut route: HashMap<String, usize> = HashMap::new();
        let mut all_tools: Vec<ToolDef> = Vec::new();
        let mut entries: Vec<DispatcherEntry> = Vec::new();

        for (dispatcher, availability) in self.dispatchers {
            let entry_idx = entries.len();

            for t in dispatcher.tools() {
                if route.contains_key(&t.name) {
                    return Err(ToolError::Other(format!(
                        "tool name collision in gateway: '{}'",
                        t.name
                    )));
                }
                route.insert(t.name.clone(), entry_idx);
                all_tools.push(t);
            }

            entries.push(DispatcherEntry {
                dispatcher,
                availability,
            });
        }

        Ok(ToolGateway {
            all_tools,
            route,
            entries,
        })
    }
}

#[async_trait]
impl AgentToolDispatcher for ToolGateway {
    /// Returns only the tools that are currently available.
    ///
    /// Tools with `Availability::When` predicates that return false
    /// are excluded from the returned list.
    ///
    /// **Important**: Availability is evaluated once per dispatcher entry to ensure
    /// consistency - either all tools from a dispatcher are visible or none are.
    /// This prevents partial listings when predicates are evaluated under contention.
    fn tools(&self) -> Vec<ToolDef> {
        // Pre-compute availability for each dispatcher entry once
        // This ensures consistency: all tools from a dispatcher are either visible or hidden
        let entry_available: Vec<bool> = self
            .entries
            .iter()
            .map(|e| e.availability.is_available())
            .collect();

        let mut visible = Vec::with_capacity(self.all_tools.len());

        for tool in &self.all_tools {
            if let Some(&idx) = self.route.get(&tool.name) {
                if entry_available[idx] {
                    visible.push(tool.clone());
                }
            }
        }

        visible
    }

    /// Dispatch a tool call.
    ///
    /// Returns:
    /// - `ToolError::NotFound` if the tool doesn't exist
    /// - `ToolError::Unavailable` if the tool exists but is currently hidden
    /// - The tool result if execution succeeds
    async fn dispatch(&self, name: &str, args: &Value) -> Result<Value, ToolError> {
        let idx = self
            .route
            .get(name)
            .ok_or_else(|| ToolError::not_found(name))?;

        let entry = &self.entries[*idx];

        // Check availability before dispatch
        if let Some(reason) = entry.availability.unavailable_reason() {
            return Err(ToolError::unavailable(name, reason));
        }

        entry.dispatcher.dispatch(name, args).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// A simple mock dispatcher for testing
    struct MockDispatcher {
        tools: Vec<ToolDef>,
        prefix: String,
    }

    impl MockDispatcher {
        fn new(prefix: &str, tool_names: &[&str]) -> Self {
            let tools = tool_names
                .iter()
                .map(|name| ToolDef {
                    name: name.to_string(),
                    description: format!("{prefix} tool: {name}"),
                    input_schema: json!({"type": "object"}),
                })
                .collect();
            Self {
                tools,
                prefix: prefix.to_string(),
            }
        }
    }

    #[async_trait]
    impl AgentToolDispatcher for MockDispatcher {
        fn tools(&self) -> Vec<ToolDef> {
            self.tools.clone()
        }

        async fn dispatch(&self, name: &str, _args: &Value) -> Result<Value, ToolError> {
            if self.tools.iter().any(|t| t.name == name) {
                Ok(json!({"source": self.prefix, "tool": name}))
            } else {
                Err(ToolError::not_found(name))
            }
        }
    }

    #[test]
    fn test_gateway_merges_tools() {
        let base = Arc::new(MockDispatcher::new("base", &["task_create", "task_list"]));
        let overlay = Arc::new(MockDispatcher::new(
            "comms",
            &["send_message", "list_peers"],
        ));

        let gateway = ToolGateway::new(base, Some(overlay)).unwrap();

        let tools = gateway.tools();
        let tool_names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(tool_names.len(), 4);
        assert!(tool_names.contains(&"task_create"));
        assert!(tool_names.contains(&"task_list"));
        assert!(tool_names.contains(&"send_message"));
        assert!(tool_names.contains(&"list_peers"));
    }

    #[test]
    fn test_gateway_no_overlay() {
        let base = Arc::new(MockDispatcher::new("base", &["task_create", "task_list"]));

        let gateway = ToolGateway::new(base, None).unwrap();

        assert_eq!(gateway.tools().len(), 2);
    }

    #[test]
    fn test_gateway_collision_error() {
        let base = Arc::new(MockDispatcher::new(
            "base",
            &["task_create", "send_message"],
        ));
        let overlay = Arc::new(MockDispatcher::new(
            "comms",
            &["send_message", "list_peers"],
        ));

        let result = ToolGateway::new(base, Some(overlay));

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("send_message"));
        assert!(err.to_string().contains("collision"));
    }

    #[tokio::test]
    async fn test_gateway_routes_to_base() {
        let base = Arc::new(MockDispatcher::new("base", &["task_create"]));
        let overlay = Arc::new(MockDispatcher::new("comms", &["send_message"]));

        let gateway = ToolGateway::new(base, Some(overlay)).unwrap();

        let result = gateway.dispatch("task_create", &json!({})).await.unwrap();
        assert_eq!(result["source"], "base");
        assert_eq!(result["tool"], "task_create");
    }

    #[tokio::test]
    async fn test_gateway_routes_to_overlay() {
        let base = Arc::new(MockDispatcher::new("base", &["task_create"]));
        let overlay = Arc::new(MockDispatcher::new("comms", &["send_message"]));

        let gateway = ToolGateway::new(base, Some(overlay)).unwrap();

        let result = gateway.dispatch("send_message", &json!({})).await.unwrap();
        assert_eq!(result["source"], "comms");
        assert_eq!(result["tool"], "send_message");
    }

    #[tokio::test]
    async fn test_gateway_not_found() {
        let base = Arc::new(MockDispatcher::new("base", &["task_create"]));

        let gateway = ToolGateway::new(base, None).unwrap();

        let result = gateway.dispatch("unknown_tool", &json!({})).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ToolError::NotFound { .. }));
    }

    #[test]
    fn test_builder_multiple_dispatchers() {
        let base = Arc::new(MockDispatcher::new("base", &["task_create"]));
        let comms = Arc::new(MockDispatcher::new("comms", &["send_message"]));
        let shell = Arc::new(MockDispatcher::new("shell", &["run_command"]));

        let gateway = ToolGatewayBuilder::new()
            .add_dispatcher(base)
            .add_dispatcher(comms)
            .add_dispatcher(shell)
            .build()
            .unwrap();

        assert_eq!(gateway.tools().len(), 3);
    }

    #[test]
    fn test_availability_always() {
        let avail = Availability::Always;
        assert!(avail.is_available());
        assert!(avail.unavailable_reason().is_none());
    }

    #[test]
    fn test_availability_when_true() {
        let avail = Availability::when("no peers", Arc::new(|| true));
        assert!(avail.is_available());
        assert!(avail.unavailable_reason().is_none());
    }

    #[test]
    fn test_availability_when_false() {
        let avail = Availability::when("no peers configured", Arc::new(|| false));
        assert!(!avail.is_available());
        assert_eq!(avail.unavailable_reason(), Some("no peers configured"));
    }

    #[test]
    fn test_availability_dynamic() {
        let flag = Arc::new(AtomicBool::new(false));
        let flag_clone = flag.clone();
        let avail = Availability::when(
            "no peers",
            Arc::new(move || flag_clone.load(Ordering::SeqCst)),
        );

        assert!(!avail.is_available());

        flag.store(true, Ordering::SeqCst);
        assert!(avail.is_available());

        flag.store(false, Ordering::SeqCst);
        assert!(!avail.is_available());
    }

    #[test]
    fn test_gateway_conditional_visibility() {
        let flag = Arc::new(AtomicBool::new(false));
        let flag_clone = flag.clone();

        let base = Arc::new(MockDispatcher::new("base", &["task_create"]));
        let comms = Arc::new(MockDispatcher::new("comms", &["send_message"]));

        let gateway = ToolGatewayBuilder::new()
            .add_dispatcher(base)
            .add_dispatcher_with_availability(
                comms,
                Availability::when(
                    "no peers",
                    Arc::new(move || flag_clone.load(Ordering::SeqCst)),
                ),
            )
            .build()
            .unwrap();

        // Initially comms tools are hidden
        let tools = gateway.tools();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "task_create");

        // Enable comms
        flag.store(true, Ordering::SeqCst);
        let tools = gateway.tools();
        assert_eq!(tools.len(), 2);

        // Disable again
        flag.store(false, Ordering::SeqCst);
        let tools = gateway.tools();
        assert_eq!(tools.len(), 1);
    }

    #[tokio::test]
    async fn test_gateway_unavailable_dispatch() {
        let flag = Arc::new(AtomicBool::new(false));
        let flag_clone = flag.clone();

        let base = Arc::new(MockDispatcher::new("base", &["task_create"]));
        let comms = Arc::new(MockDispatcher::new("comms", &["send_message"]));

        let gateway = ToolGatewayBuilder::new()
            .add_dispatcher(base)
            .add_dispatcher_with_availability(
                comms,
                Availability::when(
                    "no peers configured",
                    Arc::new(move || flag_clone.load(Ordering::SeqCst)),
                ),
            )
            .build()
            .unwrap();

        // Try to dispatch unavailable tool
        let result = gateway.dispatch("send_message", &json!({})).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, ToolError::Unavailable { .. }));
        assert!(err.to_string().contains("no peers configured"));

        // Enable comms
        flag.store(true, Ordering::SeqCst);
        let result = gateway.dispatch("send_message", &json!({})).await;
        assert!(result.is_ok());
    }

    #[test]
    fn test_collision_detection_ignores_availability() {
        // Collision should be detected even if one dispatcher is conditionally hidden
        let flag = Arc::new(AtomicBool::new(false));
        let flag_clone = flag.clone();

        let base = Arc::new(MockDispatcher::new("base", &["send_message"]));
        let comms = Arc::new(MockDispatcher::new("comms", &["send_message"]));

        let result = ToolGatewayBuilder::new()
            .add_dispatcher(base)
            .add_dispatcher_with_availability(
                comms,
                Availability::when(
                    "no peers",
                    Arc::new(move || flag_clone.load(Ordering::SeqCst)),
                ),
            )
            .build();

        // Should fail even though comms is currently unavailable
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("collision"));
    }

    #[test]
    fn test_availability_debug() {
        let always = Availability::Always;
        assert_eq!(format!("{:?}", always), "Availability::Always");

        let when = Availability::when("test reason", Arc::new(|| true));
        assert!(format!("{:?}", when).contains("test reason"));
    }

    #[test]
    fn test_builder_maybe_add() {
        let base = Arc::new(MockDispatcher::new("base", &["task_create"]));

        // None case
        let gateway = ToolGatewayBuilder::new()
            .add_dispatcher(base.clone())
            .maybe_add_dispatcher(None)
            .build()
            .unwrap();
        assert_eq!(gateway.tools().len(), 1);

        // Some case
        let overlay = Arc::new(MockDispatcher::new("comms", &["send_message"]));
        let gateway = ToolGatewayBuilder::new()
            .add_dispatcher(base)
            .maybe_add_dispatcher(Some(overlay))
            .build()
            .unwrap();
        assert_eq!(gateway.tools().len(), 2);
    }

    #[test]
    fn test_dispatcher_all_or_nothing_visibility() {
        // Verify that all tools from a dispatcher appear/disappear together
        // (no partial visibility within a single dispatcher)
        let flag = Arc::new(AtomicBool::new(false));
        let flag_clone = flag.clone();

        let base = Arc::new(MockDispatcher::new("base", &["task_create"]));
        // Dispatcher with multiple tools
        let comms = Arc::new(MockDispatcher::new(
            "comms",
            &[
                "send_message",
                "send_request",
                "send_response",
                "list_peers",
            ],
        ));

        let gateway = ToolGatewayBuilder::new()
            .add_dispatcher(base)
            .add_dispatcher_with_availability(
                comms,
                Availability::when(
                    "no peers",
                    Arc::new(move || flag_clone.load(Ordering::SeqCst)),
                ),
            )
            .build()
            .unwrap();

        // Initially unavailable - only base tool visible
        let tools = gateway.tools();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "task_create");

        // Enable - ALL comms tools should appear together
        flag.store(true, Ordering::SeqCst);
        let tools = gateway.tools();
        assert_eq!(tools.len(), 5); // 1 base + 4 comms
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"task_create"));
        assert!(names.contains(&"send_message"));
        assert!(names.contains(&"send_request"));
        assert!(names.contains(&"send_response"));
        assert!(names.contains(&"list_peers"));

        // Disable - ALL comms tools should disappear together
        flag.store(false, Ordering::SeqCst);
        let tools = gateway.tools();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "task_create");
    }
}
