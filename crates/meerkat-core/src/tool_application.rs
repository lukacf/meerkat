//! Host UI requests and observations bound to native tool invocations.
//!
//! These are runtime integration types. App authors use their protocol's
//! standard tools and resources; no application manifest is defined here.

use std::any::Any;
use std::collections::BTreeMap;
use std::sync::{
    Arc, RwLock,
    atomic::{AtomicBool, Ordering},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    Message, OperationAuthorizationError, ResolvedToolExecutionPlan, RunId, SessionId,
    ToolExecutionOwnerWitness, ToolExecutionResolutionError, ToolUnavailableReason,
};

/// Read-only host projection of one tool result accepted by the live Agent.
/// This process-only value is neither committed history nor IO authority.
#[derive(Clone, PartialEq)]
pub struct ToolApplicationObservation {
    pub tool_call_id: String,
    pub tool_name: String,
    pub host_metadata: BTreeMap<String, Value>,
}

impl ToolApplicationObservation {
    /// Project only unambiguous tool-use/result pairs from one complete
    /// canonical transcript. Duplicate ids are omitted, never last-wins.
    pub fn from_messages(messages: &[Message]) -> Vec<Self> {
        let mut calls = BTreeMap::<&str, Vec<(usize, &str)>>::new();
        let mut results = BTreeMap::<&str, Vec<(usize, &crate::ToolResult)>>::new();
        for (index, message) in messages.iter().enumerate() {
            match message {
                Message::BlockAssistant(assistant) => {
                    for call in assistant.tool_calls() {
                        calls.entry(call.id).or_default().push((index, call.name));
                    }
                }
                Message::ToolResults { results: batch, .. } => {
                    for result in batch {
                        results
                            .entry(&result.tool_use_id)
                            .or_default()
                            .push((index, result));
                    }
                }
                _ => {}
            }
        }
        let mut observations = Vec::new();
        for (index, message) in messages.iter().enumerate() {
            let Message::ToolResults { results: batch, .. } = message else {
                continue;
            };
            for result in batch {
                let id = result.tool_use_id.as_str();
                let Some([(call_index, name)]) = calls.get(id).map(Vec::as_slice) else {
                    continue;
                };
                if *call_index >= index
                    || results.get(id).is_none_or(|matches| matches.len() != 1)
                    || result.host_metadata.is_empty()
                {
                    continue;
                }
                observations.push(Self {
                    tool_call_id: id.to_string(),
                    tool_name: (*name).to_string(),
                    host_metadata: result.host_metadata.clone(),
                });
            }
        }
        observations
    }
}

impl std::fmt::Debug for ToolApplicationObservation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolApplicationObservation")
            .field("tool_call_id", &self.tool_call_id)
            .field("tool_name", &self.tool_name)
            .finish_non_exhaustive()
    }
}

/// An Agent-owned read handle. Callers cannot publish or authorize actions
/// through this handle, and must bind it to the exact live actor and run.
#[derive(Clone, Default)]
pub struct ToolApplicationObservationReader {
    state: Arc<RwLock<Option<ToolApplicationObservationState>>>,
}

enum ToolApplicationObservationState {
    Idle(Vec<ToolApplicationObservation>),
    ActiveRun(RunId, Vec<ToolApplicationObservation>),
}

impl ToolApplicationObservationReader {
    pub fn snapshot(&self) -> Option<(RunId, Vec<ToolApplicationObservation>)> {
        let state = self.state.read().ok()?;
        match state.as_ref()? {
            ToolApplicationObservationState::ActiveRun(run_id, observations) => {
                Some((run_id.clone(), observations.clone()))
            }
            ToolApplicationObservationState::Idle(_) => None,
        }
    }

    /// A between-commands snapshot published by the native actor. An active
    /// run never falls back to an earlier idle snapshot.
    pub fn idle_snapshot(&self) -> Option<Vec<ToolApplicationObservation>> {
        let state = self.state.read().ok()?;
        match state.as_ref()? {
            ToolApplicationObservationState::Idle(observations) => Some(observations.clone()),
            ToolApplicationObservationState::ActiveRun(_, _) => None,
        }
    }

    pub(crate) fn publish_idle(&self, messages: &[Message]) {
        let mut state = self
            .state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !matches!(
            state.as_ref(),
            Some(ToolApplicationObservationState::ActiveRun(_, _))
        ) {
            *state = Some(ToolApplicationObservationState::Idle(
                ToolApplicationObservation::from_messages(messages),
            ));
        }
    }

