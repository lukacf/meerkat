//! Block assembly for streaming LLM responses.
//!
//! This module provides the `BlockAssembler` for assembling ordered blocks
//! from streaming LLM events. It tracks global arrival order across all block
//! types and handles interleaved reasoning/tool-use blocks correctly.

use indexmap::IndexMap;
use meerkat_core::{AssistantBlock, ProviderMeta};
use serde_json::value::RawValue;

/// Errors that can occur during stream assembly.
/// Returned to caller, who decides whether to skip, count, or abort.
#[derive(Debug, thiserror::Error)]
pub enum StreamAssemblyError {
    #[error("delta for unknown tool call: {0}")]
    OrphanedToolDelta(String),
    #[error("delta for unknown reasoning block")]
    OrphanedReasoningDelta,
    #[error("duplicate tool call start: {0}")]
    DuplicateToolStart(String),
    #[error("complete event for unknown tool: {0}")]
    UnknownToolComplete(String),
    #[error("finalize args for unknown tool: {0}")]
    UnknownToolFinalize(String),
    #[error("invalid args JSON for tool {id}: {reason}")]
    InvalidArgsJson { id: String, reason: String },
}

/// Typed key into the block list - prevents mixing up different indices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockKey(usize);

/// Represents either a finalized block or a placeholder for one still streaming.
enum BlockSlot {
    Finalized(AssistantBlock),
    Pending,
}

/// Buffer for tool call being assembled from streaming deltas.
/// Key (id) is stored in the map, not duplicated here.
struct ToolCallBuffer {
    name: Option<String>,
    args_json: String,
    block_key: BlockKey,
}

/// Buffer for reasoning block being assembled.
struct ReasoningBuffer {
    text: String,
    block_key: BlockKey,
}

/// Assembler for building ordered blocks from streaming events.
///
/// The assembler tracks **global arrival order** across all block types,
/// not just tool calls. Blocks are ordered by when they *started*, not
/// when they completed.
///
/// # Design
///
/// - `Vec<BlockSlot>` provides stable indices because we never remove elements
/// - `BlockKey(usize)` is a newtype - prevents mixing up different indices
/// - Methods return `Result<(), StreamAssemblyError>` for caller-decided policy
/// - `ToolCallBuffer` does NOT store `id` - it's the map key, avoiding duplication
/// - `Box<RawValue>` for args - no parsing in adapter
pub struct BlockAssembler {
    /// Slots are append-only to preserve start-order.
    slots: Vec<BlockSlot>,
    /// Map from tool call ID to buffer. ID is the key, not stored in value.
    tool_buffers: IndexMap<String, ToolCallBuffer>,
    /// Active reasoning block buffer.
    reasoning_buffer: Option<ReasoningBuffer>,
}

impl BlockAssembler {
    /// Create a new empty assembler.
    pub fn new() -> Self {
        Self {
            slots: Vec::new(),
            tool_buffers: IndexMap::new(),
            reasoning_buffer: None,
        }
    }

    /// Handle a text delta event.
    ///
    /// Text deltas can always succeed - no Result needed.
    /// `meta` is used by Gemini for thoughtSignature on text parts.
    pub fn on_text_delta(&mut self, delta: &str, meta: Option<Box<ProviderMeta>>) {
        if meta.is_none() {
            if let Some(BlockSlot::Finalized(AssistantBlock::Text { text, meta: None })) =
                self.slots.last_mut()
            {
                text.push_str(delta);
                return;
            }
        }
        // Insert new text block
        self.slots.push(BlockSlot::Finalized(AssistantBlock::Text {
            text: delta.into(),
            meta,
        }));
    }

    /// Start a new reasoning block.
    pub fn on_reasoning_start(&mut self) {
        let key = BlockKey(self.slots.len());
        self.slots.push(BlockSlot::Pending);
        self.reasoning_buffer = Some(ReasoningBuffer {
            text: String::new(),
            block_key: key,
        });
    }

    /// Handle a reasoning delta event.
    ///
    /// # Errors
    /// Returns `OrphanedReasoningDelta` if no reasoning block is currently being assembled.
    pub fn on_reasoning_delta(&mut self, delta: &str) -> Result<(), StreamAssemblyError> {
        let buf = self
            .reasoning_buffer
            .as_mut()
            .ok_or(StreamAssemblyError::OrphanedReasoningDelta)?;
        buf.text.push_str(delta);
        Ok(())
    }

