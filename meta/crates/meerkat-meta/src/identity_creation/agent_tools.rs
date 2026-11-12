//! The agent tools of the identity family: what an agent calls to create and
//! change long-term agents, and every word a model reads about them.
//!
//! Eight tools: `identity_create`, `identity_update`, `identity_archive`,
//! `identity_unarchive`, `identity_remove`, and the reads `identity_list`,
//! `identity_get` and `identity_directory`. They are one more surface of
//! the family
//! ([`IdentityFamily`]) and own no meaning: a tool parses its arguments
//! into the family's wire types, attributes the requester from the session
//! its call ran in, names the operation and calls the family. It builds no
//! consent, applies no rule of the host and never reaches the executor
//! seam.
//!
//! - **Arguments.** The one wire spelling ([`CreateAgents`],
//!   [`UpdateAgent`], [`AgentRef`], [`ListAgents`], [`GetAgent`],
//!   [`ReadDirectory`]),
//!   without what only the host plane writes (`proposer`,
//!   `source_evidence`). Each tool's schema is closed, and the schema is
//!   what a call is checked against before anything else reads it: an
//!   argument it does not name is refused `invalid_request` with the
//!   argument's path as `field`, so who asks can never be an argument. A
//!   `null` member is a member left out.
//! - **Requester.** [`ChangeRequester::agent_session`] of the calling
//!   member, by the session the dispatch names, marked private to a person
//!   when it is.
//! - **Operation.** The caller's `request_id` through
//!   [`OperationId::for_request`], else [`OperationId::for_tool_call`] over
//!   where the call ran: its mob, session, run, and its round and call id
//!   within that run. A provider's call ids repeat from one model response
//!   to the next, so the round is part of what names a call within its run;
//!   the same call dispatched again (after a resume) names the same
//!   operation and reads the same state. A call dispatched outside a run
//!   has no such place and must give a `request_id`.
//! - **Results.** [`IdentityOutcome::to_json`] verbatim for a change; the
//!   family's answer as it serializes for a read, without each agent's
//!   provenance and open operations, which an agent is not shown; a refused
//!   read as a refused outcome. Which agents an agent is shown is the
//!   family's own rule: the tools ask for no view of their own.
//!
//! The descriptions and schemas below are product text: an agent knows of
//! long-term agents only what they say. Each is pinned by a golden
//! (`tests/support/identity_tool_goldens.rs`).

use std::sync::{Arc, LazyLock};

use meerkat_core::ToolCatalogEntry;
use meerkat_core::types::{SessionId, ToolDef, ToolProvenance, ToolSourceKind};
use meerkat_mob::MobHandle;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::digest::Sha256Digest;
use super::executor::ChangeRequester;
use super::family::{
    AgentRef, AgentSet, AgentSpec, AgentSummary, CreateAgents, GetAgent, IdentityChangeCall,
    IdentityChangeSpec, IdentityFamily, IdentityRead, IdentityReadAnswer, ListAgents,
    ReadDirectory, UpdateAgent,
};
use super::operation::{OperationId, ToolCallOrigin};
use super::outcome::IdentityOutcome;
use super::policy::CORE_BATCH_MAX;
use super::refusal::{IdentityRefusal, IdentityRefusalKind, RequestProblem};

/// Create one or more long-term agents.
pub const IDENTITY_CREATE: &str = "identity_create";
/// Change one long-term agent.
pub const IDENTITY_UPDATE: &str = "identity_update";
/// Stop one long-term agent and keep it.
pub const IDENTITY_ARCHIVE: &str = "identity_archive";
/// Bring one archived long-term agent back.
pub const IDENTITY_UNARCHIVE: &str = "identity_unarchive";
/// Remove one long-term agent for good.
pub const IDENTITY_REMOVE: &str = "identity_remove";
/// List the mob's long-term agents, of any profile.
pub const IDENTITY_LIST: &str = "identity_list";
/// Read one long-term agent's record by name.
pub const IDENTITY_GET: &str = "identity_get";
/// List what the caller may name in a create or an update.
pub const IDENTITY_DIRECTORY: &str = "identity_directory";

/// The identity tools, in the order a catalog lists them.
pub const IDENTITY_TOOLS: [&str; 8] = [
    IDENTITY_CREATE,
    IDENTITY_UPDATE,
    IDENTITY_ARCHIVE,
    IDENTITY_UNARCHIVE,
    IDENTITY_REMOVE,
    IDENTITY_LIST,
    IDENTITY_GET,
    IDENTITY_DIRECTORY,
];

/// The source every identity tool names as where it comes from.
const TOOL_SOURCE: &str = "identity";

/// Whether `name` is one of the identity tools.
pub fn is_identity_tool(name: &str) -> bool {
    IDENTITY_TOOLS.contains(&name)
}

