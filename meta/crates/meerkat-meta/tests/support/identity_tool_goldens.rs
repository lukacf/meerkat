//! The pinned text of the identity tools: each tool's name, description and
//! argument schema (pretty JSON), exactly as a model reads them.
//!
//! This is product text. A change here is a deliberate change of what an
//! agent is told: `every_identity_tool_text_is_pinned` prints the new pins
//! when run with `UPDATE_IDENTITY_TOOL_GOLDENS=1`.

pub const PINS: &[(&str, &str, &str)] = &[
    (
        "identity_create",
        r#"Create one or more long-term agents in your mob.

A long-term agent is a durable, named member of the mob: it keeps its own conversation and memory across restarts, is addressed by its name, and is the same kind of agent as one the host configured. You are recorded as who asked for it. You do not own it, it does not report to you, and it is not limited to your tools. For a helper that does one task and is discarded, use the worker tools (delegate, fork_off, mob_spawn_member) instead.

Each agent takes a unique `name` and a `profile` of the mob, which gives its model, tools and skills. Call identity_directory first: it lists the profiles you may use and the agents you may name as peers. Optional: `display_name`; `instructions`, its standing brief; `context`, descriptive data about it; `labels`; `peers`, the agents it can message; `first_message`.

`peers`: nothing is wired to you unless you list your own name. The agents of one call may name each other.

`first_message` runs once, as the new agent's first turn: a kickoff with no requester. It is not a message from you: the agent has nothing to reply to, and reaches you only if you are one of its peers. That turn has the same tools as any other turn of the agent. Write it as the agent's opening task. A create with a first message answers once the new agent's session exists; the first run continues after the answer. Without it the agent is created dormant: nothing runs until something is delivered to it. In a governed realm an agent is created without a first message: a create that gives one is refused, and the agent's first input comes through the host's own ingress.

All agents of one call (1 to 256) are created together or not at all. The host's rules decide what happens, and the result's `outcome` says which:
- `done`: created. `agents` lists each agent's `name`, `spec_digest` and `spec_revision`. `seat: "owed"` on an agent means its first message has not started: the agent exists, and it is seated and its first message runs at its next delivery.
- `pending_decision`: a person must decide (`decision_id`, `deadline_ms`). Nothing is created yet. Do not repeat the call with new arguments: that is a new request. If you are a long-term agent yourself, a notice reaches you when it is decided; in every case the identical call with the same `request_id` returns the current state.
- `pending_preparation`: created, and the host is still preparing at least one agent, which is not usable yet.
- `refused`: nothing was created. `kind` names why, `field` the argument it concerns, `detail` what to send instead. Repeat the same call later only when `retryable` is true.

Pass `request_id` when you may need to ask again: the same id with the same arguments returns the same state and never creates twice, and the same id with other arguments is refused `operation_reused`. Without it, only a retry of this same tool call is recognized as the same request.

A name that was removed is never available again (`name_taken`)."#,
        r#"{
  "additionalProperties": false,
  "properties": {
    "agents": {
      "description": "The agents to create, 1 to 256: all of them are created together, or none is.",
      "items": {
        "additionalProperties": false,
        "properties": {
          "context": {
            "description": "Descriptive data about the agent, any JSON value. It describes the agent and is never authority."
          },
          "display_name": {
            "description": "The name people see for it. It can be changed later; the name cannot.",
            "type": "string"
          },
          "first_message": {
            "description": "Runs once as the agent's first turn: a kickoff with no requester, not a message from you. A create with a first message answers once the new agent's session exists; the first run continues after the answer. Leave it out to create the agent dormant. In a governed realm an agent is created without a first message: a create that gives one is refused, and the agent's first input comes through the host's own ingress.",
            "type": "string"
          },
          "instructions": {
            "description": "The agent's standing brief: who it is and what it is for. One string per section; most agents need one.",
            "items": {
              "type": "string"
            },
            "type": "array"
          },
          "labels": {
            "additionalProperties": {
              "type": "string"
            },
            "description": "Labels, key to value. The host reserves some keys and refuses them.",
            "type": "object"
          },
          "name": {
            "description": "The agent's name: unique in the mob, how it is addressed, never changed, and never available again once the agent is removed.",
            "type": "string"
          },
          "peers": {
            "description": "The agents it can message, by name: agents of the mob, or other agents of this call. Nothing is wired to you unless you list your own name.",
            "items": {
              "type": "string"
            },
            "type": "array"
          },
          "profile": {
            "description": "One of the mob's profiles. It gives the agent's model, tools and skills. identity_directory lists the profiles you may use.",
            "type": "string"
          }
        },
        "required": [
          "name",
          "profile"
        ],
        "type": "object"
      },
      "maxItems": 256,
      "minItems": 1,
      "type": "array"
    },
    "request_id": {
      "description": "Your own id for this request. The same id with the same arguments returns the same state and never creates twice.",
      "type": "string"
    }
  },
  "required": [
    "agents"
  ],
  "type": "object"
}"#,
    ),
    (
        "identity_update",
        r#"Change a long-term agent of your mob: its display name, instructions, context, labels, profile or peers.

Send `expected_spec_digest`: the agent's `spec_digest` from identity_list, or from the last result that named it. If the agent changed meanwhile you get `stale_spec` with `current_digest`: read the agent again, then decide whether your change still applies.

`set` names what changes; a member you leave out stays as it is. `instructions` and `context` are replaced whole. `labels` sets each key you give and keeps the others; `labels_remove` removes keys. `profile` moves the agent to that profile's model, tools and skills on the same conversation. `peers_add` and `peers_remove` change who it can message.

An instructions change is in the agent's record at once. It reaches the agent at its next run; a run that starts within moments of the change may still use the earlier instructions; earlier instructions are never rewritten. The result says what became of it in `agents[0].instructions.kind`: `applies_at_next_run`; `applies_after_compaction` (the agent's model takes new instructions only at the head of what it reads, so the change reaches it after its next compaction); `applies_at_next_session` with a `reason` (the change does not reach the agent as it runs now, and applies when it next starts a new conversation); `unchanged`.

