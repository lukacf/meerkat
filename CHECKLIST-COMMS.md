# Meerkat Comms Implementation Checklist

**Purpose:** Task-oriented checklist for agentic AI execution of the inter-agent communication system.

**Specification:** See `DESIGN-COMMS.md` for authoritative requirements.

---

## RCT Methodology Overview

This implementation follows the **RCT (Representation Contract Tests)** methodology. Key principles:

1. **Gate 0 Must Be Green First** - Representation contracts must pass before any behavior implementation
2. **Representations First, Behavior Second, Internals Last**
3. **Integration Tests Over Unit Tests** - System proof trumps local correctness
4. **No Platonic Completion** - Passing tests are the source of truth, not code quality

### RCT Gate Progression (Methodology Gates)
- **RCT Gate 0**: RCT tests green (round-trip, encoding, NULL semantics)
- **RCT Gate 1**: E2E scenarios written and executable (red OK)
- **RCT Gate 2**: Integration choke-point tests written and executable (red OK)
- **RCT Gate 3**: Unit tests as needed to support integration

> **Note:** These are **methodology gates**, separate from the Phase Gates below.

---

## Global Dependencies

- `meerkat-comms` crate must exist before any phase tasks
- Ed25519 crypto library (e.g., `ed25519-dalek`) required for Phase 1+
- CBOR library with canonical encoding (e.g., `ciborium`) required for Phase 0+
- All types in Phase 0 must compile before Phase 1+ can proceed

---

## Phase Completion Protocol

When completing a phase, use this checklist:

1. [ ] All phase checklist items marked complete
2. [ ] Run relevant tests, note results
3. [ ] Spawn reviewer agents IN PARALLEL (single message, multiple Task calls)
4. [ ] Collect all verdicts
5. [ ] If any BLOCK verdicts:
   - Address blocking issues
   - Re-run *all* reviewers
6. [ ] Record phase completion with reviewer verdicts
7. [ ] Proceed to next phase

---

## Phase 0: Representation Contracts (RCT Gate 0 - MUST BE GREEN)

**Goal:** Lock down all core types with serialization round-trips. No behavior yet.

**Dependencies:** None (this is the foundation)

### Tasks - Crate Setup

- [ ] Create `meerkat-comms/Cargo.toml` with workspace dependencies (Done when: `cargo check -p meerkat-comms` succeeds)
- [ ] Create `meerkat-comms/src/lib.rs` with module declarations (Done when: compiles with `mod model; mod identity;`)

### Tasks - Identity Types (`identity.rs`)

- [ ] Implement `PubKey` newtype in `identity.rs` (Done when: compiles with 32-byte array wrapper)
- [ ] Implement `NodeId` newtype in `identity.rs` (Done when: compiles as hash of PubKey)
- [ ] Implement `Signature` newtype in `identity.rs` (Done when: compiles with 64-byte array wrapper)
- [ ] Derive Serialize/Deserialize for `PubKey` (Done when: serde derives compile)
- [ ] Derive Serialize/Deserialize for `NodeId` (Done when: serde derives compile)
- [ ] Derive Serialize/Deserialize for `Signature` (Done when: serde derives compile)

### Tasks - Core Enums (`model.rs`)

- [ ] Implement `Role` enum in `model.rs` (Done when: compiles with Observer, Member, Operator, Admin variants)
- [ ] Implement `Status` enum in `model.rs` (Done when: compiles with Accepted, Rejected, Completed, Failed variants)
- [ ] Implement `PresenceState` enum in `model.rs` (Done when: compiles with Online, Away, Offline variants)
- [ ] Implement `Priority` enum in `model.rs` (Done when: compiles with High, Normal, Low variants)
- [ ] Implement `Destination` enum in `model.rs` (Done when: compiles with Channel(Uuid), Peer(PubKey) variants)
- [ ] Derive Serialize/Deserialize for all enums (Done when: serde derives compile for Role, Status, PresenceState, Priority, Destination)

### Tasks - MessageKind Enum (`model.rs`)