/// Where the identity tools come from: what a deferred catalog lists them
/// under.
pub fn identity_tool_provenance() -> ToolProvenance {
    ToolProvenance {
        kind: ToolSourceKind::Mob,
        source_id: TOOL_SOURCE.into(),
        artifact: None,
    }
}

static TOOL_DEFS: LazyLock<Arc<[Arc<ToolDef>]>> = LazyLock::new(|| {
    IDENTITY_TOOLS
        .iter()
        .map(|name| {
            let mut tool = ToolDef::new(*name, description(name), input_schema(name));
            tool.provenance = Some(identity_tool_provenance());
            Arc::new(tool)
        })
        .collect::<Vec<_>>()
        .into()
});

/// The definitions of the identity tools: name, description and argument
/// schema of each, as a model reads them.
pub fn identity_tool_defs() -> Arc<[Arc<ToolDef>]> {
    Arc::clone(&TOOL_DEFS)
}

/// The identity tools as a catalog lists them: session tools that may wait
/// behind the deferred catalog, so a session with many tools carries their
/// definitions only once it asked for them.
pub fn identity_tool_catalog() -> Vec<ToolCatalogEntry> {
    identity_tool_defs()
        .iter()
        .map(|tool| {
            ToolCatalogEntry::session_deferred(Arc::clone(tool), true, identity_tool_provenance())
        })
        .collect()
}

const WHAT_A_LONG_TERM_AGENT_IS: &str = "A long-term agent is a durable, named member of the \
    mob: it keeps its own conversation and memory across restarts, is addressed by its name, \
    and is the same kind of agent as one the host configured. You are recorded as who asked \
    for it. You do not own it, it does not report to you, and it is not limited to your \
    tools. For a helper that does one task and is discarded, use the worker tools (delegate, \
    fork_off, mob_spawn_member) instead.";

const INSTRUCTIONS_REACH: &str = "It reaches the agent at its next run; a run that starts \
    within moments of the change may still use the earlier instructions; earlier \
    instructions are never rewritten.";

const FIRST_MESSAGE_ANSWER: &str = "A create with a first message answers once the new agent's \
    session exists; the first run continues after the answer.";

const GOVERNED_FIRST_MESSAGE: &str = "In a governed realm an agent is created without a first \
    message: a create that gives one is refused, and the agent's first input comes through \
    the host's own ingress.";

const AS_FOR_CREATE: &str = "and `request_id` works, as for identity_create.";

const UNKNOWN_NAME: &str = "A `name` that no agent holds, and an agent you are not shown (one \
    private to a person), both answer `identity_unknown`.";

const CONFIGURED_AGENT: &str = "An agent that the host's configuration declares is changed \
    there, not here (`owner_read_only`).";

const READ_BACK_ONLY: &str = "`addressability` and `runtime_mode_override` are read back only: \
    the host sets them, and identity_create and identity_update do not take them.";

