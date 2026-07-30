//! Provider wire capability for ordered `System` transcript messages.
//!
//! Canonical session semantics are always the same: `System` is an ordinary,
//! repeatable message role that may occur anywhere in transcript order. This
//! type describes only whether a concrete provider wire can faithfully encode
//! that canonical shape.

use crate::types::Message;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Fidelity of a concrete provider wire for canonical `System` messages.
///
/// The default is deliberately the restrictive capability. Custom clients
/// must opt into [`Self::Interleaved`] only when their concrete request
/// lowering preserves every `System` message at its exact transcript position.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SystemMessageWireCapability {
    /// Every `System` message can be represented at its exact ordered position.
    Interleaved,
    /// Only one contiguous `System` prefix before all non-System messages can
    /// be represented. A later `System` message is incompatible.
    #[default]
    LeadingPrefixOnly,
}

impl SystemMessageWireCapability {
    /// Return the first canonical message index this wire cannot represent.
    #[must_use]
    pub fn first_incompatible_index(self, messages: &[Message]) -> Option<usize> {
        match self {
            Self::Interleaved => None,
            Self::LeadingPrefixOnly => {
                let mut prefix_open = true;
                for (index, message) in messages.iter().enumerate() {
                    if matches!(message, Message::System(_)) {
                        if !prefix_open {
                            return Some(index);
                        }
                    } else {
                        prefix_open = false;
                    }
                }
                None
            }
        }
    }

    /// Return the first incompatible index after appending `count` ordinary
    /// `System` messages at the canonical transcript tail.
    ///
    /// No messages are allocated or appended while making this decision.
    #[must_use]
    pub fn first_incompatible_index_after_system_append(
        self,
        messages: &[Message],
        count: usize,
    ) -> Option<usize> {
        self.first_incompatible_index(messages).or_else(|| {
            (count > 0
                && matches!(self, Self::LeadingPrefixOnly)
                && messages
                    .iter()
                    .any(|message| !matches!(message, Message::System(_))))
            .then_some(messages.len())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::SystemMessageWireCapability;
    use crate::types::{Message, SystemMessage, UserMessage};

    #[test]
    fn restrictive_default_accepts_only_a_contiguous_leading_prefix() {
        assert_eq!(
            SystemMessageWireCapability::default(),
            SystemMessageWireCapability::LeadingPrefixOnly
        );
        let leading = vec![
            Message::System(SystemMessage::new("first")),
            Message::System(SystemMessage::new("second")),
            Message::User(UserMessage::text("hello")),
        ];
        assert_eq!(
            SystemMessageWireCapability::LeadingPrefixOnly.first_incompatible_index(&leading),
            None
        );

        let interleaved = vec![
            Message::User(UserMessage::text("hello")),
            Message::System(SystemMessage::new("later")),
        ];
        assert_eq!(
            SystemMessageWireCapability::LeadingPrefixOnly.first_incompatible_index(&interleaved),
            Some(1)
        );
        assert_eq!(
            SystemMessageWireCapability::Interleaved.first_incompatible_index(&interleaved),
            None
        );
    }

    #[test]
    fn proposed_tail_append_reports_the_exact_future_index_without_mutation() {
        let messages = vec![Message::User(UserMessage::text("hello"))];
        assert_eq!(
            SystemMessageWireCapability::LeadingPrefixOnly
                .first_incompatible_index_after_system_append(&messages, 2),
            Some(1)
        );
        assert_eq!(
            SystemMessageWireCapability::LeadingPrefixOnly
                .first_incompatible_index_after_system_append(&messages, 0),
            None
        );
    }
}