- [ ] Implement `MessageKind::Message` variant (Done when: compiles with `body: String`)
- [ ] Implement `MessageKind::Request` variant (Done when: compiles with request_id, intent, params, reply_to, hop fields)
- [ ] Implement `MessageKind::Response` variant (Done when: compiles with request_id, status, result fields)
- [ ] Implement `MessageKind::Presence` variant (Done when: compiles with state, seq fields)
- [ ] Implement `MessageKind::HistoryRequest` variant (Done when: compiles with channel_id, since_ts, since_seq_by_sender, max_messages, max_bytes fields)
- [ ] Implement `MessageKind::HistoryResponse` variant (Done when: compiles with channel_id, messages fields)
- [ ] Derive Serialize/Deserialize for `MessageKind` (Done when: serde derives compile)

### Tasks - Envelope Struct (`model.rs`)

- [ ] Implement `Envelope` struct (Done when: compiles with id, from_pubkey, to, kind, ts, ttl_secs, sender_seq, sender_epoch, sig fields per spec)
- [ ] Derive Serialize/Deserialize for `Envelope` (Done when: serde derives compile)

### Tasks - Roster Types (`model.rs`)

- [ ] Implement `RosterMember` struct (Done when: compiles with pubkey, role, archive fields)
- [ ] Implement `RosterSignature` struct (Done when: compiles with pubkey, sig fields)
- [ ] Implement `Roster` struct (Done when: compiles with org_id, version, timestamp, members, signatures fields)
- [ ] Derive Serialize/Deserialize for roster types (Done when: serde derives compile for RosterMember, RosterSignature, Roster)

### Tasks - Channel Types (`model.rs`)

- [ ] Implement `ChannelPolicy` struct (Done when: compiles with channel_id, name, readers, writers, admins, signature, retention_hours, retention_messages, allow_history_fetch fields)
- [ ] Derive Serialize/Deserialize for `ChannelPolicy` (Done when: serde derives compile)

### Tasks - Inbox Types (`model.rs`)

- [ ] Implement `InboxSource` enum (Done when: compiles with Channel(Uuid), Dm(PubKey) variants)
- [ ] Implement `InboxItem::External` variant (Done when: compiles with envelope, source, priority fields)
- [ ] Implement `InboxItem::SubagentResult` variant (Done when: compiles with subagent_id, result, summary fields)
- [ ] Derive Serialize/Deserialize for inbox types (Done when: serde derives compile)

### Tasks - Config Types (`model.rs`)

- [ ] Implement `ForkBudget` enum (Done when: compiles with Shared, Fixed(u64) variants)
- [ ] Implement `ForkConfig` struct (Done when: compiles with budget field)
- [ ] Derive Serialize/Deserialize for config types (Done when: serde derives compile)

### Tasks - Handshake Types (`model.rs`)

- [ ] Implement `Hello` struct (Done when: compiles with node_id, pubkey, nonce, roster_version fields)
- [ ] Implement `HelloAck` struct (Done when: compiles with node_id, pubkey, nonce, sig, roster_version fields)
- [ ] Derive Serialize/Deserialize for handshake types (Done when: serde derives compile)

### Tasks - RCT Tests (Round-Trip)

- [ ] Write round-trip test for `PubKey` (Done when: test_pubkey_roundtrip passes)
- [ ] Write round-trip test for `NodeId` (Done when: test_nodeid_roundtrip passes)
- [ ] Write round-trip test for `Signature` (Done when: test_signature_roundtrip passes)
- [ ] Write round-trip test for `Role` (Done when: test_role_roundtrip passes)
- [ ] Write round-trip test for `Status` (Done when: test_status_roundtrip passes)
- [ ] Write round-trip test for `PresenceState` (Done when: test_presence_state_roundtrip passes)
- [ ] Write round-trip test for `Destination` (Done when: test_destination_roundtrip passes)
- [ ] Write round-trip test for `MessageKind` (all variants) (Done when: test_message_kind_roundtrip passes)
- [ ] Write round-trip test for `Envelope` (Done when: test_envelope_roundtrip passes)
- [ ] Write round-trip test for `Roster` (Done when: test_roster_roundtrip passes)
- [ ] Write round-trip test for `ChannelPolicy` (Done when: test_channel_policy_roundtrip passes)
- [ ] Write round-trip test for `InboxItem` (Done when: test_inbox_item_roundtrip passes)

