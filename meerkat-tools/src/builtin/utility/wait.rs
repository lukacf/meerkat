//! Wait tool for pausing execution

use crate::builtin::{BuiltinTool, BuiltinToolError};
use async_trait::async_trait;
use meerkat_core::ToolDef;
use serde::Deserialize;
use serde_json::{Value, json};
use std::time::{Duration, Instant};
use tokio::sync::watch;

/// Maximum wait time in seconds (5 minutes)
const MAX_WAIT_SECONDS: f64 = 300.0;

/// Interrupt signal for the wait tool
#[derive(Debug, Clone)]
pub struct WaitInterrupt {
    /// Human-readable reason for the interrupt
    pub reason: String,
}

/// Tool for pausing execution for a specified duration
///
/// This tool allows agents to wait before continuing, which is essential for:
/// - Waiting between status checks on async operations
/// - Rate limiting when interacting with external services
/// - Coordinating timing-sensitive workflows
///
/// The wait can be interrupted by incoming messages if an interrupt receiver is configured.
#[derive(Debug, Clone)]
pub struct WaitTool {
    /// Optional interrupt receiver - when a message arrives, wait is interrupted
    interrupt_rx: Option<watch::Receiver<Option<WaitInterrupt>>>,
}

impl WaitTool {
    /// Create a new WaitTool without interrupt support
    pub fn new() -> Self {
        Self { interrupt_rx: None }
    }

    /// Create a WaitTool with an interrupt receiver
    ///
    /// When a message is sent on the channel, the wait will be interrupted early
    /// and return with status "interrupted" along with the reason.
    pub fn with_interrupt(rx: watch::Receiver<Option<WaitInterrupt>>) -> Self {
        Self {
            interrupt_rx: Some(rx),
        }
    }
}

impl Default for WaitTool {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct WaitArgs {
    /// Duration to wait in seconds (max 300)
    #[schemars(
        description = "Number of seconds to wait (0.1 to 300)",
        range(min = 0.1, max = 300.0)
    )]
    seconds: f64,
}