/// The description of the identity tool `name`, as a model reads it.
fn description(name: &str) -> String {
    match name {
        IDENTITY_CREATE => format!(
            "Create one or more long-term agents in your mob.\n\n\
             {WHAT_A_LONG_TERM_AGENT_IS}\n\n\
             Each agent takes a unique `name` and a `profile` of the mob, which gives its \
             model, tools and skills. Call identity_directory first: it lists the profiles \
             you may use and the agents you may name as peers. Optional: `display_name`; \
             `instructions`, its standing brief; `context`, descriptive data about it; \
             `labels`; `peers`, the agents it can message; `first_message`.\n\n\
             `peers`: nothing is wired to you unless you list your own name. The agents of \
             one call may name each other.\n\n\
             `first_message` runs once, as the new agent's first turn: a kickoff with no \
             requester. It is not a message from you: the agent has nothing to reply to, \
             and reaches you only if you are one of its peers. That turn has the same tools \
             as any other turn of the agent. Write it as the agent's opening task. \
             {FIRST_MESSAGE_ANSWER} Without \
             it the agent is created dormant: nothing runs until something is delivered to \
             it. {GOVERNED_FIRST_MESSAGE}\n\n\
             All agents of one call (1 to {CORE_BATCH_MAX}) are created together or not at \
             all. The host's rules decide what happens, and the result's `outcome` says \
             which:\n\
             - `done`: created. `agents` lists each agent's `name`, `spec_digest` and \
             `spec_revision`. `seat: \"owed\"` on an agent means its first message has not \
             started: the agent exists, and it is seated and its first message runs at its \
             next delivery.\n\
             - `pending_decision`: a person must decide (`decision_id`, `deadline_ms`). \
             Nothing is created yet. Do not repeat the call with new arguments: that is a \
             new request. If you are a long-term agent yourself, a notice reaches you when \
             it is decided; in every case the identical call with the same `request_id` \
             returns the current state.\n\
             - `pending_preparation`: created, and at least one agent (its `state` is \
             `pending_preparation`) waits for the host to prepare it. Until the host settles \
             that, the agent has no member, takes no message and runs nothing; its first \
             message runs once when it is settled. No tool settles it, and nothing you send \
             speeds it up: identity_list and identity_get read it as `readiness: \
             \"preparation_pending\"`, and the identical call with the same `request_id` \
             returns where it stands.\n\
             - `refused`: nothing was created. `kind` names why, `field` the argument it \
             concerns, `detail` what to send instead. Repeat the same call later only when \
             `retryable` is true.\n\n\
             Pass `request_id` when you may need to ask again: the same id with the same \
             arguments returns the same state and never creates twice, and the same id with \
             other arguments is refused `operation_reused`. Without it, only a retry of this \
             same tool call is recognized as the same request.\n\n\
             A name that was removed is never available again (`name_taken`)."
        ),
        IDENTITY_UPDATE => format!(
            "Change a long-term agent of your mob: its display name, instructions, context, \
             labels, profile or peers.\n\n\
             Send `expected_spec_digest`: the agent's `spec_digest` from identity_list, or \
             from the last result that named it. If the agent changed meanwhile you get \
             `stale_spec` with `current_digest`: read the agent again, then decide whether \
             your change still applies. {UNKNOWN_NAME}\n\n\
             `set` names what changes; a member you leave out stays as it is. \
             `instructions` and `context` are replaced whole. `labels` sets each key you \
             give and keeps the others; `labels_remove` removes keys. `profile` moves the \
             agent to that profile's model, tools and skills on the same conversation. A \
             move into a profile whose agents the host prepares is refused `unsupported` at \
             `set.profile`: create the agent in that profile instead. \
             `peers_add` and `peers_remove` change who it can message.\n\n\
             An instructions change is in the agent's record at once. {INSTRUCTIONS_REACH} \
             The result says what became of it in `agents[0].instructions.kind`: \
             `applies_at_next_run`; `applies_after_compaction` (the agent's model takes new \
             instructions only at the head of what it reads, so the change reaches it after \
             its next compaction); `applies_at_next_session` with a `reason` (the change \
             does not reach the agent as it runs now, and applies when it next starts a new \
             conversation); `unchanged`.\n\n\
             You cannot change yourself (`self_change_refused`). {CONFIGURED_AGENT}\n\n\
             The result's `outcome` is `done`, `pending_decision` or `refused`, \
             {AS_FOR_CREATE} An update of an agent that waits for its host's preparation \
             changes its record only: the agent still waits."
        ),
        IDENTITY_ARCHIVE => format!(
            "Stop a long-term agent and keep it. Its name, record, conversation and memory \
             stay; it has no running member, messages to it are refused, and \
             identity_unarchive brings it back on the same conversation. Use this when the \
             agent may be needed later; identity_remove is for good.\n\n\
             Send the agent's `name` and `expected_spec_digest` (its `spec_digest` from \
             identity_list; a changed agent answers `stale_spec` with `current_digest`). \
             {UNKNOWN_NAME} You cannot archive yourself (`self_change_refused`). \
             {CONFIGURED_AGENT}\n\n\
             The result's `outcome` is `done` (the agent's `state` is `archiving` while its \
             member stops, then `archived`), `pending_decision` or `refused`, \
             {AS_FOR_CREATE}"
        ),
        IDENTITY_UNARCHIVE => format!(
            "Bring an archived long-term agent back. It returns on the conversation it had, \
             with its name, memory and current instructions, and takes messages again.\n\n\
             Send the agent's `name` and `expected_spec_digest` (its `spec_digest` from \
             identity_list with `include_archived: true`). An agent that is not archived \
             answers `not_archived`. {UNKNOWN_NAME} You cannot unarchive yourself \
             (`self_change_refused`). {CONFIGURED_AGENT}\n\n\
             The result's `outcome` is `done` (the agent's `state` is `restored`), \
             `pending_decision` or `refused`, {AS_FOR_CREATE}"
        ),
        IDENTITY_REMOVE => format!(
            "Remove a long-term agent for good. Its member stops, its peer links end, and \
             its name can never be used again, by anyone. Its conversation and memory are \
             kept, and nothing can reach them. This cannot be undone: use identity_archive \
             when the agent may be needed later.\n\n\
             Send the agent's `name` and `expected_spec_digest` (its `spec_digest` from \
             identity_list; a changed agent answers `stale_spec` with `current_digest`). \
             {UNKNOWN_NAME} You cannot remove yourself (`self_change_refused`). \
             {CONFIGURED_AGENT}\n\n\
             The result's `outcome` is `done` (the agent's `state` is `retiring` while its \
             member stops, then `removed`), `pending_decision` or `refused`, \
             {AS_FOR_CREATE}"
        ),
        IDENTITY_LIST => format!(
            "List the long-term agents of your mob: every agent that is not \
             private to a person, of any profile, whoever asked for it and however it came \
             to exist. An agent of a profile you may not create agents of is listed too: \
             `identity_directory` says what you may create and name. Read only: it never \
             wakes an agent and loads nothing.\n\n\
             Each entry of `agents` has `name`, `display_name`, `profile`, `spec_digest` \
             (what a change sends as `expected_spec_digest`), `spec_revision`, `archived` \
             and `readiness`. `readiness` is `dormant` (nothing is loaded; its next delivery \
             wakes it), `live`, `broken`, `preparation_pending` (it waits for its host to \
             prepare it: no member, no message taken, nothing runs until the host settles \
             that; no tool of yours does), `retiring` or `unseated` \
             (archived, or its seat is owed). An entry also has `addressability` when the \
             agent is `internal_only` (who reaches it; an addressable agent says nothing) \
             and `runtime_mode_override` (`autonomous_host` or `turn_driven`) when the \
             agent's record names how it runs instead of its profile. {READ_BACK_ONLY}\n\n\
             `profile` narrows the list to one profile; `include_archived` adds archived \
             agents; when the result has `next`, pass it as `after` to read on."
        ),
        IDENTITY_GET => format!(
            "Read one long-term agent of your mob by name: what identity_list says of it \
             (`name`, `display_name`, `profile`, `spec_digest`, `spec_revision`, `archived`, \
             `readiness`, and `addressability` and `runtime_mode_override` when the agent \
             has them) and the rest of what a create or an update sets: its \
             `instructions`, `context`, `labels` and `peers` (the peers its changes listed, \
             among the agents you are shown). An archived agent is read too, and so is your \
             own record. An agent that waits for its host's preparation reads `readiness: \
             \"preparation_pending\"`: it takes no message until the host settles that. \
             Read only: it never wakes an agent and loads nothing.\n\n\
             You are not shown who asked for the agent, when, or under which consent. \
             {UNKNOWN_NAME} {READ_BACK_ONLY}\n\n\
             A change of the agent sends its `spec_digest` as `expected_spec_digest`."
        ),
        IDENTITY_DIRECTORY => "See what you may name before you create or change a long-term \
             agent: `profiles`, the profiles of your mob, each with whether you may create \
             an agent of it, and `peers`, the agents of your mob that are neither archived \
             nor private to a person, each with whether you may list it as a peer. Read \
             only: it never wakes an agent and loads nothing.\n\n\
             Every entry has a `verdict`: `allowed`; `needs_decision` (a person decides a \
             request that names it, and `lines` is what they are shown); `not_allowed` \
             (with `reason` when the host's rule gives one).\n\n\
             `query` keeps the entries whose name or display name contains the text; when \
             the result has `next`, pass it as `after` to read on."
            .to_owned(),
        _ => String::new(),
    }
}

