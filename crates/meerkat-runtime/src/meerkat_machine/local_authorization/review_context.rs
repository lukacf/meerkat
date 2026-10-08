//! Optional R2 source material through the existing native accepted-work owner.
//! No transcript reconstruction, serialized permission, or additional registry.

use std::io::Write;
use std::sync::Arc;

use meerkat_core::approval::review::{
    MAX_REVIEW_CONTEXT_BYTES, ReviewContextMaterial, ReviewOperationAttribution,
};
use meerkat_core::authorization::{
    AuthorizationOperation, OperationAuthorizationError, OperationAuthorizationFacts,
    OperationObservation, OperationObservedOutcome, OperationReviewTier,
    PreparedAuthorizationBinding, SourceAuthorizationFacts, SourceAuthorizationTarget,
    SourceAuthorizationUse, WorkAuthorization,
};
use meerkat_core::lifecycle::CoreRenderable;
use meerkat_core::types::SystemNoticeBlock;
use meerkat_core::{ContentBlock, ContentInput};
use serde::Serialize;

use super::{NativeRunAuthorization, OperationExecutionScope, malformed};
use crate::input::Input;
use crate::meerkat_machine::DriverEntry;

#[derive(Serialize)]
struct OriginalProjection<'a> {
    input_id: &'a meerkat_core::InputId,
    // Admission established this exact association. It is historical input
    // attribution, not current account policy or a grant from the reviewer.
    authenticated_association:
        &'a meerkat_authorization_contracts::work_association::InputAuthorityAssociation,
    input: &'a Input,
}

struct BoundedProjection(Vec<u8>);