### Tasks - RCT Tests (Invariants)

- [ ] Write invariant test: Role enum encodes as string (Done when: test_role_encodes_as_string passes)
- [ ] Write invariant test: Status enum encodes as string (Done when: test_status_encodes_as_string passes)
- [ ] Write invariant test: PresenceState enum encodes as string (Done when: test_presence_state_encodes_as_string passes)
- [ ] Write invariant test: Envelope with expired TTL is detectable (Done when: test_envelope_ttl_expiry passes)
- [ ] Write invariant test: Roster version monotonicity check (Done when: test_roster_version_monotonic passes)
- [ ] Write invariant test: Request hop limit enforced (Done when: test_request_hop_limit passes)
- [ ] Write invariant test: canonical CBOR produces deterministic bytes (Done when: test_canonical_cbor_determinism passes)

### Phase 0 Gate Verification Commands

```bash
cargo test -p meerkat-comms
```

### Phase 0 Gate Review

Spawn reviewers IN PARALLEL:
- **RCT Guardian**: Verify representation round-trips for all types in model.rs and identity.rs
- **Spec Auditor**: Verify all fields match DESIGN-COMMS.md spec exactly

---

## Phase 1: Cryptographic Identity

**Goal:** Keypair generation, signing, and verification work correctly.

**Dependencies:** Phase 0 (all types compile and round-trip)

### Tasks - Keypair Operations (`identity.rs`)

- [ ] Implement `Keypair` struct (Done when: compiles with secret + public key)
- [ ] Implement `Keypair::generate()` (Done when: generates valid Ed25519 keypair)
- [ ] Implement `Keypair::sign(data: &[u8])` (Done when: returns Signature)
- [ ] Implement `PubKey::verify(data: &[u8], sig: &Signature)` (Done when: returns bool)
- [ ] Implement `PubKey::to_node_id()` (Done when: returns hash of pubkey)

### Tasks - Envelope Signing (`model.rs`)

- [ ] Implement `Envelope::signable_bytes()` using canonical CBOR (Done when: returns deterministic bytes for signing)
- [ ] Implement `Envelope::sign(keypair: &Keypair)` (Done when: sets sig field)
- [ ] Implement `Envelope::verify(&self)` (Done when: returns bool based on signature validity)

### Tasks - Integration Tests

- [ ] Write test: sign and verify envelope (Done when: test_envelope_sign_verify passes)
- [ ] Write test: tampered envelope fails verification (Done when: test_envelope_tamper_detection passes)
- [ ] Write test: wrong key fails verification (Done when: test_envelope_wrong_key passes)

### Phase 1 Gate Verification Commands

```bash
cargo test -p meerkat-comms
```

### Phase 1 Gate Review

Spawn reviewers IN PARALLEL:
- **RCT Guardian**: Verify signing/verification round-trips correctly
- **Integration Sheriff**: Verify sign → verify → tamper → fail flow works

---

## Phase 2: Roster Management

**Goal:** Roster parsing, validation, version comparison, and admin signature verification.

**Dependencies:** Phase 1 (signing/verification works)

### Tasks - Roster Module (`roster.rs`)

- [ ] Create `roster.rs` module (Done when: module compiles)
- [ ] Implement `Roster::is_member(pubkey: &PubKey) -> bool` (Done when: checks membership)
- [ ] Implement `Roster::get_role(pubkey: &PubKey) -> Option<Role>` (Done when: returns role if member)
- [ ] Implement `Roster::admins() -> Vec<&RosterMember>` (Done when: filters admin members)
- [ ] Implement `Roster::verify_signatures() -> bool` (Done when: verifies all signatures from admins)
- [ ] Implement `Roster::is_newer_than(other: &Roster) -> bool` (Done when: compares versions with monotonicity)
- [ ] Implement `Roster::can_accept_update(new: &Roster) -> Result<(), RosterError>` (Done when: validates version > current and signatures valid)