    /// Complete the current reasoning block.
    ///
    /// Provider adapter converts raw JSON to typed `ProviderMeta` before calling.
    pub fn on_reasoning_complete(&mut self, meta: Option<Box<ProviderMeta>>) {
        if let Some(buf) = self.reasoning_buffer.take() {
            if let Some(slot) = self.slots.get_mut(buf.block_key.0) {
                *slot = BlockSlot::Finalized(AssistantBlock::Reasoning {
                    text: buf.text,
                    meta,
                });
            }
        }
        // Complete without prior start is silently ignored - provider protocol quirk
    }

    /// Start a new tool call block.
    ///
    /// # Errors
    /// Returns `DuplicateToolStart` if a tool call with the same ID is already being assembled.
    pub fn on_tool_call_start(&mut self, id: String) -> Result<(), StreamAssemblyError> {
        if self.tool_buffers.contains_key(&id) {
            return Err(StreamAssemblyError::DuplicateToolStart(id));
        }
        let key = BlockKey(self.slots.len());
        self.slots.push(BlockSlot::Pending);
        self.tool_buffers.insert(
            id,
            ToolCallBuffer {
                name: None,
                args_json: String::new(),
                block_key: key,
            },
        );
        Ok(())
    }

    /// Handle a tool call delta event.
    ///
    /// # Errors
    /// Returns `OrphanedToolDelta` if no tool call with the given ID is being assembled.
    pub fn on_tool_call_delta(
        &mut self,
        id: &str,
        name: Option<&str>,
        args_delta: &str,
    ) -> Result<(), StreamAssemblyError> {
        let buf = self
            .tool_buffers
            .get_mut(id)
            .ok_or_else(|| StreamAssemblyError::OrphanedToolDelta(id.to_string()))?;
        if let Some(n) = name {
            buf.name = Some(n.into());
        }
        buf.args_json.push_str(args_delta);
        Ok(())
    }

    /// Convert buffered args_json to RawValue. Called before on_tool_call_complete.
    ///
    /// # Errors
    /// - `UnknownToolFinalize` if no buffer exists for this ID (protocol error)
    /// - `InvalidArgsJson` if the buffered JSON is malformed
    pub fn finalize_tool_args(&self, id: &str) -> Result<Box<RawValue>, StreamAssemblyError> {
        let buf = self
            .tool_buffers
            .get(id)
            .ok_or_else(|| StreamAssemblyError::UnknownToolFinalize(id.to_string()))?;

        // Handle empty args (tools with no parameters)
        let args_str = if buf.args_json.is_empty() {
            "{}".to_string()
        } else {
            buf.args_json.clone()
        };

        RawValue::from_string(args_str).map_err(|e| StreamAssemblyError::InvalidArgsJson {
            id: id.to_string(),
            reason: e.to_string(),
        })
    }

    /// Complete a tool call block.
    ///
    /// Provider adapter converts raw JSON to typed `ProviderMeta` before calling.
    ///
    /// # Errors
    /// This method never returns an error. If no prior start exists for the ID,
    /// the tool call is inserted at the end (ordering may be off but we have the data).
    pub fn on_tool_call_complete(
        &mut self,
        id: String,
        name: String,
        args: Box<RawValue>,
        meta: Option<Box<ProviderMeta>>,
    ) -> Result<(), StreamAssemblyError> {
        if let Some((_, _, buf)) = self.tool_buffers.swap_remove_full(&id) {
            if let Some(slot) = self.slots.get_mut(buf.block_key.0) {
                *slot = BlockSlot::Finalized(AssistantBlock::ToolUse {
                    id,
                    name,
                    args,
                    meta,
                });
                return Ok(());
            }
            Ok(())
        } else {
            // No prior start - provider that doesn't emit start events
            // Insert at end; ordering may be off but we have the data
            self.slots
                .push(BlockSlot::Finalized(AssistantBlock::ToolUse {
                    id,
                    name,
                    args,
                    meta,
                }));
            Ok(())
        }
    }