fn request_id_schema(effect: &str) -> Value {
    json!({
        "type": "string",
        "description": format!(
            "Your own id for this request. The same id with the same arguments returns the \
             same state and never {effect} twice."
        ),
    })
}

fn one_agent_schema(name: &str, digest_source: &str, effect: &str) -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["name", "expected_spec_digest"],
        "properties": {
            "name": { "type": "string", "description": name },
            "expected_spec_digest": {
                "type": "string",
                "description": format!(
                    "The agent's `spec_digest` as you last read it ({digest_source}). A \
                     changed agent answers `stale_spec` with `current_digest`."
                ),
            },
            "request_id": request_id_schema(effect),
        },
    })
}

/// The argument schema of the identity tool `name`: closed at every depth,
/// and what a call's arguments are checked against.
fn input_schema(name: &str) -> Value {
    match name {
        IDENTITY_CREATE => json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["agents"],
            "properties": {
                "agents": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": CORE_BATCH_MAX,
                    "description": format!(
                        "The agents to create, 1 to {CORE_BATCH_MAX}: all of them are \
                         created together, or none is."
                    ),
                    "items": {
                        "type": "object",
                        "additionalProperties": false,
                        "required": ["name", "profile"],
                        "properties": {
                            "name": {
                                "type": "string",
                                "description": "The agent's name: unique in the mob, how it \
                                    is addressed, never changed, and never available again \
                                    once the agent is removed.",
                            },
                            "profile": {
                                "type": "string",
                                "description": "One of the mob's profiles. It gives the \
                                    agent's model, tools and skills. identity_directory \
                                    lists the profiles you may use.",
                            },
                            "display_name": {
                                "type": "string",
                                "description": "The name people see for it. It can be \
                                    changed later; the name cannot.",
                            },
                            "instructions": {
                                "type": "array",
                                "items": { "type": "string" },
                                "description": "The agent's standing brief: who it is and \
                                    what it is for. One string per section; most agents \
                                    need one.",
                            },
                            "context": {
                                "description": "Descriptive data about the agent, any JSON \
                                    value. It describes the agent and is never authority.",
                            },
                            "labels": {
                                "type": "object",
                                "additionalProperties": { "type": "string" },
                                "description": "Labels, key to value. The host reserves \
                                    some keys and refuses them.",
                            },
                            "first_message": {
                                "type": "string",
                                "description": format!(
                                    "Runs once as the agent's first turn: a kickoff with no \
                                     requester, not a message from you. \
                                     {FIRST_MESSAGE_ANSWER} Leave it out to create the agent \
                                     dormant. {GOVERNED_FIRST_MESSAGE}"
                                ),
                            },
                            "peers": {
                                "type": "array",
                                "items": { "type": "string" },
                                "description": "The agents it can message, by name: agents \
                                    of the mob, or other agents of this call. Nothing is \
                                    wired to you unless you list your own name.",
                            },
                        },
                    },
                },
                "request_id": request_id_schema("creates"),
            },
        }),
        IDENTITY_UPDATE => json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["name", "expected_spec_digest"],
            "properties": {
                "name": { "type": "string", "description": "The agent to change." },
                "expected_spec_digest": {
                    "type": "string",
                    "description": "The agent's `spec_digest` as you last read it \
                        (identity_list, or the last result that named the agent). A changed \
                        agent answers `stale_spec` with `current_digest`.",
                },
                "set": {
                    "type": "object",
                    "additionalProperties": false,
                    "description": "What changes. A member you leave out stays as it is.",
                    "properties": {
                        "display_name": {
                            "type": "string",
                            "description": "The name people see for it.",
                        },
                        "instructions": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": format!(
                                "The standing brief, replaced whole. One string per \
                                 section; most agents need one. {INSTRUCTIONS_REACH}"
                            ),
                        },
                        "context": {
                            "description": "Descriptive data about the agent, replaced \
                                whole. Any JSON value.",
                        },
                        "labels": {
                            "type": "object",
                            "additionalProperties": { "type": "string" },
                            "description": "Labels to set: each key takes its value, and \
                                the agent's other labels stay.",
                        },
                        "labels_remove": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Label keys to remove.",
                        },
                        "profile": {
                            "type": "string",
                            "description": "Another profile of the mob: the agent moves to \
                                its model, tools and skills on the same conversation.",
                        },
                    },
                },
                "peers_add": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Agents it can message from now on, by name.",
                },
                "peers_remove": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Agents it can no longer message, by name.",
                },
                "request_id": request_id_schema("changes"),
            },
        }),
        IDENTITY_ARCHIVE => one_agent_schema("The agent to archive.", "identity_list", "archives"),
        IDENTITY_UNARCHIVE => one_agent_schema(
            "The archived agent to bring back.",
            "identity_list with `include_archived`",
            "unarchives",
        ),
        IDENTITY_REMOVE => {
            one_agent_schema("The agent to remove for good.", "identity_list", "removes")
        }
        IDENTITY_LIST => json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "profile": { "type": "string", "description": "Only agents of this profile." },
                "include_archived": {
                    "type": "boolean",
                    "description": "Also list archived agents. Default false.",
                },
                "after": {
                    "type": "string",
                    "description": "The previous page's `next`, to read on.",
                },
            },
        }),
        IDENTITY_GET => json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["name"],
            "properties": {
                "name": { "type": "string", "description": "The agent to read." },
            },
        }),
        IDENTITY_DIRECTORY => json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Only entries whose name or display name contains this \
                        text.",
                },
                "after": {
                    "type": "string",
                    "description": "The previous page's `next`, to read on.",
                },
            },
        }),
        _ => Value::Null,
    }
}