### Tasks - Roster Errors

- [ ] Implement `RosterError` enum (Done when: compiles with InvalidSignature, VersionNotNewer, NotAnAdmin variants)

### Tasks - Role Permissions

- [ ] Implement `Role::can_read() -> bool` (Done when: returns true for all roles)
- [ ] Implement `Role::can_write() -> bool` (Done when: returns true for member+)
- [ ] Implement `Role::can_send_requests() -> bool` (Done when: returns true for operator+)
- [ ] Implement `Role::can_manage_acls() -> bool` (Done when: returns true for admin only)

### Tasks - Integration Tests

- [ ] Write test: roster membership check (Done when: test_roster_membership passes)
- [ ] Write test: roster signature verification (Done when: test_roster_signature_verification passes)
- [ ] Write test: roster update with older version rejected (Done when: test_roster_version_rollback_rejected passes)
- [ ] Write test: role permission checks (Done when: test_role_permissions passes)

### Phase 2 Gate Verification Commands

```bash
cargo test -p meerkat-comms
```

### Phase 2 Gate Review

Spawn reviewers IN PARALLEL:
- **RCT Guardian**: Verify Roster round-trips and signature verification
- **Spec Auditor**: Verify role permissions match spec table exactly
- **Integration Sheriff**: Verify roster update flow rejects rollbacks

---

## Phase 3: Transport Layer (UDS)

**Goal:** Unix domain socket transport with handshake flow.

**Dependencies:** Phase 2 (roster validation works)

### Tasks - Transport Trait (`transport/mod.rs`)

- [ ] Create `transport/mod.rs` module (Done when: module compiles)
- [ ] Define `PeerTransport` trait (Done when: trait compiles with connect, accept, send, recv methods)
- [ ] Define `TransportError` enum (Done when: compiles with IoError, HandshakeFailed, Timeout variants)

### Tasks - UDS Transport (`transport/uds.rs`)

- [ ] Create `transport/uds.rs` module (Done when: module compiles)
- [ ] Implement `UdsTransport` struct (Done when: compiles with socket path)
- [ ] Implement `UdsTransport::bind(path)` for server (Done when: creates listening socket)
- [ ] Implement `UdsTransport::connect(path)` for client (Done when: connects to socket)
- [ ] Implement `UdsTransport::send(envelope)` (Done when: serializes and writes to socket)
- [ ] Implement `UdsTransport::recv() -> Envelope` (Done when: reads and deserializes from socket)

### Tasks - Handshake (`transport/mod.rs`)

- [ ] Implement `perform_handshake_client(transport, keypair, roster)` (Done when: sends Hello, receives HelloAck, verifies)
- [ ] Implement `perform_handshake_server(transport, keypair, roster)` (Done when: receives Hello, sends HelloAck, verifies)
- [ ] Implement roster version exchange during handshake (Done when: newer roster is shared)

### Tasks - Integration Tests

- [ ] Write test: UDS connect and send envelope (Done when: test_uds_send_recv passes)
- [ ] Write test: handshake succeeds between valid peers (Done when: test_handshake_success passes)
- [ ] Write test: handshake fails with unknown peer (Done when: test_handshake_unknown_peer_rejected passes)
- [ ] Write test: handshake exchanges roster update (Done when: test_handshake_roster_sync passes)

### Phase 3 Gate Verification Commands

```bash
cargo test -p meerkat-comms
```

### Phase 3 Gate Review

Spawn reviewers IN PARALLEL:
- **RCT Guardian**: Verify Envelope serialization over UDS round-trips
- **Integration Sheriff**: Verify handshake flow with roster validation
- **Spec Auditor**: Verify handshake matches spec flow diagram

---

## Phase 4: Inbox and Wake Logic

**Goal:** Inbox queue with priority handling and wake rules.

**Dependencies:** Phase 0 (InboxItem types)

### Tasks - Inbox Module (`inbox.rs`)

