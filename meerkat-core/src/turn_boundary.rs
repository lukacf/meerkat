//! Turn-boundary hooks for composing prompt input.

use std::borrow::Cow;

/// A message that can be rendered into a user-prompt fragment at a turn boundary.
pub trait TurnBoundaryMessage {
    fn render_for_prompt(&self) -> Cow<'_, str>;
}

/// Shared turn-boundary composition logic.
#[derive(Debug, Default, Clone, Copy)]
pub struct TurnBoundaryHook;

impl TurnBoundaryHook {
    pub fn format_messages<M: TurnBoundaryMessage>(messages: &[M]) -> String {
        let mut out = String::new();
        let mut first = true;

        for msg in messages {
            if !first {
                out.push_str("\n\n");
            }
            first = false;
            out.push_str(msg.render_for_prompt().as_ref());
        }

        out
    }

    pub fn merge_with_user_input(injected: &str, user_input: &str) -> String {
        if injected.is_empty() {
            return user_input.to_string();
        }
        if user_input.is_empty() {
            return injected.to_string();
        }

        format!("{injected}\n\n---\n\n{user_input}")
    }

    pub fn merge_with_user_input_owned(injected: String, user_input: String) -> String {
        if injected.is_empty() {
            return user_input;
        }
        if user_input.is_empty() {
            return injected;
        }

        format!("{injected}\n\n---\n\n{user_input}")
    }
}

impl TurnBoundaryMessage for crate::comms_runtime::CommsMessage {
    fn render_for_prompt(&self) -> Cow<'_, str> {
        Cow::Owned(self.to_user_message_text())
    }
}
