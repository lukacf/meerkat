//! The thread a console send records, and replies delivered into it.
//!
//! A console send keeps its thread on its send record (the `user_input`
//! frame of the send): the member it went to (the frame's identity), the
//! send's interaction id (the frame's), who may read the replies to it (the
//! audience) and the person they are for (the addressee). Everything is set
//! by the console from the caller it resolved and the route the send took;
//! no request field names any of it.

use serde::{Deserialize, Serialize};

use super::{ConsoleLogResult, MetaConsoleAggregator};

/// Who may read the replies to a console request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ConsoleThreadAudience {
    /// Everyone who may view the member the request was sent to.
    Shared,
    /// The one person the member is private to, who made the request (and
    /// the access admins, who see every private member).
    Person,
}

/// The thread a console send records in its `user_input` payload, under
/// [`ConsoleSendThread::PAYLOAD_KEY`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ConsoleSendThread {
    pub(crate) audience: ConsoleThreadAudience,
    /// The private-owner key ([`crate::access::PRIVATE_TO_LABEL`]) of the
    /// person the replies to this send are for: the sender, when the send
    /// names a person (a verified person on a console that requires
    /// sign-in). Absent for every other send, whose replies have no one to
    /// go to privately.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) addressee: Option<String>,
}

impl ConsoleSendThread {
    /// The key of the thread in a `user_input` payload.
    pub(crate) const PAYLOAD_KEY: &'static str = "thread";
}

/// Where a console request came from, read back from its send record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConsoleThreadReference {
    /// The member the request was sent to: an agent, or a side chat.
    pub(crate) identity: String,
    /// The send's interaction id.
    pub(crate) interaction_id: String,
    /// The send record's frame id: the request a reply links to.
    pub(crate) input_frame_id: String,
    pub(crate) audience: ConsoleThreadAudience,
    /// The private-owner key of the person the replies are for, when the
    /// send names one.
    pub(crate) addressee: Option<String>,
    /// The sender's console principal (the access subject: the verified
    /// email, else the `sub`), when the send names a person: what an
    /// approver's `people` entry is matched against.
    pub(crate) access_principal: Option<String>,
}

impl MetaConsoleAggregator {
    /// The thread the console send whose interaction id is `interaction_id`
    /// recorded; `None` when no send record with that interaction id holds a
    /// thread (an unknown interaction, a send made before threads were
    /// recorded, or a send of the public send API, which has no console
    /// caller).
    pub(crate) async fn thread_reference(
        &self,
        _interaction_id: &str,
    ) -> ConsoleLogResult<Option<ConsoleThreadReference>> {
        Ok(None)
    }
}
