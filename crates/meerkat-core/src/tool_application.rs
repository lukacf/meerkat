//! Host UI requests and observations bound to native tool invocations.
//!
//! These are runtime integration types. App authors use their protocol's
//! standard tools and resources; no application manifest is defined here.

use std::any::Any;
use std::collections::BTreeMap;
use std::sync::{
    Arc, OnceLock, RwLock,
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
                observations.clear();
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

/// Canonical invocation ownership lives with the Agent, independently of the
/// public display reader. Only Agent transcript publication can mint records.
#[derive(Default)]
pub(crate) struct ToolApplicationInvocationOwner {
    observations: ToolApplicationObservationReader,
    state: Arc<RwLock<ToolApplicationInvocationState>>,
}

struct ToolApplicationInvocationState {
    open: bool,
    accepted: BTreeMap<String, Arc<ToolApplicationObservation>>,
}

impl Default for ToolApplicationInvocationState {
    fn default() -> Self {
        Self {
            open: true,
            accepted: BTreeMap::new(),
        }
    }
}

impl ToolApplicationInvocationOwner {
    pub(crate) fn reader(&self) -> ToolApplicationObservationReader {
        self.observations.clone()
    }

    pub(crate) fn execution_owner(&self) -> ToolApplicationExecutionOwner {
        ToolApplicationExecutionOwner {
            state: self.state.clone(),
        }
    }

    fn publish_accepted(&self, messages: &[Message]) {
        let mut state = self
            .state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut previous = std::mem::take(&mut state.accepted);
        state.accepted = ToolApplicationObservation::from_messages(messages)
            .into_iter()
            .map(|observation| {
                let id = observation.tool_call_id.clone();
                let accepted = previous
                    .remove(&id)
                    .filter(|old| old.as_ref() == &observation)
                    .unwrap_or_else(|| Arc::new(observation));
                (id, accepted)
            })
            .collect();
    }

    pub(crate) fn publish_idle(&self, messages: &[Message]) {
        self.publish_accepted(messages);
        self.observations.publish_idle(messages);
    }

    pub(crate) fn begin_run(
        &self,
        run_id: RunId,
        messages: &[Message],
    ) -> ToolApplicationObservationRun {
        self.publish_accepted(messages);
        self.observations.begin_run(run_id, messages)
    }

    pub(crate) fn refresh(&self, messages: &[Message]) {
        self.publish_accepted(messages);
        self.observations.refresh(messages);
    }

    pub(crate) fn invalidate(&self) {
        self.state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .accepted
            .clear();
        self.observations.invalidate();
    }
}

impl Drop for ToolApplicationInvocationOwner {
    fn drop(&mut self) {
        self.execution_owner().close_admission();
    }
}

/// Process-only identity of one exact Agent, never derived from a session id
/// or reconstructed from a display projection.
#[derive(Clone)]
pub(crate) struct ToolApplicationExecutionOwner {
    state: Arc<RwLock<ToolApplicationInvocationState>>,
}

impl ToolApplicationExecutionOwner {
    pub(crate) fn same_owner(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.state, &other.state)
    }

    pub(crate) fn close_admission(&self) {
        self.state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .open = false;
    }

    pub(crate) fn accept(
        &self,
        request: &ToolApplicationRequest,
        scope: crate::tool_scope::ToolScope,
    ) -> Result<Arc<ToolApplicationInvocationGuard>, OperationAuthorizationError> {
        let state = self
            .state
            .read()
            .map_err(|_| OperationAuthorizationError::Unavailable)?;
        if !state.open {
            return Err(OperationAuthorizationError::Unavailable);
        }
        let accepted = state
            .accepted
            .get(&request.tool_call_id)
            .filter(|record| record.host_metadata.contains_key(&request.extension))
            .cloned()
            .ok_or(OperationAuthorizationError::Unavailable)?;
        Ok(Arc::new(ToolApplicationInvocationGuard {
            owner: self.clone(),
            accepted,
            scope,
            target: OnceLock::new(),
        }))
    }
}

pub(crate) struct ToolApplicationInvocationGuard {
    owner: ToolApplicationExecutionOwner,
    accepted: Arc<ToolApplicationObservation>,
    scope: crate::tool_scope::ToolScope,
    target: OnceLock<String>,
}

impl ToolApplicationInvocationGuard {
    pub(crate) fn source(&self) -> &str {
        &self.accepted.tool_name
    }
    pub(crate) fn invocation(&self, extension: &str) -> &Value {
        &self.accepted.host_metadata[extension]
    }
    pub(crate) fn bind_target(&self, target: String) -> Result<(), OperationAuthorizationError> {
        self.target
            .set(target)
            .map_err(|_| OperationAuthorizationError::Unavailable)?;
        self.revalidate()
    }
}

impl ToolApplicationIngress for ToolApplicationInvocationGuard {
    fn revalidate(&self) -> Result<(), OperationAuthorizationError> {
        let state = self
            .owner
            .state
            .read()
            .map_err(|_| OperationAuthorizationError::Unavailable)?;
        if !state.open
            || !state
                .accepted
                .get(&self.accepted.tool_call_id)
                .is_some_and(|current| Arc::ptr_eq(current, &self.accepted))
        {
            return Err(OperationAuthorizationError::Unavailable);
        }
        if !self
            .scope
            .host_visible_tool_names()
            .map_err(|_| OperationAuthorizationError::Unavailable)?
            .iter()
            .any(|name| name.as_str() == self.source())
        {
            return Err(OperationAuthorizationError::Unavailable);
        }
        if let Some(target) = self.target.get()
            && !self
                .scope
                .app_visible_tool_names()
                .map_err(|_| OperationAuthorizationError::Unavailable)?
                .iter()
                .any(|name| name.as_str() == target)
        {
            return Err(OperationAuthorizationError::Unavailable);
        }
        Ok(())
    }
    fn as_any(&self) -> &(dyn Any + Send + Sync) {
        self
    }
}