- [ ] Create `inbox.rs` module (Done when: module compiles)
- [ ] Implement `Inbox` struct (Done when: compiles with internal queue)
- [ ] Implement `Inbox::push(item: InboxItem)` (Done when: adds item to queue)
- [ ] Implement `Inbox::drain() -> Vec<InboxItem>` (Done when: returns and clears all items)
- [ ] Implement `Inbox::is_empty() -> bool` (Done when: checks if queue empty)
- [ ] Implement `Inbox::has_wake_items() -> bool` (Done when: checks for High/Normal priority)

### Tasks - Priority Assignment

- [ ] Implement `InboxItem::priority(&self) -> Priority` (Done when: returns priority per spec table)
- [ ] Implement priority for Request → High (Done when: test verifies)
- [ ] Implement priority for DM Message → High (Done when: test verifies)
- [ ] Implement priority for Channel Message → Normal (Done when: test verifies)
- [ ] Implement priority for SubagentResult → High (Done when: test verifies)
- [ ] Implement priority for Response → Normal (Done when: test verifies)

### Tasks - Mute Logic

- [ ] Implement `Inbox::mute_channel(channel_id)` (Done when: channel messages get Low priority)
- [ ] Implement `Inbox::unmute_channel(channel_id)` (Done when: channel messages restore Normal priority)

### Tasks - Integration Tests

- [ ] Write test: inbox push and drain (Done when: test_inbox_push_drain passes)
- [ ] Write test: wake items detection (Done when: test_inbox_wake_items passes)
- [ ] Write test: muted channel doesn't wake (Done when: test_inbox_muted_no_wake passes)
- [ ] Write test: priority assignment per spec (Done when: test_inbox_priority_assignment passes)

### Phase 4 Gate Verification Commands

```bash
cargo test -p meerkat-comms
```

### Phase 4 Gate Review

Spawn reviewers IN PARALLEL:
- **RCT Guardian**: Verify InboxItem round-trips
- **Spec Auditor**: Verify wake rules match spec table exactly
- **Integration Sheriff**: Verify mute/unmute affects wake behavior

---

## Phase 5: Message Router

**Goal:** PeerRouter for message dispatch with role/permission checks.

**Dependencies:** Phase 3 (transport), Phase 4 (inbox)

### Tasks - Router Module (`router.rs`)

- [ ] Create `router.rs` module (Done when: module compiles)
- [ ] Implement `PeerRouter` struct (Done when: compiles with peer map, roster, inbox)
- [ ] Implement `PeerRouter::add_peer(pubkey, transport)` (Done when: stores peer connection)
- [ ] Implement `PeerRouter::remove_peer(pubkey)` (Done when: removes peer connection)
- [ ] Implement `PeerRouter::send(envelope)` (Done when: routes to correct peer(s))

### Tasks - Message Reception

- [ ] Implement `PeerRouter::on_receive(envelope)` (Done when: validates and delivers to inbox)
- [ ] Implement signature verification on receive (Done when: invalid sigs rejected)
- [ ] Implement TTL check on receive (Done when: expired messages rejected)
- [ ] Implement roster membership check on receive (Done when: non-members rejected)
- [ ] Implement role permission check on receive (Done when: unauthorized actions rejected)
- [ ] Implement hop limit check for Requests (Done when: hop > 3 rejected)

### Tasks - Channel Routing

- [ ] Implement `PeerRouter::channel_members(channel_id) -> Vec<PubKey>` (Done when: returns members from ChannelPolicy)
- [ ] Implement fan-out for channel messages (Done when: sends to all channel members)

### Tasks - Integration Tests

- [ ] Write test: send message to peer (Done when: test_router_send_to_peer passes)
- [ ] Write test: send message to channel (Done when: test_router_send_to_channel passes)
- [ ] Write test: receive validates signature (Done when: test_router_rejects_invalid_sig passes)
- [ ] Write test: receive checks TTL (Done when: test_router_rejects_expired passes)
- [ ] Write test: receive checks roster membership (Done when: test_router_rejects_non_member passes)
- [ ] Write test: receive checks role for Request (Done when: test_router_rejects_member_request passes)
- [ ] Write test: receive checks hop limit (Done when: test_router_rejects_hop_exceeded passes)