/// The member whose agent calls an identity tool, as the dispatch that
/// carries the call attributes it. Nothing of it is ever read from the
/// call's arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct IdentityToolCaller {
    /// The member's identity name.
    pub identity: String,
    /// The session the call ran in.
    pub session: SessionId,
    /// Whether the member is private to a person.
    pub private: bool,
}

impl IdentityToolCaller {
    /// The member `identity`, calling from `session`.
    pub fn new(identity: impl Into<String>, session: SessionId) -> Self {
        Self {
            identity: identity.into(),
            session,
            private: false,
        }
    }

    /// This caller, marked a member private to a person when `private`.
    #[must_use]
    pub fn private_to_a_person(mut self, private: bool) -> Self {
        self.private = private;
        self
    }

    /// The caller as the family takes a requester.
    pub fn requester(&self) -> ChangeRequester {
        ChangeRequester::agent_session(self.identity.clone(), self.session.clone())
            .private_to_a_person(self.private)
    }
}

/// Where one tool call ran within its session, as the dispatch that carries
/// it says: what names the call when its arguments give no `request_id`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct IdentityToolCallSite<'a> {
    /// The run of the session the call belongs to, and the zero-based round
    /// of that run; `None` for a call dispatched outside a run's round.
    pub round: Option<(&'a str, u64)>,
    /// The call's id, as the model's response gave it. It is unique only
    /// within that one response.
    pub call_id: &'a str,
}