    pub(crate) fn begin_run(
        &self,
        run_id: RunId,
        messages: &[Message],
    ) -> ToolApplicationObservationRun {
        *self
            .state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Some(ToolApplicationObservationState::ActiveRun(
                run_id.clone(),
                ToolApplicationObservation::from_messages(messages),
            ));
        ToolApplicationObservationRun {
            reader: self.clone(),
            run_id,
        }
    }

    pub(crate) fn invalidate(&self) {
        let mut state = self
            .state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match state.as_mut() {
            Some(ToolApplicationObservationState::ActiveRun(_, observations)) => {
                observations.clear()
            }
            _ => *state = None,
        }
    }

    pub(crate) fn refresh(&self, messages: &[Message]) {
        let mut state = self
            .state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(ToolApplicationObservationState::ActiveRun(_, observations)) = state.as_mut() {
            *observations = ToolApplicationObservation::from_messages(messages);
        }
    }
}

pub(crate) struct ToolApplicationObservationRun {
    reader: ToolApplicationObservationReader,
    run_id: RunId,
}

impl Drop for ToolApplicationObservationRun {
    fn drop(&mut self) {
        let mut state = self
            .reader
            .state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state
            .as_ref()
            .is_some_and(|state| matches!(state, ToolApplicationObservationState::ActiveRun(run_id, _) if run_id == &self.run_id))
        {
            *state = None;
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ToolApplicationRequest {
    pub tool_call_id: String,
    pub extension: String,
    pub operation: ToolApplicationOperation,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ToolApplicationOperation {
    Resolve,
    ReadResource { uri: String },
    CallTool { name: String, arguments: Value },
}

/// Leaf-owned binding retained between UI resolution and normal tool dispatch.
/// It is process-only context, not a request accepted from an app or a model.
#[derive(Clone, PartialEq)]
pub struct ToolApplicationBinding {
    pub extension: String,
    pub payload: Value,
    owner_witnesses: Vec<ToolExecutionOwnerWitness>,
}

impl ToolApplicationBinding {
    pub fn new(extension: impl Into<String>, payload: Value) -> Self {
        Self {
            extension: extension.into(),
            payload,
            owner_witnesses: Vec::new(),
        }
    }

    /// Retain the action target selected by a routing owner during app
    /// resolution. This evidence stays outside the leaf's protocol payload.
    /// Nested routing owners each contribute their own authority key.
    pub fn with_owner_witness(
        mut self,
        witness: ToolExecutionOwnerWitness,
    ) -> Result<Self, ToolExecutionResolutionError> {
        if let Some(existing) = self
            .owner_witnesses
            .iter()
            .find(|existing| existing.authority_key() == witness.authority_key())
        {
            return Err(ToolExecutionResolutionError::OwnerWitnessAlreadyAssigned {
                existing: Box::new(existing.clone()),
                attempted: Box::new(witness),
            });
        }
        self.owner_witnesses.push(witness);
        Ok(self)
    }

    pub fn owner_witnesses(&self) -> &[ToolExecutionOwnerWitness] {
        &self.owner_witnesses
    }

    /// A fresh execution plan must retain every owner selected before app
    /// resolution completed. A newly published tool cannot capture the action
    /// between protocol resolution and ordinary native plan resolution.
    pub fn validate_execution_plan(
        &self,
        tool_name: &str,
        plan: &ResolvedToolExecutionPlan,
    ) -> Result<(), ToolExecutionResolutionError> {
        if self
            .owner_witnesses
            .iter()
            .any(|expected| plan.owner_witness(expected.authority_key()) != Some(expected))
        {
            return Err(ToolExecutionResolutionError::Unavailable {
                tool_name: tool_name.into(),
                reason: ToolUnavailableReason::ExecutionOwnerChanged,
            });
        }
        Ok(())
    }
}

/// Resolving an app action selects a canonical tool name. Core then runs that
/// call through its ordinary scope, execution policy and reviewed entry.
pub enum ToolApplicationResolution {
    Value(Value),
    Call {
        name: String,
        binding: ToolApplicationBinding,
        project_result: fn(&crate::types::ToolResult) -> Result<Value, crate::ToolError>,
    },
}

impl ToolApplicationRequest {
    pub fn validate(&self) -> Result<(), OperationAuthorizationError> {
        if self.tool_call_id.trim().is_empty()
            || self.tool_call_id.len() > 1024
            || self.extension.trim().is_empty()
            || self.extension.len() > 256
        {
            return Err(OperationAuthorizationError::Unavailable);
        }
        match &self.operation {
            ToolApplicationOperation::ReadResource { uri }
                if uri.is_empty() || uri.len() > 8192 =>
            {
                Err(OperationAuthorizationError::Unavailable)
            }
            ToolApplicationOperation::CallTool { name, arguments }
                if name.trim().is_empty() || name.len() > 1024 || !arguments.is_object() =>
            {
                Err(OperationAuthorizationError::Unavailable)
            }
            _ => Ok(()),
        }
    }
}

/// Trusted native ingress proves current viewer and exact member/session access.
/// This process object is never deserialized from an app request. Governed
/// owners additionally compose their own fresh member work authorization.
pub trait ToolApplicationIngress: Any + Send + Sync {
    fn revalidate(&self) -> Result<(), OperationAuthorizationError>;
    /// Recheck native member/session custody after queueing or other waits.
    fn revalidate_async(
        &self,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<(), OperationAuthorizationError>> + Send + '_>,
    > {
        Box::pin(async move { self.revalidate() })
    }
    fn as_any(&self) -> &(dyn Any + Send + Sync);
}

/// One immutable native submission, distinct from any previous agent run.
pub struct ToolApplicationControlRequest {
    session_id: SessionId,
    request: ToolApplicationRequest,
    ingress: Arc<dyn ToolApplicationIngress>,
    claimed: AtomicBool,
    execution_claimed: AtomicBool,
}

impl std::fmt::Debug for ToolApplicationControlRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ToolApplicationControlRequest")
            .field("session_id", &self.session_id)
            .field("extension", &self.request.extension)
            .finish_non_exhaustive()
    }
}

impl ToolApplicationControlRequest {
    pub fn from_trusted_ingress(
        session_id: SessionId,
        request: ToolApplicationRequest,
        ingress: Arc<dyn ToolApplicationIngress>,
    ) -> Result<Arc<Self>, OperationAuthorizationError> {
        request.validate()?;
        ingress.revalidate()?;
        Ok(Arc::new(Self {
            session_id,
            request,
            ingress,
            claimed: AtomicBool::new(false),
            execution_claimed: AtomicBool::new(false),
        }))
    }

    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }
    pub fn request(&self) -> &ToolApplicationRequest {
        &self.request
    }
    pub fn ingress(&self) -> &dyn ToolApplicationIngress {
        self.ingress.as_ref()
    }

    pub fn revalidate(&self) -> Result<(), OperationAuthorizationError> {
        self.ingress.revalidate()
    }

    pub async fn revalidate_async(&self) -> Result<(), OperationAuthorizationError> {
        self.ingress.revalidate_async().await
    }

    /// The Agent also owns one execution attempt, including direct embedded
    /// hosts which do not use a SessionService admission queue.
    pub(crate) fn claim_execution(&self) -> Result<(), OperationAuthorizationError> {
        self.revalidate()?;
        self.execution_claimed
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| ())
            .map_err(|_| OperationAuthorizationError::Unavailable)
    }

    /// Every submission has one attempt. A dropped HTTP response cannot turn
    /// the same receipt into another tool action.
    pub fn claim(&self) -> Result<(), OperationAuthorizationError> {
        self.revalidate()?;
        self.claimed
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| ())
            .map_err(|_| OperationAuthorizationError::Unavailable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observed_messages() -> Result<Vec<Message>, serde_json::Error> {
        let call = Message::BlockAssistant(crate::types::BlockAssistantMessage::snapshot(vec![
            crate::types::AssistantBlock::ToolUse {
                id: "result-id".into(),
                name: "canonical-tool".into(),
                args: serde_json::value::to_raw_value(&serde_json::json!({}))?,
                meta: None,
            },
        ]));
        let mut result = crate::ToolResult::new("result-id".into(), "text".into(), false);
        result
            .host_metadata
            .insert("test".into(), serde_json::json!({"private":"private-ui"}));
        Ok(vec![call, Message::tool_results(vec![result])])
    }

    #[test]
    fn application_observation_requires_unique_ordered_canonical_pair()
    -> Result<(), serde_json::Error> {
        let messages = observed_messages()?;
        let observations = ToolApplicationObservation::from_messages(&messages);
        assert_eq!(observations.len(), 1);
        assert_eq!(observations[0].tool_name, "canonical-tool");
        assert!(!format!("{:?}", observations[0]).contains("private-ui"));
        assert!(
            ToolApplicationObservation::from_messages(&[messages[1].clone(), messages[0].clone()])
                .is_empty()
        );
        assert!(ToolApplicationObservation::from_messages(&messages[1..]).is_empty());
        for duplicate in &messages {
            let mut ambiguous = messages.clone();
            ambiguous.push(duplicate.clone());
            assert!(ToolApplicationObservation::from_messages(&ambiguous).is_empty());
        }
        Ok(())
    }

    #[test]
    fn application_observation_reader_rebuilds_and_clears_at_run_lifecycle()
    -> Result<(), serde_json::Error> {
        let reader = ToolApplicationObservationReader::default();
        let messages = observed_messages()?;
        reader.publish_idle(&messages);
        assert_eq!(reader.idle_snapshot().map(|rows| rows.len()), Some(1));
        assert!(reader.snapshot().is_none());
        let first_run = RunId::new();
        let first = reader.begin_run(first_run.clone(), &messages);
        assert!(reader.idle_snapshot().is_none());
        reader.publish_idle(&messages);
        assert!(
            reader.idle_snapshot().is_none(),
            "idle publication cannot replace an active run"
        );
        assert_eq!(
            reader.snapshot().map(|(run, rows)| (run, rows.len())),
            Some((first_run.clone(), 1))
        );
        reader.invalidate();
        assert_eq!(reader.snapshot(), Some((first_run, Vec::new())));
        reader.refresh(&messages);
        assert_eq!(reader.snapshot().map(|(_, rows)| rows.len()), Some(1));
        reader.refresh(&[]);
        assert_eq!(reader.snapshot().map(|(_, rows)| rows.len()), Some(0));
        let second_run = RunId::new();
        let second = reader.begin_run(second_run.clone(), &[]);
        drop(first);
        assert_eq!(reader.snapshot(), Some((second_run, Vec::new())));
        drop(second);
        assert!(reader.snapshot().is_none());
        assert!(reader.idle_snapshot().is_none());
        reader.publish_idle(&messages);
        assert_eq!(reader.idle_snapshot().map(|rows| rows.len()), Some(1));
        reader.invalidate();
        assert!(reader.idle_snapshot().is_none());
        Ok(())
    }

    struct RevocableIngress(AtomicBool);
    impl ToolApplicationIngress for RevocableIngress {
        fn revalidate(&self) -> Result<(), OperationAuthorizationError> {
            self.0
                .load(Ordering::Acquire)
                .then_some(())
                .ok_or(OperationAuthorizationError::Unavailable)
        }
        fn as_any(&self) -> &(dyn Any + Send + Sync) {
            self
        }
    }

    fn request() -> ToolApplicationRequest {
        ToolApplicationRequest {
            tool_call_id: "call-1".into(),
            extension: "test".into(),
            operation: ToolApplicationOperation::CallTool {
                name: "action".into(),
                arguments: serde_json::json!({}),
            },
        }
    }

    #[test]
    fn receipt_is_one_attempt_and_rechecks_ingress_after_creation()
    -> Result<(), OperationAuthorizationError> {
        let ingress = Arc::new(RevocableIngress(AtomicBool::new(true)));
        let control = ToolApplicationControlRequest::from_trusted_ingress(
            SessionId::new(),
            request(),
            ingress.clone(),
        )?;
        assert!(control.claim().is_ok());
        assert!(control.claim().is_err());
        ingress.0.store(false, Ordering::Release);
        assert!(control.revalidate().is_err());
        assert!(
            ToolApplicationControlRequest::from_trusted_ingress(
                SessionId::new(),
                request(),
                ingress
            )
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn serialized_request_cannot_supply_ingress_or_a_previous_work_context()
    -> Result<(), serde_json::Error> {
        let mut value = serde_json::to_value(request())?;
        value["work_authorization"] = serde_json::json!({ "trusted": true });
        assert!(serde_json::from_value::<ToolApplicationRequest>(value).is_err());
        let mut invalid = request();
        invalid.operation = ToolApplicationOperation::CallTool {
            name: "action".into(),
            arguments: serde_json::json!([]),
        };
        assert!(invalid.validate().is_err());
        Ok(())
    }
}
