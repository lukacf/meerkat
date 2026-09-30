# OB3 product review of r2

Received via agent bus from `ob3`, host `Lukas-BigMac`, at 2026-09-30T20:01:38+00:00.
Message: `20260930T200138.457300-ob3-9401c1.json`. The text below preserves the review as received.

ADR-001 r2 adversarial review, OB3 product integration. Reviewer: ob3 (Claude, OB3 validator lane).
Candidate: r2, 568 lines, SHA-256 44882a1ba3c8ea36d9a44c13df6c4b52b246c459dacc93437ef188ea7ac055ca (verified on the local path).
OB3 facts are checked against ob3_validator origin/main (3.10.0-9084b80, now in production) unless marked 3.11 (codex/ob3-shared-intake).

VERDICT: RED, narrowly. The architecture direction is coherent. Three text-level gaps (F1-F3) leave the governed profile undefined or unsafe for a chat product with persistent agents, which is OB3's shape. Each needs a small normative addition, not a redesign. F4-F7 are material for OB3 but can be fixed with a clarification. Nonblocking requirements follow.

F1 [HIGH] Human conversational input has no classification authority. Sections 5 (l.266-270, 280-282) and 1 (l.76).
Counterexample: in a Slack DM, user A tells their personal agent "X is leaving the company, treat as confidential". Slack is the only source, and Slack assigns no classification. Section 5 says the facts "are provided by resource authority, not invented by the LLM", and unknown contributors "cannot be silently discarded". So A's message is either unknown, which by l.295-296 refuses every later use (the agent can never reply to A), or it is treated as unrestricted by some adapter default, which leaks. OB3 has the same problem in dashboard chat, Slack @mentions, thread replies and peer messages.
Not covered: l.211 only says caller-mutable labels are not clearance claims. Nothing assigns an authority-backed default to human-authored input.
Minimum change: make ingress (the Slack, console or comms adapter) the resource authority for message labels. Assign each message a declared default derived from the authenticated sender and the channel audience, for example DM = {sender, target agent's owner}; channel message = channel audience at post time. Senders may narrow the label, never widen it.
Acceptance: A's DM content cannot reach B's reply or a channel post. A's own follow-up reply works.

F2 [HIGH] Persisting derived output into an app-owned store is an ungoverned disclosure. Sections 5 (l.310-314) and 7.
Counterexample: the OB3 nightly review writes findings, snapshots, POR text (por_versions) and agent session bytes to BigQuery. Two later readers:
- the dashboard API, open to any authenticated @king.com user (google_auth.rs ALLOWED_DOMAIN; /initiatives, /stats and similar are not admin-gated)
- the separate ob3-dashboard repo, plus humans with dataset IAM
The ADR authorizes "release" at runtime output time. Here the real recipients are decided later, outside Meerkat, and labels do not survive into the rows.
Not covered: section 5 lists resources the runtime reads, but not app databases the runtime writes to and others read from.
Minimum change: state that writing governed content to a non-governed store is a disclosure to that store's full reader set. Either persist the dependency/label envelope with the row and make the app's read path an enforcing projection, or require the store's reader set to be authorized as a recipient at write time.
Acceptance: a finding derived from a restricted POR is not served by the dashboard to an unauthorized user, and not by a direct table read without label enforcement.

F3 [HIGH] Long-lived multi-contributor sessions monotonically accumulate restrictions, and no content-dropping boundary is defined. Sections 5 (l.280-289, 316-323) and 3.
Counterexample: OB3 initiative and channel agents have persistent sessions lasting months (BigQuerySessionStore, per-turn persistence). Their inputs come from many requesters: the review coordinator, subscribers' corrections, and the deep-investigator. Under the first profile's "complete selected context domain", one restricted document read in July attaches to every later output of that agent. Section 5 says compaction does not declassify. So after one restricted read, the channel agent can never post to its channel again, in a live product.
Not covered: section 5 defers message-level projection, but defines no epoch boundary that drops content, and therefore dependencies, without declassification.
Minimum change: define an authorized "context segment reset". A new segment carries no prior content and therefore no prior dependencies. The session-to-segment association is recorded. Any later reference to an old segment is a fresh authorized read, not inherited context. Resume and compaction stay inside a segment.
Acceptance: after a restricted read and a reset, a later channel post is allowed and carries no hidden dependency from the old segment. Without a reset, the post is refused.

F4 [MEDIUM-HIGH] Access-conferring attributes come from sources whose writers are not authorized grantors. Sections 1 (l.76) and 3.
Counterexamples in OB3 today:
(a) discovery.rs auto_subscribe_owners subscribes whoever the sheet's Owner cell names. Owner emails are inferred heuristically (King one-dot convention, Slack-directory fallback). Any sheet editor can therefore route an initiative's reviews and evidence to themselves or others.
(b) king-search-mcp: server.py _url_on_read_allowlist makes any URL present in ob3_history.document_allowlist readable regardless of Glean visibility, via allowed = king_public or on_allowlist. OB3's scope_ingest populates that table from sheet hyperlinks, so pasting a link into the sheet grants the bot a read, and via F2 dashboard readers a disclosure. (Today the dynamic list is empty because of a 403, but that is by accident.)
Not covered: "resource authority" names who is authoritative for an attribute's value. It does not require that the authority's writers be authorized to grant the access the attribute confers.
Minimum change: an attribute that confers read, recipient or delegation rights must come from an authority whose write path is itself an authorized grant or release decision. Otherwise it is advisory, for example "suggested subscriber: needs confirmation".
Acceptance: editing an Owner cell or pasting a link produces a pending suggestion, not a subscription or a readable document.

F5 [MEDIUM] Dynamic audiences and standing secondary disclosures are core to OB3, not optional. Section 5 (l.316-319).
Counterexamples:
- OB3 channel agents post to Slack channels whose membership Slack controls over time.
- SLACK_DM_COPY_CHANNEL (serve.rs:235, 373) copies every personal-agent DM into a monitoring channel, a standing policy-sanctioned second audience for all private DMs.
- SLACK_TEST_* redirects change the recipient.
Deferring dynamic audiences makes the governed profile unable to host OB3 at all. That is acceptable only if the ADR says so, but the DM copy channel is a design question the text should answer now.
Minimum change: model an external channel as a destination principal whose audience is resolved by its platform authority at send time under a declared freshness bound. Model monitoring copies and test redirects as explicit, auditable disclosure grants with their own audience, never as adapter configuration.
Acceptance: a DM copy is sent only under a recorded grant, and a channel post re-resolves membership within the bound.

F6 [MEDIUM] Admin is one blanket role today. Operator-through-agent is a confused deputy. Section 3 (l.186-189).
Counterexample: OB3 admins (DASHBOARD_ADMIN_EMAILS plus /admins/add) can call /chat/send and /chat/history on any agent (routes.rs admin_routes), including another user's personal agent, whose context holds that user's DMs. The operator becomes the requester, and the agent's reply draws on the owner's private content.
The ADR says policy, identity and credential administration are separate from data access. Good. It should also state explicitly that addressing an agent does not grant read of the agent's accumulated context. An agent is not a resource boundary; its context contributors are.
Acceptance: an admin chatting with B's personal agent cannot elicit B's DM content without a data-access grant for B's data, and transcript reads are authorized per contributor.

F7 [MEDIUM] An ambiguous receipt append is treated as failure. Section 7 (l.387-391, 428-431).
Counterexample: OB3's durable stores are BigQuery streaming inserts and DML. A client timeout frequently means the write committed (measured in OB3 incidents: "a client timeout is not a failed write"). "Failed persistence refuses new protected effects" then either refuses while the receipt exists, or retries and double-appends.
Minimum change: the receipt store contract defines Unknown for its own appends, requires idempotent append identity and reconciliation, and blocks only until reconciliation settles.
Acceptance: a timed-out append that actually committed produces exactly one receipt and no spurious refusal once reconciled.

NONBLOCKING IMPLEMENTATION REQUIREMENTS (OB3)
- N1. Sources without per-user access-control queries (Glean through king-search-mcp is read with a service credential; nightly work has no human requester): disclosure of their content must be limited to the source's own public projection unless the source can answer "may recipient R see document D" at release time. Please state this pattern explicitly next to service commissioning.
- N2. Multi-replica hosts: OB3 overlaps two pods during rolling updates (about 30 s measured on 2026-09-30, sometimes longer). Entry fences and leases must be cross-process for the same host type, not only for remote adapters.
- N3. Receipt latency: an OB3 review cycle is 130 initiatives times several tool calls each. Pre-effect durable receipts on BigQuery add seconds per effect, so a batched or low-latency receipt store is needed.
- N4. Subscriptions (task_subscriptions, self-service and admin-set) are standing recipient grants and should be modeled as such: who may subscribe whom to what, and revocation. OB3 also needs an explicit data-classification decision for review findings and evidence (internal-all versus initiative-restricted) before any governed profile is meaningful.
- N5. Model provider routes: OB3 uses OpenAI via Copilot and Anthropic with provider-side stored responses and prompt caching. The l.300-304 provider-held context rule applies directly.

Covered adequately in my view:
- actor/requester separation for nightly service work (the l.191-201 commissioning contract)
- flow retries and resume (l.158-163)
- model-chosen destinations such as send_to_slack channel arguments (l.206-209)
- revocation versus cannot-recall bytes (l.314)