You cannot change yourself (`self_change_refused`). An agent that the host's configuration declares is changed there, not here (`owner_read_only`).

The result's `outcome` is `done`, `pending_decision`, `pending_preparation` or `refused`, and `request_id` works, as for identity_create."#,
        r#"{
  "additionalProperties": false,
  "properties": {
    "expected_spec_digest": {
      "description": "The agent's `spec_digest` as you last read it (identity_list, or the last result that named the agent). A changed agent answers `stale_spec` with `current_digest`.",
      "type": "string"
    },
    "name": {
      "description": "The agent to change.",
      "type": "string"
    },
    "peers_add": {
      "description": "Agents it can message from now on, by name.",
      "items": {
        "type": "string"
      },
      "type": "array"
    },
    "peers_remove": {
      "description": "Agents it can no longer message, by name.",
      "items": {
        "type": "string"
      },
      "type": "array"
    },
    "request_id": {
      "description": "Your own id for this request. The same id with the same arguments returns the same state and never changes twice.",
      "type": "string"
    },
    "set": {
      "additionalProperties": false,
      "description": "What changes. A member you leave out stays as it is.",
      "properties": {
        "context": {
          "description": "Descriptive data about the agent, replaced whole. Any JSON value."
        },
        "display_name": {
          "description": "The name people see for it.",
          "type": "string"
        },
        "instructions": {
          "description": "The standing brief, replaced whole. One string per section; most agents need one. It reaches the agent at its next run; a run that starts within moments of the change may still use the earlier instructions; earlier instructions are never rewritten.",
          "items": {
            "type": "string"
          },
          "type": "array"
        },
        "labels": {
          "additionalProperties": {
            "type": "string"
          },
          "description": "Labels to set: each key takes its value, and the agent's other labels stay.",
          "type": "object"
        },
        "labels_remove": {
          "description": "Label keys to remove.",
          "items": {
            "type": "string"
          },
          "type": "array"
        },
        "profile": {
          "description": "Another profile of the mob: the agent moves to its model, tools and skills on the same conversation.",
          "type": "string"
        }
      },
      "type": "object"
    }
  },
  "required": [
    "name",
    "expected_spec_digest"
  ],
  "type": "object"
}"#,
    ),
    (
        "identity_archive",
        r#"Stop a long-term agent and keep it. Its name, record, conversation and memory stay; it has no running member, messages to it are refused, and identity_unarchive brings it back on the same conversation. Use this when the agent may be needed later; identity_remove is for good.

Send the agent's `name` and `expected_spec_digest` (its `spec_digest` from identity_list; a changed agent answers `stale_spec` with `current_digest`). You cannot archive yourself (`self_change_refused`).

The result's `outcome` is `done` (the agent's `state` is `archiving` while its member stops, then `archived`), `pending_decision` or `refused`, and `request_id` works, as for identity_create."#,
        r#"{
  "additionalProperties": false,
  "properties": {
    "expected_spec_digest": {
      "description": "The agent's `spec_digest` as you last read it (identity_list). A changed agent answers `stale_spec` with `current_digest`.",
      "type": "string"
    },
    "name": {
      "description": "The agent to archive.",
      "type": "string"
    },
    "request_id": {
      "description": "Your own id for this request. The same id with the same arguments returns the same state and never archives twice.",
      "type": "string"
    }
  },
  "required": [
    "name",
    "expected_spec_digest"
  ],
  "type": "object"
}"#,
    ),
    (
        "identity_unarchive",
        r#"Bring an archived long-term agent back. It returns on the conversation it had, with its name, memory and current instructions, and takes messages again.

Send the agent's `name` and `expected_spec_digest` (its `spec_digest` from identity_list with `include_archived: true`). An agent that is not archived answers `not_archived`. You cannot unarchive yourself (`self_change_refused`).

The result's `outcome` is `done` (the agent's `state` is `restored`), `pending_decision` or `refused`, and `request_id` works, as for identity_create."#,
        r#"{
  "additionalProperties": false,
  "properties": {
    "expected_spec_digest": {
      "description": "The agent's `spec_digest` as you last read it (identity_list with `include_archived`). A changed agent answers `stale_spec` with `current_digest`.",
      "type": "string"
    },
    "name": {
      "description": "The archived agent to bring back.",
      "type": "string"
    },
    "request_id": {
      "description": "Your own id for this request. The same id with the same arguments returns the same state and never unarchives twice.",
      "type": "string"
    }
  },
  "required": [
    "name",
    "expected_spec_digest"
  ],
  "type": "object"
}"#,
    ),
    (
        "identity_remove",
        r#"Remove a long-term agent for good. Its member stops, its peer links end, and its name can never be used again, by anyone. Its conversation and memory are kept, and nothing can reach them. This cannot be undone: use identity_archive when the agent may be needed later.

Send the agent's `name` and `expected_spec_digest` (its `spec_digest` from identity_list; a changed agent answers `stale_spec` with `current_digest`). You cannot remove yourself (`self_change_refused`).

The result's `outcome` is `done` (the agent's `state` is `retiring` while its member stops, then `removed`), `pending_decision` or `refused`, and `request_id` works, as for identity_create."#,
        r#"{
  "additionalProperties": false,
  "properties": {
    "expected_spec_digest": {
      "description": "The agent's `spec_digest` as you last read it (identity_list). A changed agent answers `stale_spec` with `current_digest`.",
      "type": "string"
    },
    "name": {
      "description": "The agent to remove for good.",
      "type": "string"
    },
    "request_id": {
      "description": "Your own id for this request. The same id with the same arguments returns the same state and never removes twice.",
      "type": "string"
    }
  },
  "required": [
    "name",
    "expected_spec_digest"
  ],
  "type": "object"
}"#,
    ),
    (
        "identity_list",
        r#"List the long-term agents of your mob that your grant covers: every agent of a profile you may create agents of, whoever asked for it and however it came to exist. Read only: it never wakes an agent and loads nothing.

Each entry of `agents` has `name`, `display_name`, `profile`, `spec_digest` (what a change sends as `expected_spec_digest`), `spec_revision`, `archived` and `readiness`. `readiness` is `dormant` (nothing is loaded; its next delivery wakes it), `live`, `broken`, `preparation_pending`, `retiring` or `unseated` (archived, or its seat is owed).

`profile` narrows the list to one profile; `include_archived` adds archived agents; when the result has `next`, pass it as `after` to read on."#,
        r#"{
  "additionalProperties": false,
  "properties": {
    "after": {
      "description": "The previous page's `next`, to read on.",
      "type": "string"
    },
    "include_archived": {
      "description": "Also list archived agents. Default false.",
      "type": "boolean"
    },
    "profile": {
      "description": "Only agents of this profile.",
      "type": "string"
    }
  },
  "type": "object"
}"#,
    ),
    (
        "identity_directory",
        r#"See what you may name before you create or change a long-term agent: `profiles`, the profiles of your mob, each with whether you may create an agent of it, and `peers`, the agents of your mob, each with whether you may list it as a peer. Read only: it never wakes an agent and loads nothing.

Every entry has a `verdict`: `allowed`; `needs_decision` (a person decides a request that names it, and `lines` is what they are shown); `not_allowed` (with `reason` when the host's rule gives one).

`query` keeps the entries whose name or display name contains the text; when the result has `next`, pass it as `after` to read on."#,
        r#"{
  "additionalProperties": false,
  "properties": {
    "after": {
      "description": "The previous page's `next`, to read on.",
      "type": "string"
    },
    "query": {
      "description": "Only entries whose name or display name contains this text.",
      "type": "string"
    }
  },
  "type": "object"
}"#,
    ),
];