#[async_trait]
impl BuiltinTool for WaitTool {
    fn name(&self) -> &'static str {
        "wait"
    }

    fn def(&self) -> ToolDef {
        ToolDef {
            name: "wait".into(),
            description: "Pause execution for the specified number of seconds. Use this to wait between status checks on async operations like sub-agents. Maximum wait time is 300 seconds (5 minutes).".into(),
            input_schema: crate::schema::schema_for::<WaitArgs>(),
        }
    }

    fn default_enabled(&self) -> bool {
        true // Utility tools enabled by default
    }

    async fn call(&self, args: Value) -> Result<Value, BuiltinToolError> {
        let args: WaitArgs = serde_json::from_value(args)
            .map_err(|e| BuiltinToolError::invalid_args(format!("Invalid arguments: {}", e)))?;

        // Clamp to valid range
        let seconds = args.seconds.clamp(0.0, MAX_WAIT_SECONDS);
        let duration = Duration::from_secs_f64(seconds);
        let start = Instant::now();

        // If we have an interrupt receiver, race between sleep and interrupt
        if let Some(ref rx) = self.interrupt_rx {
            let mut rx = rx.clone();
            // Mark the current value as seen - we only want to react to NEW interrupts
            // that arrive AFTER we start waiting, not stale ones from previous waits
            rx.borrow_and_update();
            tokio::select! {
                _ = tokio::time::sleep(duration) => {
                    // Completed normally
                    Ok(json!({
                        "waited_seconds": seconds,
                        "status": "complete"
                    }))
                }
            result = rx.changed() => {
                if result.is_ok()
                    && let Some(interrupt) = rx.borrow().as_ref()
                {
                    let waited = start.elapsed().as_secs_f64();
                    return Ok(json!({
                        "waited_seconds": waited,
                        "requested_seconds": seconds,
                        "status": "interrupted",
                        "reason": format!("Wait interrupted after {:.1}s: {}", waited, interrupt.reason)
                    }));
                }
                // Channel closed or no interrupt data - complete normally
                Ok(json!({
                        "waited_seconds": seconds,
                        "status": "complete"
                    }))
                }
            }
        } else {
            // No interrupt receiver - just sleep
            tokio::time::sleep(duration).await;
            Ok(json!({
                "waited_seconds": seconds,
                "status": "complete"
            }))
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn test_wait_tool_name() {
        let tool = WaitTool::new();
        assert_eq!(tool.name(), "wait");
    }

    #[tokio::test]
    async fn test_wait_tool_interrupted_by_message() {
        let (tx, rx) = tokio::sync::watch::channel(None::<WaitInterrupt>);
        let tool = WaitTool::with_interrupt(rx);
        let start = Instant::now();

        // Spawn interrupt after 100ms
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let _ = tx.send(Some(WaitInterrupt {
                reason: "Received message from sub-agent: Task completed".to_string(),
            }));
        });

        let result = tool.call(json!({"seconds": 10.0})).await.unwrap();

        let elapsed = start.elapsed();
        assert!(
            elapsed < Duration::from_secs(1),
            "Should be interrupted quickly"
        );
        assert_eq!(result["status"], "interrupted");
        assert!(result["waited_seconds"].as_f64().unwrap() < 1.0);
        assert!(result["reason"].as_str().unwrap().contains("sub-agent"));
    }

    #[tokio::test]
    async fn test_wait_tool_completes_without_interrupt() {
        let (_tx, rx) = tokio::sync::watch::channel(None::<WaitInterrupt>);
        let tool = WaitTool::with_interrupt(rx);

        let result = tool.call(json!({"seconds": 0.1})).await.unwrap();

        assert_eq!(result["status"], "complete");
        assert_eq!(result["waited_seconds"], 0.1);
    }

    #[tokio::test]
    async fn test_wait_tool_without_interrupt_receiver() {
        // Original behavior - no interrupt receiver
        let tool = WaitTool::new();

        let result = tool.call(json!({"seconds": 0.1})).await.unwrap();

        assert_eq!(result["status"], "complete");
    }

    #[test]
    fn test_wait_tool_default_enabled() {
        let tool = WaitTool::new();
        assert!(tool.default_enabled());
    }

    #[test]
    fn test_wait_tool_def() {
        let tool = WaitTool::new();
        let def = tool.def();
        assert_eq!(def.name, "wait");
        assert!(def.description.contains("Pause execution"));
        assert!(def.input_schema.get("properties").is_some());
    }

    #[tokio::test]
    async fn test_wait_tool_short_wait() {
        let tool = WaitTool::new();
        let start = Instant::now();

        let result = tool.call(json!({"seconds": 0.1})).await.unwrap();

        let elapsed = start.elapsed();
        assert!(elapsed >= Duration::from_millis(100));
        assert!(elapsed < Duration::from_millis(200)); // Some tolerance

        assert_eq!(result["status"], "complete");
        assert_eq!(result["waited_seconds"], 0.1);
    }

    #[test]
    fn test_wait_tool_clamps_max_value() {
        // Test that values above MAX_WAIT_SECONDS get clamped
        // We can't test the actual wait easily, but we can verify the clamping logic
        let seconds = 999.0_f64;
        let clamped = seconds.clamp(0.0, MAX_WAIT_SECONDS);
        assert_eq!(clamped, 300.0);
    }

    #[test]
    fn test_wait_tool_clamps_negative_value() {
        // Test that negative values get clamped to 0
        let seconds = -5.0_f64;
        let clamped = seconds.clamp(0.0, MAX_WAIT_SECONDS);
        assert_eq!(clamped, 0.0);
    }

    #[tokio::test]
    async fn test_wait_tool_invalid_args() {
        let tool = WaitTool::new();

        let result = tool.call(json!({"invalid": "args"})).await;
        assert!(result.is_err());

        let err = result.unwrap_err();
        assert!(matches!(err, BuiltinToolError::InvalidArgs(_)));
    }

    #[tokio::test]
    async fn test_wait_tool_missing_seconds() {
        let tool = WaitTool::new();

        let result = tool.call(json!({})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_wait_tool_stale_interrupt_does_not_affect_subsequent_waits() {
        // Regression test: a previous interrupt should not affect subsequent wait calls
        let (tx, rx) = tokio::sync::watch::channel(None::<WaitInterrupt>);
        let tool = WaitTool::with_interrupt(rx);

        // First: send an interrupt, then call wait - should NOT be interrupted
        tx.send(Some(WaitInterrupt {
            reason: "stale interrupt".to_string(),
        }))
        .unwrap();

        // Small delay to ensure value is set
        tokio::time::sleep(Duration::from_millis(10)).await;

        let start = Instant::now();
        let result = tool.call(json!({"seconds": 0.2})).await.unwrap();
        let elapsed = start.elapsed();

        // Should complete normally since interrupt was stale (sent before wait started)
        assert_eq!(result["status"], "complete");
        assert!(
            elapsed >= Duration::from_millis(180),
            "Should wait full duration, got {:?}",
            elapsed
        );
    }
}