/// An Agent-minted executor. Hosts retain it only with the exact native actor
/// custody lease, and own its future through outcome settlement on cancellation.
#[derive(Clone)]
pub struct ToolApplicationExecutor {
    pub(crate) execution: Arc<dyn ToolApplicationExecution>,
    pub(crate) owner: ToolApplicationExecutionOwner,
}

#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
pub(crate) trait ToolApplicationExecution: Send + Sync {
    async fn execute(
        &self,
        request: ToolApplicationRequest,
        context: crate::ToolDispatchContext,
    ) -> ToolApplicationExecutionOutcome;
}

impl ToolApplicationExecutor {
    pub async fn execute(
        &self,
        request: ToolApplicationRequest,
        context: crate::ToolDispatchContext,
    ) -> ToolApplicationExecutionOutcome {
        self.execution.execute(request, context).await
    }

    /// Refuse new entry during native actor shutdown. Existing outcome
    /// settlement remains valid for this exact owner.
    pub fn close_admission(&self) {
        self.owner.close_admission();
    }
}

/// Result plus every pending canonical mutation, including on failed hooks or
/// failed result publication. Fields are opaque so hosts cannot forge effects.
pub struct ToolApplicationExecutionOutcome {
    pub(crate) owner: ToolApplicationExecutionOwner,
    pub(crate) result: Result<Value, crate::AgentError>,
    pub(crate) notices: Vec<Message>,
    pub(crate) events: Vec<crate::AgentEvent>,
    pub(crate) effects: Vec<crate::ops::SessionEffect>,
    pub(crate) settlement_failures: Vec<crate::ops::ToolDispatchSettlementFailure>,
}

impl ToolApplicationExecutionOutcome {
    pub fn requires_settlement(&self) -> bool {
        !self.notices.is_empty() || !self.effects.is_empty()
    }

    /// Revoke result publication after an actor wait without undoing entered
    /// work or discarding hook notices and non-publication effects. A primary
    /// execution/hook failure remains typed when publication also becomes invalid.
    pub fn withhold_result(mut self, error: crate::AgentError) -> Self {
        self.effects.retain(|effect| {
            !matches!(
                effect,
                crate::ops::SessionEffect::AppendAssistantBlocks { .. }
            )
        });
        if self.result.is_ok() {
            self.result = Err(error.with_settlement_failures(self.settlement_failures()));
        }
        self
    }

    /// Empty outcomes can be returned while the model owns its Agent borrow.
    /// A mutation-bearing outcome is returned intact for actor settlement.
    pub fn into_immediate_result(self) -> Result<Result<Value, crate::AgentError>, Box<Self>> {
        if self.requires_settlement() {
            Err(Box::new(self))
        } else {
            Ok(self.result)
        }
    }

    /// Native actor loss cannot settle this outcome elsewhere. Preserve the
    /// entered operation's diagnostics when the owning service reports refusal.
    pub fn into_refused_result(self, error: crate::AgentError) -> Result<Value, crate::AgentError> {
        Err(error.with_settlement_failures(self.settlement_failures()))
    }

    /// Retain diagnostics when native custody or persistence fails without
    /// changing that failure's primary session classification.
    pub fn into_refused_session_result(
        self,
        error: crate::SessionError,
    ) -> Result<Value, crate::SessionError> {
        Err(error.with_settlement_failures(self.settlement_failures()))
    }

    pub fn settlement_failures(&self) -> Vec<crate::ops::ToolDispatchSettlementFailure> {
        let mut failures = self.settlement_failures.clone();
        if let Err(result) = &self.result {
            for failure in result.settlement_failures() {
                if !failures.contains(failure) {
                    failures.push(failure.clone());
                }
            }
        }
        failures
    }
}

/// Actor settlement preserves dirty evidence independently of result success.
pub struct ToolApplicationSettlement {
    pub(crate) result: Result<Value, crate::AgentError>,
    pub(crate) dirty: bool,
}

impl ToolApplicationSettlement {
    pub fn into_parts(self) -> (Result<Value, crate::AgentError>, bool) {
        (self.result, self.dirty)
    }
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
    execution_guard: OnceLock<Arc<dyn ToolApplicationIngress>>,
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
            execution_guard: OnceLock::new(),
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
        self.ingress.revalidate()?;
        if let Some(guard) = self.execution_guard.get() {
            guard.revalidate()?;
        }
        Ok(())
    }

    pub async fn revalidate_async(&self) -> Result<(), OperationAuthorizationError> {
        self.ingress.revalidate_async().await?;
        if let Some(guard) = self.execution_guard.get() {
            guard.revalidate_async().await?;
        }
        Ok(())
    }

    pub(crate) fn bind_execution_guard(
        &self,
        guard: Arc<dyn ToolApplicationIngress>,
    ) -> Result<(), OperationAuthorizationError> {
        self.execution_guard
            .set(guard)
            .map_err(|_| OperationAuthorizationError::Unavailable)?;
        self.revalidate()
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
