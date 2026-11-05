//! Human escalation broker.
//!
//! The admission gate decides parallel tool calls concurrently, but the human can
//! only answer one dialog at a time, so the broker serializes requests with an
//! async mutex held across the whole wait. Any failure to get an answer (UI
//! gone, responder dropped) resolves as [`HumanDecision::Deny`]: fail closed.

use super::rules::Rule;
use meerkat_core::SessionId;
use tokio::sync::{Mutex, MutexGuard, mpsc, oneshot};

/// The human's answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HumanDecision {
    /// Allow this one call.
    AllowOnce,
    /// Allow and add the given (narrow) allow rule.
    AllowAndAddRule(Box<Rule>),
    /// Deny this call.
    Deny,
    /// Deny and stop the turn; pending escalations for the session are denied too.
    Stop,
}

/// What the UI shows in the escalation dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EscalationPrompt {
    /// Engine-assigned id; a withdrawn escalation
    /// ([`super::ApprovalEvent::EscalationWithdrawn`]) names it.
    pub id: u64,
    /// Session the call belongs to.
    pub session_id: SessionId,
    /// Tool name.
    pub tool: String,
    /// Short rendering of the arguments.
    pub args_preview: String,
    /// Canonical rule subjects of the action (see
    /// [`super::policy::Action::subjects`]); "allow + add rule" mints a literal
    /// rule from the first one.
    pub subjects: Vec<String>,
    /// The classifier's opinion, if it produced one.
    pub classifier_hint: Option<String>,
}

/// An escalation delivered to the UI, carrying the responder.
#[derive(Debug)]
pub struct EscalationRequest {
    /// Engine-assigned id (see [`EscalationPrompt::id`]).
    pub id: u64,
    /// Session the call belongs to.
    pub session_id: SessionId,
    /// Tool name.
    pub tool: String,
    /// Short rendering of the arguments.
    pub args_preview: String,
    /// Canonical rule subjects of the action.
    pub subjects: Vec<String>,
    /// The classifier's opinion, if it produced one.
    pub classifier_hint: Option<String>,
    /// Where to send the answer.
    pub respond: oneshot::Sender<HumanDecision>,
}

/// Serializes human escalations and forwards them to the UI.
#[derive(Debug)]
pub struct EscalationBroker {
    serial: Mutex<()>,
    tx: mpsc::Sender<EscalationRequest>,
}

impl EscalationBroker {
    /// Create a broker and the receiver the UI reads requests from.
    pub fn new(capacity: usize) -> (Self, mpsc::Receiver<EscalationRequest>) {
        let (tx, rx) = mpsc::channel(capacity.max(1));
        (
            Self {
                serial: Mutex::new(()),
                tx,
            },
            rx,
        )
    }

    /// Acquire the serial lock. Callers re-check their state after this
    /// returns and then call [`Self::ask_locked`].
    pub async fn serial(&self) -> MutexGuard<'_, ()> {
        self.serial.lock().await
    }

    /// Ask the human while holding the serial lock.
    pub async fn ask_locked(
        &self,
        _guard: &MutexGuard<'_, ()>,
        prompt: EscalationPrompt,
    ) -> HumanDecision {
        let (respond, answer) = oneshot::channel();
        let request = EscalationRequest {
            id: prompt.id,
            session_id: prompt.session_id,
            tool: prompt.tool,
            args_preview: prompt.args_preview,
            subjects: prompt.subjects,
            classifier_hint: prompt.classifier_hint,
            respond,
        };
        if self.tx.send(request).await.is_err() {
            return HumanDecision::Deny;
        }
        answer.await.unwrap_or(HumanDecision::Deny)
    }

    /// Acquire the serial lock and ask the human.
    pub async fn ask(&self, prompt: EscalationPrompt) -> HumanDecision {
        let guard = self.serial().await;
        self.ask_locked(&guard, prompt).await
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Duration;

    fn prompt(tool: &str) -> EscalationPrompt {
        EscalationPrompt {
            id: 0,
            session_id: SessionId::new(),
            tool: tool.into(),
            args_preview: "{}".into(),
            subjects: vec!["{}".into()],
            classifier_hint: None,
        }
    }

    #[tokio::test]
    async fn concurrent_asks_are_served_one_at_a_time() {
        let (broker, mut rx) = EscalationBroker::new(8);
        let broker = Arc::new(broker);
        let a = tokio::spawn({
            let b = Arc::clone(&broker);
            async move { b.ask(prompt("a")).await }
        });
        let b = tokio::spawn({
            let b = Arc::clone(&broker);
            async move { b.ask(prompt("b")).await }
        });

        let first = rx.recv().await.unwrap();
        // While the first request is unanswered, no second request may arrive.
        assert!(
            tokio::time::timeout(Duration::from_millis(100), rx.recv())
                .await
                .is_err(),
            "second escalation delivered before the first was answered"
        );
        let first_tool = first.tool.clone();
        first.respond.send(HumanDecision::AllowOnce).unwrap();
        let second = rx.recv().await.unwrap();
        assert_ne!(second.tool, first_tool);
        second.respond.send(HumanDecision::Deny).unwrap();

        let (ra, rb) = (a.await.unwrap(), b.await.unwrap());
        let mut answers = [ra, rb];
        answers.sort_by_key(|d| matches!(d, HumanDecision::Deny));
        assert_eq!(answers, [HumanDecision::AllowOnce, HumanDecision::Deny]);
    }

    #[tokio::test]
    async fn dropped_responder_denies() {
        let (broker, mut rx) = EscalationBroker::new(1);
        let task = tokio::spawn(async move { broker.ask(prompt("x")).await });
        let req = rx.recv().await.unwrap();
        drop(req);
        assert_eq!(task.await.unwrap(), HumanDecision::Deny);
    }

    #[tokio::test]
    async fn dropped_receiver_denies() {
        let (broker, rx) = EscalationBroker::new(1);
        drop(rx);
        assert_eq!(broker.ask(prompt("x")).await, HumanDecision::Deny);
    }
}