impl<'a> IdentityToolCallSite<'a> {
    /// The call `call_id` of round `round` of the run `run_id`.
    pub fn in_round(run_id: &'a str, round: u64, call_id: &'a str) -> Self {
        Self {
            round: Some((run_id, round)),
            call_id,
        }
    }

    /// The call `call_id`, dispatched outside any run's round.
    pub fn outside_a_run(call_id: &'a str) -> Self {
        Self {
            round: None,
            call_id,
        }
    }
}

/// What an identity tool answers: the result as the model reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct IdentityToolAnswer {
    /// The result: a change's typed outcome, or a read's answer.
    pub content: Value,
    /// Whether the call was refused: its result is then an error result.
    pub refused: bool,
}

impl IdentityToolAnswer {
    fn outcome(outcome: &IdentityOutcome) -> Self {
        Self {
            content: outcome.to_json(),
            refused: outcome.refusal().is_some(),
        }
    }

    fn refusal(refusal: IdentityRefusal) -> Self {
        Self::outcome(&IdentityOutcome::Refused(refusal))
    }

    /// The result's text, as the tool result carries it.
    pub fn text(&self) -> String {
        self.content.to_string()
    }
}

/// Why an identity tool call has no answer of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum IdentityToolFault {
    /// The name is none of the identity tools.
    UnknownTool,
    /// The family answered a read with another read's answer.
    UnexpectedAnswer,
}

impl std::fmt::Display for IdentityToolFault {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::UnknownTool => "not an identity tool",
            Self::UnexpectedAnswer => "the identity family answered another read",
        })
    }
}

impl std::error::Error for IdentityToolFault {}

/// The identity tools over one mob's family.
#[derive(Clone)]
pub struct IdentityAgentTools {
    family: Arc<dyn IdentityFamily>,
    mob: MobHandle,
}