    /// Finalize the assembler and return the ordered blocks.
    ///
    /// Slab iteration is in insertion order, so blocks are returned
    /// in the order they were started.
    pub fn finalize(self) -> Vec<AssistantBlock> {
        self.slots
            .into_iter()
            .filter_map(|slot| match slot {
                BlockSlot::Finalized(block) => Some(block),
                BlockSlot::Pending => None,
            })
            .collect()
    }
}

impl Default for BlockAssembler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    // =========================================================================
    // Text delta coalescing tests
    // =========================================================================

    #[test]
    fn test_text_deltas_coalesce_into_single_block() {
        let mut assembler = BlockAssembler::new();

        assembler.on_text_delta("Hello", None);
        assembler.on_text_delta(" ", None);
        assembler.on_text_delta("World", None);

        let blocks = assembler.finalize();
        assert_eq!(blocks.len(), 1);

        match &blocks[0] {
            AssistantBlock::Text { text, meta } => {
                assert_eq!(text, "Hello World");
                assert!(meta.is_none());
            }
            _ => panic!("Expected Text block"),
        }
    }

    #[test]
    fn test_text_deltas_with_meta_do_not_coalesce() {
        let mut assembler = BlockAssembler::new();

        assembler.on_text_delta("First", None);
        assembler.on_text_delta(
            "Second",
            Some(Box::new(ProviderMeta::Gemini {
                thought_signature: "sig1".to_string(),
            })),
        );
        assembler.on_text_delta("Third", None);

        let blocks = assembler.finalize();
        assert_eq!(blocks.len(), 3);

        // First and second should not coalesce (second has meta)
        match &blocks[0] {
            AssistantBlock::Text { text, .. } => assert_eq!(text, "First"),
            _ => panic!("Expected Text block"),
        }
        match &blocks[1] {
            AssistantBlock::Text { text, meta } => {
                assert_eq!(text, "Second");
                assert!(meta.is_some());
            }
            _ => panic!("Expected Text block"),
        }
        // Third starts new block because previous has meta
        match &blocks[2] {
            AssistantBlock::Text { text, .. } => assert_eq!(text, "Third"),
            _ => panic!("Expected Text block"),
        }
    }

    // =========================================================================
    // Reasoning block tests
    // =========================================================================

    #[test]
    fn test_reasoning_start_delta_complete() {
        let mut assembler = BlockAssembler::new();

        assembler.on_reasoning_start();
        assembler.on_reasoning_delta("Let me think").unwrap();
        assembler.on_reasoning_delta("...").unwrap();
        assembler.on_reasoning_complete(Some(Box::new(ProviderMeta::Anthropic {
            signature: "sig_abc".to_string(),
        })));

        let blocks = assembler.finalize();
        assert_eq!(blocks.len(), 1);

        match &blocks[0] {
            AssistantBlock::Reasoning { text, meta } => {
                assert_eq!(text, "Let me think...");
                match meta.as_deref() {
                    Some(ProviderMeta::Anthropic { signature }) => {
                        assert_eq!(signature, "sig_abc");
                    }
                    _ => panic!("Expected Anthropic meta"),
                }
            }
            _ => panic!("Expected Reasoning block"),
        }
    }

    #[test]
    fn test_reasoning_complete_without_start_is_ignored() {
        let mut assembler = BlockAssembler::new();

        // Complete without prior start should be silently ignored
        assembler.on_reasoning_complete(None);

        let blocks = assembler.finalize();
        assert!(blocks.is_empty());
    }

    #[test]
    fn test_orphaned_reasoning_delta_returns_error() {
        let mut assembler = BlockAssembler::new();

        // Delta without prior start should error
        let result = assembler.on_reasoning_delta("orphan");
        assert!(matches!(
            result,
            Err(StreamAssemblyError::OrphanedReasoningDelta)
        ));
    }

    // =========================================================================
    // Tool call block tests
    // =========================================================================

    #[test]
    fn test_tool_call_start_delta_complete() {
        let mut assembler = BlockAssembler::new();

        assembler.on_tool_call_start("tc_1".to_string()).unwrap();
        assembler
            .on_tool_call_delta("tc_1", Some("read_file"), r#"{"pa"#)
            .unwrap();
        assembler
            .on_tool_call_delta("tc_1", None, r#"th":"#)
            .unwrap();
        assembler
            .on_tool_call_delta("tc_1", None, r#""/tmp/test"}"#)
            .unwrap();

        let args = assembler.finalize_tool_args("tc_1").unwrap();
        assembler
            .on_tool_call_complete("tc_1".to_string(), "read_file".to_string(), args, None)
            .unwrap();

        let blocks = assembler.finalize();
        assert_eq!(blocks.len(), 1);

        match &blocks[0] {
            AssistantBlock::ToolUse {
                id,
                name,
                args,
                meta,
            } => {
                assert_eq!(id, "tc_1");
                assert_eq!(name, "read_file");
                assert!(meta.is_none());
                // Parse args to verify
                let parsed: serde_json::Value = serde_json::from_str(args.get()).unwrap();
                assert_eq!(parsed["path"], "/tmp/test");
            }
            _ => panic!("Expected ToolUse block"),
        }
    }

    #[test]
    fn test_tool_call_with_gemini_meta() {
        let mut assembler = BlockAssembler::new();

        assembler.on_tool_call_start("tc_2".to_string()).unwrap();
        assembler
            .on_tool_call_delta("tc_2", Some("search"), r#"{"q":"test"}"#)
            .unwrap();

        let args = assembler.finalize_tool_args("tc_2").unwrap();
        assembler
            .on_tool_call_complete(
                "tc_2".to_string(),
                "search".to_string(),
                args,
                Some(Box::new(ProviderMeta::Gemini {
                    thought_signature: "gemini_sig".to_string(),
                })),
            )
            .unwrap();

        let blocks = assembler.finalize();
        match &blocks[0] {
            AssistantBlock::ToolUse { meta, .. } => match meta.as_deref() {
                Some(ProviderMeta::Gemini { thought_signature }) => {
                    assert_eq!(thought_signature, "gemini_sig");
                }
                _ => panic!("Expected Gemini meta"),
            },
            _ => panic!("Expected ToolUse block"),
        }
    }

    #[test]
    fn test_tool_call_complete_without_start_inserts_at_end() {
        let mut assembler = BlockAssembler::new();

        // Add some text first
        assembler.on_text_delta("Hello", None);

        // Complete without prior start - should still work
        let args = RawValue::from_string(r#"{"key":"value"}"#.to_string()).unwrap();
        assembler
            .on_tool_call_complete(
                "tc_orphan".to_string(),
                "orphan_tool".to_string(),
                args,
                None,
            )
            .unwrap();

        let blocks = assembler.finalize();
        assert_eq!(blocks.len(), 2);

        // Tool should be at end
        match &blocks[1] {
            AssistantBlock::ToolUse { id, name, .. } => {
                assert_eq!(id, "tc_orphan");
                assert_eq!(name, "orphan_tool");
            }
            _ => panic!("Expected ToolUse block"),
        }
    }

    #[test]
    fn test_duplicate_tool_start_returns_error() {
        let mut assembler = BlockAssembler::new();

        assembler.on_tool_call_start("tc_dup".to_string()).unwrap();
        let result = assembler.on_tool_call_start("tc_dup".to_string());

        assert!(matches!(
            result,
            Err(StreamAssemblyError::DuplicateToolStart(id)) if id == "tc_dup"
        ));
    }

    #[test]
    fn test_orphaned_tool_delta_returns_error() {
        let mut assembler = BlockAssembler::new();

        let result = assembler.on_tool_call_delta("unknown", Some("tool"), "{}");
        assert!(matches!(
            result,
            Err(StreamAssemblyError::OrphanedToolDelta(id)) if id == "unknown"
        ));
    }

    #[test]
    fn test_finalize_tool_args_unknown_id() {
        let assembler = BlockAssembler::new();

        let result = assembler.finalize_tool_args("unknown");
        assert!(matches!(
            result,
            Err(StreamAssemblyError::UnknownToolFinalize(id)) if id == "unknown"
        ));
    }

    #[test]
    fn test_finalize_tool_args_invalid_json() {
        let mut assembler = BlockAssembler::new();

        assembler.on_tool_call_start("tc_bad".to_string()).unwrap();
        assembler
            .on_tool_call_delta("tc_bad", Some("bad_tool"), r#"{"invalid"#)
            .unwrap();

        let result = assembler.finalize_tool_args("tc_bad");
        assert!(matches!(
            result,
            Err(StreamAssemblyError::InvalidArgsJson { id, .. }) if id == "tc_bad"
        ));
    }

    #[test]
    fn test_finalize_tool_args_empty_args() {
        let mut assembler = BlockAssembler::new();

        assembler
            .on_tool_call_start("tc_empty".to_string())
            .unwrap();
        // No deltas - empty args

        let args = assembler.finalize_tool_args("tc_empty").unwrap();
        let parsed: serde_json::Value = serde_json::from_str(args.get()).unwrap();
        assert_eq!(parsed, serde_json::json!({}));
    }

    // =========================================================================
    // Block ordering tests
    // =========================================================================

    #[test]
    fn test_block_ordering_interleaved_events() {
        let mut assembler = BlockAssembler::new();

        // Simulate interleaved stream:
        // 1. Text "Let me help"
        // 2. Reasoning starts
        // 3. Tool call starts
        // 4. Reasoning delta
        // 5. Tool call delta
        // 6. More text (should NOT coalesce with #1 - reasoning is in between)
        // 7. Reasoning complete
        // 8. Tool complete

        assembler.on_text_delta("Let me help. ", None);

        assembler.on_reasoning_start();

        assembler.on_tool_call_start("tc_1".to_string()).unwrap();

        assembler.on_reasoning_delta("thinking...").unwrap();

        assembler
            .on_tool_call_delta("tc_1", Some("search"), r#"{"q":"x"}"#)
            .unwrap();

        assembler.on_text_delta("Done!", None);

        assembler.on_reasoning_complete(Some(Box::new(ProviderMeta::Anthropic {
            signature: "sig".to_string(),
        })));

        let args = assembler.finalize_tool_args("tc_1").unwrap();
        assembler
            .on_tool_call_complete("tc_1".to_string(), "search".to_string(), args, None)
            .unwrap();

        let blocks = assembler.finalize();

        // Expected order: Text, Reasoning, ToolUse, Text
        assert_eq!(blocks.len(), 4);

        assert!(matches!(&blocks[0], AssistantBlock::Text { text, .. } if text == "Let me help. "));
        assert!(
            matches!(&blocks[1], AssistantBlock::Reasoning { text, .. } if text == "thinking...")
        );
        assert!(matches!(&blocks[2], AssistantBlock::ToolUse { name, .. } if name == "search"));
        assert!(matches!(&blocks[3], AssistantBlock::Text { text, .. } if text == "Done!"));
    }

    #[test]
    fn test_multiple_tool_calls_preserve_start_order() {
        let mut assembler = BlockAssembler::new();

        // Start two tool calls
        assembler
            .on_tool_call_start("tc_first".to_string())
            .unwrap();
        assembler
            .on_tool_call_start("tc_second".to_string())
            .unwrap();

        // Complete in reverse order
        assembler
            .on_tool_call_delta("tc_second", Some("tool_b"), r#"{}"#)
            .unwrap();
        let args2 = assembler.finalize_tool_args("tc_second").unwrap();
        assembler
            .on_tool_call_complete("tc_second".to_string(), "tool_b".to_string(), args2, None)
            .unwrap();

        assembler
            .on_tool_call_delta("tc_first", Some("tool_a"), r#"{}"#)
            .unwrap();
        let args1 = assembler.finalize_tool_args("tc_first").unwrap();
        assembler
            .on_tool_call_complete("tc_first".to_string(), "tool_a".to_string(), args1, None)
            .unwrap();

        let blocks = assembler.finalize();

        // Should be in START order, not completion order
        assert_eq!(blocks.len(), 2);
        assert!(matches!(&blocks[0], AssistantBlock::ToolUse { id, .. } if id == "tc_first"));
        assert!(matches!(&blocks[1], AssistantBlock::ToolUse { id, .. } if id == "tc_second"));
    }

    #[test]
    fn test_pending_blocks_filtered_on_finalize() {
        let mut assembler = BlockAssembler::new();

        assembler.on_text_delta("Complete text", None);
        assembler.on_reasoning_start(); // Started but never completed
        assembler
            .on_tool_call_start("tc_incomplete".to_string())
            .unwrap(); // Started but never completed

        let blocks = assembler.finalize();

        // Only the completed text block should remain
        assert_eq!(blocks.len(), 1);
        assert!(matches!(&blocks[0], AssistantBlock::Text { .. }));
    }
}
