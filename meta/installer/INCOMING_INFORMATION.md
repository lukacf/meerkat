# Incoming information in a Forge design

Forge's guide can retain which changes matter, who should receive them, and how
much existing history is useful. The proposal shows these choices under
**What reaches your team**. **Refine with your guide** opens a conversation about
the selected source; it does not start collection.

For example: "Send new changes from our selected project spaces to the project
owner, and let an intake specialist judge ambiguous updates." Name the intended
audience as well as the recipient. Access still needs to be verified during
implementation and delivery.

## What the saved brief means

`brief.incoming_data` is optional design intent. Each source links to a requested
feature through `requirement_id` and cites user messages through
`source_message_ids`. The linked requirement records implementation gaps and
scope decisions. Marking a feature deferred, cancelled or handed off requires a
later user decision; it does not erase the source from the displayed proposal.
Every revision retains existing source IDs. The guide can revise their choices
and add sources, but omitting the field, dropping an item or replacing its ID
is rejected. This keeps an unrelated answer from silently losing saved scope,
recipients or history choices. Briefs that have never recorded a source can
continue to omit the field.

| Choice | Meaning |
| --- | --- |
| `scope`, `purpose`, `audience` | Descriptions of the requested information, its use and intended audience. These fields are not verified provider identifiers or access grants. |
| `routes: []` | Recipients remain undecided. Source discovery can precede a team proposal. |
| Identity route | Names an identity in the proposed or saved team. `direct` receives matching updates; `triage` judges relevance; `conversation` owns participation in the conversation. |
| Class route | Uses `recipient_kind: "class"` and `delivery: "direct"` for all current and future members of a proposed class, including a class with zero initial members. |
| `history: "undecided"` | Keeps the history decision open. This is the default. |
| `history: "new_only"` | Requests new changes only. |
| `history: "bounded_history"` | Requires a nonblank `history_window`, such as "the current project's last two weeks". Other history choices must leave the window empty. |

A source can have different recipients, but each recipient gets one route for
that source. Duplicate routes and overlap between a class and one of its known
members are rejected. Triage and conversation ownership must name one identity.
A replacement team is checked before publication, so removing a recipient or
introducing an overlap requires updating the routes in the same proposal.
The initial compiler scaffolding is not a proposed team.

The guide validates the complete publication before saving it. A rejected
publication leaves the saved brief, proposal, conversation and version unchanged.
A corrected response can then be submitted against that same version.

A guide reply that leaves the design unchanged can use the saved team's
recipients, whether it explains the current status or asks the next question.
It does not need to compile or stage a replacement team. This also applies to
direct routes for classes with zero initial members. Such a reply is still
rejected when a route names a missing recipient or a source cites non-user
evidence; initial compiler scaffolding does not qualify as a saved team.

## Implementation boundary

These choices do not create native sources, subscriptions, permissions or
collectors. The intended shared implementation uses Meta mob announcements and
Connect observation adapters, with Core owning scheduling and delivery admission.
The selected runtime and adapter must expose the required contracts before a
source can be described as supported setup. Existing MCP tools alone do not
establish a changefeed.

Do not put invented provider scopes, verified principals, collector bindings or
cursor tokens into `incoming_data`. Unknown fields are rejected. Forge does not
use this brief as a separate polling loop, inbox or cursor store. A class route
also does not prove that future members have access, or that an agent cannot be
woken twice. Searchable knowledge, retained announcements, delivered information
and completed work need their own evidence.

## Contributor checks

The models and publication checks are in
[`launchkit/guide_models.py`](launchkit/guide_models.py). The rendered review is
[`web/incoming-data-review.jsx`](web/incoming-data-review.jsx). Omitted or empty
`incoming_data` is omitted from serialized briefs; an identity route omits the
default `recipient_kind`, preserving the existing format.

An exact identity reference can contain the full 40-character class ID, a colon,
and a 20-character named-member key: 61 characters in total. Incoming recipients,
topology endpoints, explicit authoring selectors, workflow targets and acquisition
helpers use that same reference bound. The class and member declaration bounds remain unchanged;
every reference must still name an identity in the proposed or saved team.

From the repository root, using an environment with the installer test
dependencies:

```bash
PYTHONPATH=installer:sdk/python python -m pytest \
  installer/tests/test_incoming_data_intent.py \
  installer/tests/test_identity_reference_bounds.py \
  installer/tests/test_guided_edit_publication.py -q
node --test installer/web/incoming-data-review.test.mjs
```

The Python tests exercise publication validation and temporary SQLite guide
storage, including reopen after rejection, a corrected retry, and unchanged
guided replies through offline native-transport fixtures. The UI tests
render the component and inspect its refinement callback. They do not prove
provider access, collection, native delivery, restart recovery of a collector,
or behavior in a running installation.