### Phase 5 Gate Verification Commands

```bash
cargo test -p meerkat-comms
```

### Phase 5 Gate Review

Spawn reviewers IN PARALLEL:
- **RCT Guardian**: Verify message flow preserves envelope integrity
- **Spec Auditor**: Verify all receive validation steps match spec "On Receipt" section
- **Integration Sheriff**: Verify router → inbox delivery works end-to-end

---

## Phase 6: History Sync

**Goal:** HistoryRequest/Response flow with sequence tracking and dedup.

**Dependencies:** Phase 5 (router can send/receive)

### Tasks - Sequence Tracking

- [ ] Implement `SequenceTracker` struct (Done when: tracks (pubkey, epoch) → last_seq)
- [ ] Implement `SequenceTracker::update(pubkey, epoch, seq)` (Done when: updates tracking)
- [ ] Implement `SequenceTracker::has_gap(pubkey, epoch, seq) -> bool` (Done when: detects gaps)
- [ ] Implement epoch reset detection (Done when: new epoch resets seq tracking)

### Tasks - History Storage

- [ ] Implement `ChannelHistory` struct (Done when: stores messages with retention limits)
- [ ] Implement `ChannelHistory::add(envelope)` (Done when: stores envelope)
- [ ] Implement `ChannelHistory::query(since_ts, since_seq_by_sender, max_messages, max_bytes)` (Done when: returns matching messages)
- [ ] Implement retention pruning (Done when: old messages removed per policy)

### Tasks - History Protocol

- [ ] Implement `handle_history_request(request) -> HistoryResponse` (Done when: queries and responds)
- [ ] Implement `request_history(channel_id, peers)` (Done when: sends HistoryRequest to peers)
- [ ] Implement dedup on history receive (Done when: duplicate message IDs ignored)

### Tasks - Integration Tests

- [ ] Write test: sequence tracking detects gaps (Done when: test_sequence_gap_detection passes)
- [ ] Write test: epoch reset clears tracking (Done when: test_epoch_reset passes)
- [ ] Write test: history query respects limits (Done when: test_history_query_limits passes)
- [ ] Write test: history sync dedup (Done when: test_history_dedup passes)
- [ ] Write test: retention pruning (Done when: test_retention_pruning passes)

### Phase 6 Gate Verification Commands

```bash
cargo test -p meerkat-comms
```

### Phase 6 Gate Review

Spawn reviewers IN PARALLEL:
- **RCT Guardian**: Verify HistoryRequest/Response round-trips
- **Spec Auditor**: Verify sync algorithm matches spec
- **Integration Sheriff**: Verify full sync flow with multiple peers

---

## Phase 7: MCP Tools

**Goal:** MCP server exposing comms tools for agent use.

**Dependencies:** Phase 5 (router), Phase 4 (inbox)

### Tasks - Crate Setup

- [ ] Create `meerkat-comms-mcp/Cargo.toml` (Done when: `cargo check -p meerkat-comms-mcp` succeeds)
- [ ] Create `meerkat-comms-mcp/src/lib.rs` (Done when: module compiles)

### Tasks - Tool Implementations (`tools.rs`)

- [ ] Implement `send_message(target, body)` tool (Done when: sends Message via router)
- [ ] Implement `send_request(target, intent, params)` tool (Done when: sends Request via router)
- [ ] Implement `send_response(request_id, status, result)` tool (Done when: sends Response via router)
- [ ] Implement `set_presence(state)` tool (Done when: broadcasts Presence)
- [ ] Implement `subscribe(channel)` tool (Done when: adds channel subscription)
- [ ] Implement `unsubscribe(channel)` tool (Done when: removes channel subscription)
- [ ] Implement `mute(channel)` tool (Done when: mutes channel in inbox)
- [ ] Implement `unmute(channel)` tool (Done when: unmutes channel in inbox)

### Tasks - MCP Server