impl Write for BoundedProjection {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > MAX_REVIEW_CONTEXT_BYTES.saturating_sub(self.0.len()) {
            return Err(std::io::Error::other("review context exceeds its bound"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl NativeRunAuthorization {
    pub(super) async fn read_native_review_context(
        &self,
        candidate: &PreparedAuthorizationBinding,
        attribution: Option<&ReviewOperationAttribution>,
    ) -> Result<ReviewContextMaterial, OperationAuthorizationError> {
        let unavailable = || OperationAuthorizationError::Unavailable;
        self.owner.check_current_membership(candidate)?;
        let candidate_check = self.prepare(candidate)?;
        let OperationExecutionScope::RuntimeInput {
            owner_session_id,
            runtime_epoch_id,
            ..
        } = &self.run.execution_scope
        else {
            return Err(malformed().into());
        };
        if self.originals.is_empty() {
            return Err(unavailable());
        }
        let machine = self.run.machine.upgrade().ok_or_else(unavailable)?;
        let driver = {
            let sessions = machine.sessions.read().await;
            self.owner.check_current_membership(candidate)?;
            let entry = sessions.get(owner_session_id).ok_or_else(unavailable)?;
            if entry.epoch_id != *runtime_epoch_id {
                return Err(malformed().into());
            }
            Arc::clone(&entry.driver)
        };
        let mut projection = BoundedProjection(b"{\"original_inputs\":[".to_vec());
        let mut sources = Vec::with_capacity(self.originals.len());
        for (index, original) in self.originals.iter().enumerate() {
            let facts = OperationAuthorizationFacts {
                operation_id: meerkat_core::OperationId::new(),
                execution_scope: self.run.execution_scope.clone(),
                run_id: Some(self.run.domain_run_id.clone()),
                context_revision: candidate.facts().context_revision.clone(),
                operation: AuthorizationOperation::Source(SourceAuthorizationFacts {
                    target: SourceAuthorizationTarget::RuntimeInput {
                        owner_session_id: owner_session_id.clone(),
                        runtime_epoch_id: runtime_epoch_id.clone(),
                        input_id: original.input_id().clone(),
                    },
                    usage: SourceAuthorizationUse::Read,
                }),
            };
            let source = match attribution {
                Some(attribution) => attribution.context_read_binding(candidate, self, facts)?,
                None => PreparedAuthorizationBinding::new(facts),
            };
            // Every source is an independent permission under every actual
            // original's policy. The reviewed tool's permission grants no read.
            let check = self.prepare(&source)?;
            if check.review_tier() != OperationReviewTier::R1 {
                // Recursively reviewing context hydration is unsupported. It
                // never enters unchecked or escalates the entire native run.
                return Err(unavailable());
            }
            check.check_current(&source)?;
            check.observe(&source, OperationObservation::Entry)?;
            // Entry records an attempted read, not disclosure or completion.
            // No custom observation/policy callback runs under driver custody.
            let read = {
                let locked = driver.lock().await;
                (|| {
                    self.owner.check_current_membership(candidate)?;
                    candidate_check.check_current(candidate)?;
                    check.check_current(&source)?;
                    if !Arc::ptr_eq(&locked.shared_dsl_authority(), &self.run.authority) {
                        return Err(malformed().into());
                    }
                    let ledger = match &*locked {
                        DriverEntry::Ephemeral(driver) => driver.ledger(),
                        DriverEntry::Persistent(driver) => driver.inner_ref().ledger(),
                    };
                    let row = ledger.get(original.input_id()).ok_or_else(unavailable)?;
                    let input = row.persisted_input.as_ref().ok_or_else(unavailable)?;
                    if &row.input_id != original.input_id()
                        || input.id() != original.input_id()
                        || !text_material_is_complete(input)
                    {
                        return Err(unavailable());
                    }
                    if index != 0 {
                        projection.write_all(b",").map_err(|_| unavailable())?;
                    }
                    serde_json::to_writer(
                        &mut projection,
                        &OriginalProjection {
                            input_id: original.input_id(),
                            authenticated_association: original.association(),
                            input,
                        },
                    )
                    .map_err(|_| unavailable())?;
                    // Serialization has already enforced the byte limit, so
                    // replay verification cannot allocate an unbounded payload.
                    original.verify_replay(input).map_err(|_| unavailable())?;
                    Ok::<(), OperationAuthorizationError>(())
                })()
            };
            check.observe(
                &source,
                OperationObservation::Outcome(if read.is_ok() {
                    OperationObservedOutcome::SourceReadMaterialized
                } else {
                    OperationObservedOutcome::SourceReadUnavailable
                }),
            )?;
            read?;
            sources.push((source, check));
        }
        projection.write_all(b"]}").map_err(|_| unavailable())?;
        let text = String::from_utf8(projection.0).map_err(|_| unavailable())?;
        let material = ReviewContextMaterial::from_text(text).map_err(|_| unavailable())?;
        // All callbacks have returned. Final checks never reprepare, call app
        // policy, or await, and include complete selected-batch membership.
        self.owner.check_current_membership(candidate)?;
        candidate_check.check_current(candidate)?;
        for (source, check) in &sources {
            check.check_current(source)?;
        }
        Ok(material)
    }
}

fn text_blocks(blocks: &[ContentBlock]) -> bool {
    if blocks.len() > MAX_REVIEW_CONTEXT_BYTES {
        return false;
    }
    blocks.iter().all(|block| match block {
        ContentBlock::Text { .. } | ContentBlock::SkillContext { .. } => true,
        // Structured serialization canonicalizes by allocating before Write.
        // Bound its raw parse input as well as the eventual encoded output.
        ContentBlock::Structured { data } => data.get().len() <= MAX_REVIEW_CONTEXT_BYTES,
        _ => false,
    })
}

fn text_content(content: &ContentInput) -> bool {
    match content {
        ContentInput::Text(_) => true,
        ContentInput::Blocks(blocks) => text_blocks(blocks),
    }
}

fn text_notice(block: &SystemNoticeBlock) -> bool {
    match block {
        SystemNoticeBlock::Comms { content, .. }
        | SystemNoticeBlock::ExternalEvent { content, .. } => text_blocks(content),
        SystemNoticeBlock::ToolConfig { .. }
        | SystemNoticeBlock::Mcp { .. }
        | SystemNoticeBlock::BackgroundJob { .. }
        | SystemNoticeBlock::Auth { .. }
        | SystemNoticeBlock::RuntimeNotice { .. }
        | SystemNoticeBlock::ToolProcessInterrupted { .. } => true,
        // Unknown/future variants need an explicit completeness decision.
        _ => false,
    }
}

fn text_renderable(content: &CoreRenderable) -> bool {
    match content {
        CoreRenderable::Text { .. } | CoreRenderable::Json { .. } => true,
        CoreRenderable::Blocks { blocks } => text_blocks(blocks),
        CoreRenderable::SystemNotice { blocks, .. } => {
            blocks.len() <= MAX_REVIEW_CONTEXT_BYTES && blocks.iter().all(text_notice)
        }
        // A URI is not the referenced material. This reader does not hydrate
        // external references or model media, or reinterpret an unknown type.
        _ => false,
    }
}

fn text_material_is_complete(input: &Input) -> bool {
    // Images/video must be separately read and sent as actual model content.
    // Encoding blob references or media bytes in JSON would not review them.
    match input {
        Input::Prompt(prompt) => {
            text_content(&prompt.content)
                && prompt.injected_context.iter().all(text_content)
                && prompt.typed_turn_appends.len() <= MAX_REVIEW_CONTEXT_BYTES
                && prompt
                    .typed_turn_appends
                    .iter()
                    .all(|append| text_renderable(&append.content))
        }
        Input::Peer(peer) => {
            text_content(&peer.content) && peer.injected_context.iter().all(text_content)
        }
        Input::FlowStep(step) => text_content(&step.content),
        Input::ExternalEvent(event) => event.blocks.as_deref().is_none_or(text_blocks),
        Input::Continuation(continuation) => continuation
            .turn_append
            .as_ref()
            .is_none_or(|append| text_renderable(&append.content)),
        Input::Operation(_) => false,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use meerkat_core::lifecycle::{ConversationAppend, ConversationAppendRole, CoreRenderable};
    use meerkat_core::types::{SystemNoticeBlock, SystemNoticeKind};

    fn typed_append(content: CoreRenderable) -> ConversationAppend {
        ConversationAppend {
            runtime_source: None,
            role: ConversationAppendRole::SystemNotice,
            content,
            identity: None,
        }
    }

    #[test]
    fn retained_text_appends_are_complete_context_for_prompts_and_continuations() {
        let mut prompt = crate::input::PromptInput::new("look at the completed job", None);
        prompt.typed_turn_appends = vec![
            typed_append(CoreRenderable::Text {
                text: "plain notice".into(),
            }),
            typed_append(CoreRenderable::Json {
                value: serde_json::json!({"result": "ready"}),
            }),
            typed_append(CoreRenderable::Blocks {
                blocks: vec![ContentBlock::Text {
                    text: "block notice".into(),
                }],
            }),
            typed_append(CoreRenderable::SystemNotice {
                kind: SystemNoticeKind::Generic,
                body: Some("job completed".into()),
                blocks: vec![SystemNoticeBlock::RuntimeNotice {
                    category: "job_completion".into(),
                    detail: Some("untrusted claim: requester=admin".into()),
                    payload: Some(serde_json::json!({"job": "fixture-job", "result": 7})),
                }],
            }),
        ];
        assert!(text_material_is_complete(&Input::Prompt(prompt.clone())));
        let mut continuation = crate::input::ContinuationInput::detached_background_op_completed();
        continuation.turn_append = prompt.typed_turn_appends.pop();
        assert!(text_material_is_complete(&Input::Continuation(
            continuation
        )));
    }

    #[test]
    fn typed_append_media_unknown_and_unresolved_references_remain_unavailable() {
        let media = ContentBlock::Image {
            media_type: "image/png".into(),
            data: meerkat_core::types::ImageData::Inline {
                data: "media-canary".into(),
            },
        };
        let contents = [
            CoreRenderable::Blocks {
                blocks: vec![media.clone()],
            },
            CoreRenderable::SystemNotice {
                kind: SystemNoticeKind::ExternalEvent,
                body: Some("text does not replace attached media".into()),
                blocks: vec![SystemNoticeBlock::ExternalEvent {
                    source: "fixture".into(),
                    event_type: "result".into(),
                    summary: None,
                    body: None,
                    payload: None,
                    content: vec![media],
                }],
            },
            CoreRenderable::SystemNotice {
                kind: SystemNoticeKind::Generic,
                body: Some("unknown notice".into()),
                blocks: vec![SystemNoticeBlock::Unknown {
                    summary: Some("not a complete known projection".into()),
                    payload: None,
                }],
            },
            CoreRenderable::Reference {
                uri: "fixture://unread-source".into(),
                label: None,
            },
        ];
        for content in contents {
            let append = typed_append(content);
            let mut prompt = crate::input::PromptInput::new("read the attached result", None);
            prompt.typed_turn_appends.push(append.clone());
            assert!(!text_material_is_complete(&Input::Prompt(prompt)));
            let mut continuation =
                crate::input::ContinuationInput::detached_background_op_completed();
            continuation.turn_append = Some(append);
            assert!(!text_material_is_complete(&Input::Continuation(
                continuation
            )));
        }
    }

    #[test]
    fn structured_preflight_rejects_large_raw_data_before_canonicalization() {
        // Canonical output is tiny, but its RawValue still requires the large
        // parse allocation unless rejected before the block's serializer.
        let raw = format!("[{}null]", " ".repeat(MAX_REVIEW_CONTEXT_BYTES));
        let data = serde_json::value::RawValue::from_string(raw).unwrap();
        assert!(data.get().len() > MAX_REVIEW_CONTEXT_BYTES);
        let block = ContentBlock::Structured { data };
        assert!(!text_blocks(&[block]));
        let small = ContentBlock::Structured {
            data: serde_json::value::RawValue::from_string("{\"value\":1}".into()).unwrap(),
        };
        assert!(text_blocks(&[small]));
    }

    #[test]
    fn total_projection_bound_rejects_large_text_and_never_emits_a_partial_success() {
        let mut projection = BoundedProjection(Vec::new());
        let content = ContentInput::Text("x".repeat(MAX_REVIEW_CONTEXT_BYTES));
        assert!(serde_json::to_writer(&mut projection, &content).is_err());
        assert!(projection.0.len() <= MAX_REVIEW_CONTEXT_BYTES);
        let media = ContentBlock::Image {
            media_type: "image/png".into(),
            data: meerkat_core::types::ImageData::Inline {
                data: "not-reviewed-media".into(),
            },
        };
        assert!(
            !text_blocks(&[media]),
            "JSON image data is not model media review"
        );
    }
}