impl std::fmt::Debug for IdentityAgentTools {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("IdentityAgentTools")
            .field("mob", self.mob.mob_id())
            .finish_non_exhaustive()
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateArguments {
    agents: Vec<AgentSpec>,
    #[serde(default)]
    request_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateArguments {
    name: String,
    expected_spec_digest: String,
    #[serde(default)]
    set: AgentSet,
    #[serde(default)]
    peers_add: Vec<String>,
    #[serde(default)]
    peers_remove: Vec<String>,
    #[serde(default)]
    request_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OneAgentArguments {
    name: String,
    expected_spec_digest: String,
    #[serde(default)]
    request_id: Option<String>,
}

/// Which read a tool asked of the family: its answer must be that read's.
#[derive(Clone, Copy)]
enum Asked {
    List,
    Get,
    Directory,
}

fn malformed(detail: String) -> IdentityRefusal {
    IdentityRefusal::new(IdentityRefusalKind::InvalidRequest {
        problem: RequestProblem::Malformed,
    })
    .with_detail(detail)
}

fn member_path(path: &str, member: &str) -> String {
    if path.is_empty() {
        member.to_owned()
    } else {
        format!("{path}.{member}")
    }
}

/// `value` as `schema` admits it, with `null` members of an object left
/// out; refused `invalid_request` at the first place it leaves the schema:
/// a member the schema does not name, a required member left out, or a
/// value of another type. `path` is where `value` stands in the arguments.
fn closed(tool: &str, schema: &Value, value: Value, path: &str) -> Result<Value, IdentityRefusal> {
    let wrong = |expected: &str| {
        let what = if path.is_empty() {
            format!("The arguments of {tool} must be {expected}.")
        } else {
            format!("`{path}` must be {expected}.")
        };
        let refusal = malformed(what);
        if path.is_empty() {
            refusal
        } else {
            refusal.at(path)
        }
    };
    match schema.get("type").and_then(Value::as_str) {
        Some("object") => {
            let Value::Object(members) = value else {
                return Err(wrong("an object"));
            };
            let properties = schema.get("properties").and_then(Value::as_object);
            let additional = schema
                .get("additionalProperties")
                .filter(|rule| rule.is_object());
            let mut admitted = Map::new();
            for (name, member) in members {
                let at = member_path(path, &name);
                let declared = properties.and_then(|properties| properties.get(&name));
                let member = match (declared, additional) {
                    // A null member is a member left out.
                    (Some(_), _) if member.is_null() => continue,
                    (Some(rule), _) | (None, Some(rule)) => closed(tool, rule, member, &at)?,
                    (None, None) => {
                        return Err(malformed(format!(
                            "`{at}` is not an argument of {tool}: leave it out."
                        ))
                        .at(at));
                    }
                };
                admitted.insert(name, member);
            }
            for required in schema
                .get("required")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                if !admitted.contains_key(required) {
                    let at = member_path(path, required);
                    return Err(malformed(format!("`{at}` is required.")).at(at));
                }
            }
            Ok(Value::Object(admitted))
        }
        Some("array") => {
            let Value::Array(items) = value else {
                return Err(wrong("a list"));
            };
            let Some(item_schema) = schema.get("items") else {
                return Ok(Value::Array(items));
            };
            items
                .into_iter()
                .enumerate()
                .map(|(index, item)| closed(tool, item_schema, item, &format!("{path}[{index}]")))
                .collect::<Result<Vec<_>, _>>()
                .map(Value::Array)
        }
        Some("string") if !value.is_string() => Err(wrong("text")),
        Some("boolean") if !value.is_boolean() => Err(wrong("true or false")),
        _ => Ok(value),
    }
}

/// The arguments of `tool` as its schema admits them, parsed as `T`.
fn parse<T: for<'de> Deserialize<'de>>(tool: &str, arguments: &str) -> Result<T, IdentityRefusal> {
    let value = match serde_json::from_str::<Value>(arguments) {
        Ok(Value::Null) => Value::Object(Map::new()),
        Ok(value) => value,
        Err(_) => {
            return Err(malformed(format!(
                "The arguments of {tool} must be an object."
            )));
        }
    };
    let value = closed(tool, &input_schema(tool), value, "")?;
    serde_json::from_value(value).map_err(|error| malformed(error.to_string()))
}

fn spec_digest(value: &str) -> Result<Sha256Digest, IdentityRefusal> {
    Sha256Digest::parse(value).ok_or_else(|| {
        malformed(
            "`expected_spec_digest` is `sha256:` and 64 lowercase hex digits: send the agent's \
             `spec_digest` whole, as identity_list gave it."
                .to_owned(),
        )
        .at("expected_spec_digest")
    })
}

impl IdentityAgentTools {
    /// The identity tools over `family`, in `mob`.
    pub fn new(family: Arc<dyn IdentityFamily>, mob: MobHandle) -> Self {
        Self { family, mob }
    }

    /// The mob the tools change.
    pub fn mob(&self) -> &MobHandle {
        &self.mob
    }

    /// Whether the live grant of the member session `session` admits spawn
    /// tooling in the mob: manage scope over it, or spawn scope for one of
    /// its profiles. These are the reads Core's admission of an agent's
    /// change makes for the same session; a session the mob did not build,
    /// and one whose build bound no mob authority, hold none.
    pub fn grant_admits_spawn_tooling(&self, session: &SessionId) -> bool {
        self.mob.member_manage_scope_contains(session)
            || self.mob.definition().profiles.keys().any(|profile| {
                self.mob
                    .member_spawn_profile_scope_contains(session, profile.as_str())
            })
    }

    /// The operation of a change by `caller`: its `request_id`, or where
    /// its call ran.
    fn operation_id(
        &self,
        caller: &IdentityToolCaller,
        site: &IdentityToolCallSite<'_>,
        request_id: Option<&str>,
    ) -> Result<OperationId, IdentityRefusal> {
        let requester = caller.requester().recorded();
        if let Some(request_id) = request_id {
            return OperationId::for_request(&requester, request_id);
        }
        let Some((run_id, round)) = site.round else {
            return Err(malformed(
                "This call runs outside a run, so no request id can be derived for it: pass \
                 `request_id`."
                    .to_owned(),
            )
            .at("request_id"));
        };
        // A call id is unique within one model response only: the round
        // names the response within the run.
        Ok(OperationId::for_tool_call(
            &requester,
            &ToolCallOrigin::new(
                self.mob.mob_id().as_str(),
                &caller.session.to_string(),
                run_id,
                &round.to_string(),
                site.call_id,
            ),
        ))
    }

    async fn change(
        &self,
        caller: &IdentityToolCaller,
        site: &IdentityToolCallSite<'_>,
        change: IdentityChangeSpec,
    ) -> IdentityToolAnswer {
        let operation_id = match self.operation_id(caller, site, change.request_id()) {
            Ok(operation_id) => operation_id,
            Err(refusal) => return IdentityToolAnswer::refusal(refusal),
        };
        let outcome = self
            .family
            .change(
                &self.mob,
                &caller.requester(),
                IdentityChangeCall::new(operation_id, change),
            )
            .await;
        IdentityToolAnswer::outcome(&outcome)
    }

    fn one_agent(tool: &str, arguments: &str) -> Result<AgentRef, IdentityRefusal> {
        let arguments: OneAgentArguments = parse(tool, arguments)?;
        let mut agent = AgentRef::new(
            arguments.name,
            spec_digest(&arguments.expected_spec_digest)?,
        );
        agent.request_id = arguments.request_id;
        Ok(agent)
    }

    fn change_of(tool: &str, arguments: &str) -> Result<IdentityChangeSpec, IdentityRefusal> {
        Ok(match tool {
            IDENTITY_CREATE => {
                let arguments: CreateArguments = parse(tool, arguments)?;
                let mut create = CreateAgents::new(arguments.agents);
                create.request_id = arguments.request_id;
                IdentityChangeSpec::Create(create)
            }
            IDENTITY_UPDATE => {
                let arguments: UpdateArguments = parse(tool, arguments)?;
                let mut update = UpdateAgent::new(
                    arguments.name,
                    spec_digest(&arguments.expected_spec_digest)?,
                );
                update.request_id = arguments.request_id;
                update.set = arguments.set;
                update.peers_add = arguments.peers_add;
                update.peers_remove = arguments.peers_remove;
                IdentityChangeSpec::Update(update)
            }
            IDENTITY_ARCHIVE => IdentityChangeSpec::Archive(Self::one_agent(tool, arguments)?),
            IDENTITY_UNARCHIVE => IdentityChangeSpec::Unarchive(Self::one_agent(tool, arguments)?),
            _ => IdentityChangeSpec::Remove(Self::one_agent(tool, arguments)?),
        })
    }

    async fn read(
        &self,
        caller: &IdentityToolCaller,
        read: Result<IdentityRead, IdentityRefusal>,
    ) -> Result<IdentityToolAnswer, IdentityToolFault> {
        let read = match read {
            Ok(read) => read,
            Err(refusal) => return Ok(IdentityToolAnswer::refusal(refusal)),
        };
        // An agent is not shown who asked for an agent, when, or under
        // which consent: the host's read keeps it. An open operation's id
        // tells who asked too, so it stays there as well. Nor is an agent
        // shown where an agent's record lives (whether the host's roster
        // lists the name as well): that, too, is the host's to read.
        let shown = |agent: &mut AgentSummary| {
            agent.provenance = None;
            agent.open_operations.clear();
            agent.roster_listing_superseded = false;
        };
        let asked = match &read {
            IdentityRead::List(_) => Asked::List,
            IdentityRead::Get(_) => Asked::Get,
            IdentityRead::Directory(_) => Asked::Directory,
            // No tool asks any other read of the family.
            _ => return Err(IdentityToolFault::UnexpectedAnswer),
        };
        let answer = self.family.read(&self.mob, &caller.requester(), read).await;
        let content = match (asked, answer) {
            (_, Err(refusal)) => return Ok(IdentityToolAnswer::refusal(refusal)),
            (Asked::List, Ok(IdentityReadAnswer::List(mut page))) => {
                page.agents.iter_mut().for_each(shown);
                serde_json::to_value(page)
            }
            (Asked::Get, Ok(IdentityReadAnswer::Get(mut record))) => {
                shown(&mut record.agent);
                serde_json::to_value(record)
            }
            (Asked::Directory, Ok(IdentityReadAnswer::Directory(mut page))) => {
                for peer in &mut page.peers {
                    peer.roster_listing_superseded = false;
                }
                serde_json::to_value(page)
            }
            (_, Ok(_)) => return Err(IdentityToolFault::UnexpectedAnswer),
        };
        content
            .map(|content| IdentityToolAnswer {
                content,
                refused: false,
            })
            .map_err(|_| IdentityToolFault::UnexpectedAnswer)
    }

    /// One call of the identity tool `tool` by `caller`, with the call's
    /// `arguments` as the model wrote them, at `site`.
    pub async fn call(
        &self,
        caller: &IdentityToolCaller,
        site: &IdentityToolCallSite<'_>,
        tool: &str,
        arguments: &str,
    ) -> Result<IdentityToolAnswer, IdentityToolFault> {
        match tool {
            IDENTITY_CREATE | IDENTITY_UPDATE | IDENTITY_ARCHIVE | IDENTITY_UNARCHIVE
            | IDENTITY_REMOVE => Ok(match Self::change_of(tool, arguments) {
                Ok(change) => self.change(caller, site, change).await,
                Err(refusal) => IdentityToolAnswer::refusal(refusal),
            }),
            IDENTITY_LIST => {
                self.read(
                    caller,
                    parse::<ListAgents>(tool, arguments).map(IdentityRead::List),
                )
                .await
            }
            IDENTITY_GET => {
                self.read(
                    caller,
                    parse::<GetAgent>(tool, arguments).map(IdentityRead::Get),
                )
                .await
            }
            IDENTITY_DIRECTORY => {
                self.read(
                    caller,
                    parse::<ReadDirectory>(tool, arguments).map(IdentityRead::Directory),
                )
                .await
            }
            _ => Err(IdentityToolFault::UnknownTool),
        }
    }
}