- [ ] Implement MCP tool registration (Done when: tools discoverable via MCP)
- [ ] Implement tool parameter validation (Done when: invalid params rejected)

### Tasks - Integration Tests

- [ ] Write test: send_message via MCP (Done when: test_mcp_send_message passes)
- [ ] Write test: send_request via MCP (Done when: test_mcp_send_request passes)
- [ ] Write test: subscribe/unsubscribe via MCP (Done when: test_mcp_subscription passes)

### Phase 7 Gate Verification Commands

```bash
cargo test -p meerkat-comms-mcp
```

### Phase 7 Gate Review

Spawn reviewers IN PARALLEL:
- **Spec Auditor**: Verify all tools from spec are implemented
- **Integration Sheriff**: Verify MCP → router → inbox flow

---

## Final Phase: E2E Scenarios and Integration

**Goal:** Full system working with E2E scenarios from spec.

**Dependencies:** All prior phases complete

### Tasks - E2E Scenario Tests

- [ ] Implement S1: Bug report between agents (Done when: test_e2e_bug_report passes per spec example)
- [ ] Implement S2: Channel broadcast (Done when: test_e2e_channel_broadcast passes per spec example)
- [ ] Implement S3: Inbox flow with subagent (Done when: test_e2e_inbox_subagent passes per spec example)
- [ ] Implement S4: Roster update propagation (Done when: test_e2e_roster_update passes)
- [ ] Implement S5: History sync after reconnect (Done when: test_e2e_history_sync passes)

### Tasks - meerkat-core Integration

- [ ] Integrate Inbox with agent loop (Done when: agent receives inbox items)
- [ ] Integrate SubagentResult delivery (Done when: subagent completion → parent inbox)
- [ ] Implement presence auto-update (Done when: presence reflects loop state)

### Tasks - CLI Commands

- [ ] Implement `rkat comms status` (Done when: shows presence and peer count)
- [ ] Implement `rkat comms peers` (Done when: lists connected peers)
- [ ] Implement `rkat comms send` (Done when: sends message from CLI)

### Tasks - Final Verification

- [ ] Run full test suite (Done when: all tests pass)
- [ ] Verify no `todo!()` or `unimplemented!()` in completed code (Done when: grep returns empty)
- [ ] Verify all spec sections have corresponding tests (Done when: coverage audit passes)

### Final Phase Gate Verification Commands

```bash
cargo test -p meerkat-comms
cargo test -p meerkat-comms-mcp
cargo test --workspace
```

### Final Phase Gate Review

Spawn reviewers IN PARALLEL:
- **RCT Guardian**: Final representation verification (scope: all types)
- **Integration Sheriff**: Verify all choke points covered (scope: all phases)
- **Spec Auditor**: Verify 100% spec coverage (scope: full DESIGN-COMMS.md)
- **Methodology Integrity Gate**: Check for process-over-product, accumulated stubs, deferrals

---

## Success Criteria

1. [ ] All existing meerkat tests still pass
2. [ ] Phase 0 RCT tests green (all round-trips pass)
3. [ ] All E2E scenario tests pass
4. [ ] All integration tests pass
5. [ ] No `todo!()`, `unimplemented!()`, or `NotImplementedError` in completed code
6. [ ] 100% spec coverage verified
7. [ ] Single-machine UDS communication works end-to-end

---

## Appendix: Reviewer Agent Prompts

See `.claude/skills/rct-methodology/references/reviewer_prompts.md` for complete prompts.

**Phase-to-Spec Mapping for Spec Auditor:**

| Phase | Spec Sections |
|-------|---------------|
| Phase 0 | Core Concepts (all types) |
| Phase 1 | Identity, Messages (signing) |
| Phase 2 | Roster, Roles |
| Phase 3 | Network Topology, Handshake Flow, Transport |
| Phase 4 | Agent Integration (Inbox, Wake Rules) |
| Phase 5 | Message Flow (On Receipt, Routing) |
| Phase 6 | History Sync |
| Phase 7 | Comms Tools (via MCP) |
| Final | Example Scenarios, all sections |
