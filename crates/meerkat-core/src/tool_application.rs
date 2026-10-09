//! Host UI requests bound to one committed tool invocation.
//!
//! These are runtime integration types. App authors use their protocol's
//! standard tools and resources; no application manifest is defined here.

use std::any::Any;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{OperationAuthorizationError, SessionId};

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
