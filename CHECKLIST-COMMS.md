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

> **Note:** These are **methodology gates**, separate from the Phase Gates below. Phase Gates are verification checkpoints at the end of each implementation phase.

---

## Gate Naming Clarification

- **RCT Gates** refer to methodology stages (RCT Gate 0–3) and do NOT align 1:1 with implementation phases.
- **Phase Gates** are the verification checkpoints at the end of each phase (Phase 0 Gate, Phase 1 Gate, etc.).
- When this checklist says "Phase X Gate Verification," it means the checkpoint for that implementation phase.

---

## Task Format (Agent-Ready)

Each task is:
- **Atomic**: One deliverable per task
- **Observable**: Clear "done when" condition with test name or verifiable outcome
- **File-specific**: Explicit output location when applicable

Format: `- [ ] <action> (Done when: <observable condition>)`

---

## Global Dependencies

- `meerkat-comms` crate must exist before any phase tasks
- Ed25519 library (`ed25519-dalek`) required for Phase 1+
- CBOR library (`ciborium`) required for Phase 0+
- Async runtime (`tokio`) required for Phase 3+
- All Phase 0 types must have passing round-trip tests before Phase 1 can begin

---

## Phase Completion Protocol

When completing a phase:

1. [ ] All phase checklist items marked complete
2. [ ] Run phase gate verification commands
3. [ ] Spawn reviewer agents IN PARALLEL (single message, multiple Task calls)
4. [ ] Collect all verdicts
5. [ ] If any BLOCK verdicts:
   - Address blocking issues
   - Re-run verification commands
   - Re-run *all* reviewers
6. [ ] Record phase completion with reviewer verdicts
7. [ ] Proceed to next phase

**Critical:** Reviewers are independent agents with access to tools (Grep, Read, Bash). They discover state themselves. Do NOT feed them test results or grep outputs.

---

## Phase 0: Representation Contracts (RCT Gate 0 - MUST BE GREEN)
PHASE_0_APPROVED

**Goal:** All types compile, serialize, and round-trip through CBOR/JSON. No behavior yet - only data shapes.

**Dependencies:** None (this is the foundation)

### Tasks - Crate Setup

- [x] Create `meerkat-comms/Cargo.toml` with dependencies (Done when: `cargo check -p meerkat-comms` exits 0)
- [x] Create `meerkat-comms/src/lib.rs` as empty module file (Done when: file exists with `// meerkat-comms` comment)
- [x] Add `mod types;` declaration to lib.rs (Done when: `cargo check -p meerkat-comms` exits 0)
- [x] Add `mod identity;` declaration to lib.rs (Done when: `cargo check -p meerkat-comms` exits 0)
- [x] Add `mod trust;` declaration to lib.rs (Done when: `cargo check -p meerkat-comms` exits 0)
- [x] Create `meerkat-comms/src/types.rs` (Done when: file exists)
- [x] Create `meerkat-comms/src/identity.rs` (Done when: file exists)
- [x] Create `meerkat-comms/src/trust.rs` (Done when: file exists)

### Tasks - Identity Types (`identity.rs`)

- [x] Define `PubKey` newtype wrapping `[u8; 32]` (Done when: `test_pubkey_size` passes asserting size_of::<PubKey>() == 32)
- [x] Define `Signature` newtype wrapping `[u8; 64]` (Done when: `test_signature_size` passes asserting size_of::<Signature>() == 64)
- [x] Derive `Serialize`/`Deserialize` for `PubKey` (Done when: `test_pubkey_cbor_roundtrip` passes)
- [x] Derive `Serialize`/`Deserialize` for `Signature` (Done when: `test_signature_cbor_roundtrip` passes)

### Tasks - Core Types (`types.rs`)

- [x] Define `Status` enum with `Accepted`, `Completed`, `Failed` variants (Done when: `test_status_variants` passes checking all 3 variants exist)
- [x] Define `MessageKind::Message` variant with `body: String` (Done when: `test_message_kind_message_fields` passes)
- [x] Define `MessageKind::Request` variant with `intent: String`, `params: JsonValue` (Done when: `test_message_kind_request_fields` passes)
- [x] Define `MessageKind::Response` variant with `in_reply_to: Uuid`, `status: Status`, `result: JsonValue` (Done when: `test_message_kind_response_fields` passes)
- [x] Define `MessageKind::Ack` variant with `in_reply_to: Uuid` (Done when: `test_message_kind_ack_fields` passes)
- [x] Define `Envelope` struct with `id`, `from`, `to`, `kind`, `sig` fields (Done when: `test_envelope_fields` passes)
- [x] Derive `Serialize`/`Deserialize` for `Status` (Done when: `test_status_cbor_roundtrip` passes)
- [x] Derive `Serialize`/`Deserialize` for `MessageKind` (Done when: `test_message_kind_cbor_roundtrip` passes)
- [x] Derive `Serialize`/`Deserialize` for `Envelope` (Done when: `test_envelope_cbor_roundtrip` passes)

### Tasks - Trust Types (`trust.rs`)

- [x] Define `TrustedPeer` struct with `name: String`, `pubkey: PubKey`, `addr: String` (Done when: `test_trusted_peer_fields` passes)
- [x] Define `TrustedPeers` struct with `peers: Vec<TrustedPeer>` (Done when: `test_trusted_peers_fields` passes)
- [x] Derive `Serialize`/`Deserialize` for `TrustedPeer` (Done when: `test_trusted_peer_json_roundtrip` passes)
- [x] Derive `Serialize`/`Deserialize` for `TrustedPeers` (Done when: `test_trusted_peers_json_roundtrip` passes)

### Tasks - Inbox Types (`types.rs`)

- [x] Define `InboxItem::External` variant with `envelope: Envelope` (Done when: `test_inbox_item_external_fields` passes)
- [x] Define `InboxItem::SubagentResult` variant with `subagent_id: Uuid`, `result: JsonValue`, `summary: String` (Done when: `test_inbox_item_subagent_fields` passes)
- [x] Derive `Serialize`/`Deserialize` for `InboxItem` (Done when: `test_inbox_item_cbor_roundtrip` passes)

### Tasks - RCT Invariant Tests

- [x] Write test: `Status` encodes as string, not ordinal (Done when: `test_status_encodes_as_string` passes verifying CBOR contains "Accepted" not 0)
- [x] Write test: `MessageKind` variant tags are strings (Done when: `test_message_kind_tags_are_strings` passes)

### Phase 0 Gate Verification Commands

```bash
cargo test -p meerkat-comms
```

Note: Stub detection is performed by reviewer agents independently.

### Phase 0 Gate Review

Spawn reviewers IN PARALLEL:
- **RCT Guardian**: Scope = all types in `types.rs`, `identity.rs`, `trust.rs`. Verify CBOR/JSON round-trips.
- **Spec Auditor**: Scope = DESIGN-COMMS.md "Messages" and "Trust" sections. Verify all fields present.

---

## Phase 1: Cryptographic Identity
PHASE_1_APPROVED

**Goal:** Key generation, signing, verification, and PeerId conversion work correctly.

**Dependencies:** Phase 0 (all types have passing round-trip tests)

### Tasks - PeerId Conversion (`identity.rs`)

- [x] Implement `PubKey::to_peer_id() -> String` returning "ed25519:" + base64 (Done when: `test_pubkey_to_peer_id` passes)
- [x] Implement `PubKey::from_peer_id(s: &str) -> Result<PubKey>` parsing "ed25519:..." (Done when: `test_pubkey_from_peer_id` passes)
- [x] Write test: peer_id format matches "ed25519:<base64>" pattern (Done when: `test_peer_id_format` passes with regex validation)
- [x] Write test: peer_id round-trips correctly (Done when: `test_peer_id_roundtrip` passes: to_peer_id → from_peer_id → equals original)

### Tasks - Keypair (`identity.rs`)

- [x] Define `Keypair` struct holding secret and public key (Done when: `test_keypair_struct` passes)
- [x] Implement `Keypair::generate()` (Done when: `test_keypair_generate` passes verifying pubkey is 32 bytes)
- [x] Implement `Keypair::public_key() -> PubKey` (Done when: `test_keypair_public_key` passes)
- [x] Implement `Keypair::sign(data: &[u8]) -> Signature` (Done when: `test_keypair_sign` passes verifying signature is 64 bytes)
- [x] Implement `PubKey::verify(data: &[u8], sig: &Signature) -> bool` (Done when: `test_pubkey_verify` passes)

### Tasks - Envelope Signing (`types.rs`)

- [x] Implement `Envelope::signable_bytes()` returning canonical CBOR of `[id, from, to, kind]` (Done when: `test_signable_bytes_deterministic` passes verifying same envelope produces identical bytes)
- [x] Implement `Envelope::sign(&mut self, keypair: &Keypair)` (Done when: `test_envelope_sign` passes verifying sig field is set)
- [x] Implement `Envelope::verify(&self) -> bool` (Done when: `test_envelope_verify` passes)

### Tasks - Key Persistence (`identity.rs`)

- [x] Implement `Keypair::save(dir: &Path)` writing `identity.key` and `identity.pub` (Done when: `test_keypair_save` passes verifying files exist)
- [x] Implement `Keypair::load(dir: &Path)` reading key files (Done when: `test_keypair_load` passes)
- [x] Implement `Keypair::load_or_generate(dir: &Path)` loading existing keys (Done when: `test_keypair_load_or_generate_existing` passes)
- [x] Implement `Keypair::load_or_generate(dir: &Path)` generating new keys (Done when: `test_keypair_load_or_generate_new` passes)

### Tasks - Security Tests

- [x] Write test: sign then verify succeeds (Done when: `test_sign_verify_roundtrip` passes)
- [x] Write test: tampered data fails verification (Done when: `test_tamper_detection` passes)
- [x] Write test: wrong key fails verification (Done when: `test_wrong_key_rejection` passes)
- [x] Write test: keypair persistence round-trips (Done when: `test_keypair_persistence_roundtrip` passes: save → load → sign → verify)

### Phase 1 Gate Verification Commands

```bash
cargo test -p meerkat-comms
```

### Phase 1 Gate Review

Spawn reviewers IN PARALLEL:
- **RCT Guardian**: Scope = signing/verification in `identity.rs`. Verify `signable_bytes` determinism.
- **Integration Sheriff**: Scope = sign → verify → tamper flows. Verify tamper detection works.

---

## Phase 2: Trust Management
PHASE_2_APPROVED

**Goal:** Load, save, and query trusted peers list.

**Dependencies:** Phase 1 (PubKey operations work)

### Tasks - Trust Operations (`trust.rs`)

- [x] Implement `TrustedPeers::load(path: &Path) -> Result<Self>` (Done when: `test_trusted_peers_load` passes with sample JSON)
- [x] Implement `TrustedPeers::save(&self, path: &Path) -> Result<()>` (Done when: `test_trusted_peers_save` passes)
- [x] Implement `TrustedPeers::is_trusted(&self, pubkey: &PubKey) -> bool` for trusted peer (Done when: `test_is_trusted_found` passes)
- [x] Implement `TrustedPeers::is_trusted(&self, pubkey: &PubKey) -> bool` for unknown peer (Done when: `test_is_trusted_not_found` passes)
- [x] Implement `TrustedPeers::get_peer(&self, pubkey: &PubKey) -> Option<&TrustedPeer>` (Done when: `test_get_peer` passes)
- [x] Implement `TrustedPeers::get_by_name(&self, name: &str) -> Option<&TrustedPeer>` (Done when: `test_get_by_name` passes)

### Tasks - JSON Format Tests

- [x] Write test: JSON format matches spec example (Done when: `test_json_format_matches_spec` passes verifying structure matches DESIGN-COMMS.md)
- [x] Write test: load/save round-trip preserves data (Done when: `test_trusted_peers_persistence_roundtrip` passes)

### Phase 2 Gate Verification Commands

```bash
cargo test -p meerkat-comms
```

### Phase 2 Gate Review

Spawn reviewers IN PARALLEL:
- **Spec Auditor**: Scope = DESIGN-COMMS.md "Trust" section. Verify JSON format matches spec exactly.

---

## Phase 3: Transport Layer
PHASE_3_APPROVED

**Goal:** UDS and TCP transports with length-prefix framing can send/receive envelopes.

**Dependencies:** Phase 1 (Envelope signing for test envelopes)

> **Note:** Transport tests use properly signed test envelopes to ensure realistic testing.

### Tasks - Framing (`transport/mod.rs`)

- [x] Create `meerkat-comms/src/transport/mod.rs` (Done when: file exists)
- [x] Add `mod transport;` to lib.rs (Done when: `cargo check -p meerkat-comms` exits 0)
- [x] Define `TransportError` enum with `Io` variant (Done when: `test_transport_error_io` passes)
- [x] Define `TransportError` enum with `Timeout` variant (Done when: `test_transport_error_timeout` passes)
- [x] Define `TransportError` enum with `MessageTooLarge` variant (Done when: `test_transport_error_too_large` passes)
- [x] Define `TransportError` enum with `InvalidFrame` variant (Done when: `test_transport_error_invalid_frame` passes)
- [x] Implement `write_envelope<W: Write>(writer: &mut W, envelope: &Envelope)` with 4-byte length prefix (Done when: `test_write_envelope_format` passes verifying first 4 bytes are big-endian length)
- [x] Implement `read_envelope<R: Read>(reader: &mut R) -> Result<Envelope>` (Done when: `test_read_envelope` passes)
- [x] Implement max payload check (1 MB) in `read_envelope` (Done when: `test_reject_oversized_payload` passes)

### Tasks - Address Parsing (`transport/mod.rs`)

- [x] Define `PeerAddr` enum with `Uds(PathBuf)` variant (Done when: `test_peer_addr_uds_variant` passes)
- [x] Define `PeerAddr` enum with `Tcp(SocketAddr)` variant (Done when: `test_peer_addr_tcp_variant` passes)
- [x] Implement `PeerAddr::parse(s: &str)` for "uds://..." (Done when: `test_parse_uds_addr` passes)
- [x] Implement `PeerAddr::parse(s: &str)` for "tcp://..." (Done when: `test_parse_tcp_addr` passes)
- [x] Write test: invalid address format rejected (Done when: `test_parse_invalid_addr` passes)

### Tasks - UDS Transport (`transport/uds.rs`)

- [x] Create `meerkat-comms/src/transport/uds.rs` (Done when: file exists)
- [x] Implement async UDS listener bind (Done when: `test_uds_bind` passes)
- [x] Implement async UDS connect (Done when: `test_uds_connect` passes)
- [x] Implement send/recv envelope over UDS (Done when: `test_uds_envelope_roundtrip` passes)

### Tasks - TCP Transport (`transport/tcp.rs`)

- [x] Create `meerkat-comms/src/transport/tcp.rs` (Done when: file exists)
- [x] Implement async TCP listener bind (Done when: `test_tcp_bind` passes)
- [x] Implement async TCP connect (Done when: `test_tcp_connect` passes)
- [x] Implement send/recv envelope over TCP (Done when: `test_tcp_envelope_roundtrip` passes)

### Phase 3 Gate Verification Commands

```bash
cargo test -p meerkat-comms
```

### Phase 3 Gate Review

Spawn reviewers IN PARALLEL:
- **RCT Guardian**: Scope = framing in `transport/mod.rs`. Verify envelope survives write → read round-trip.
- **Integration Sheriff**: Scope = UDS and TCP transports. Verify both can send/receive envelopes.
- **Spec Auditor**: Scope = DESIGN-COMMS.md "Transport" section. Verify framing format (4-byte length prefix, 1MB max).

---

## Phase 4: IO Task and Inbox
PHASE_4_APPROVED

**Goal:** Thread-safe inbox and IO task that validates messages and sends acks.

**Dependencies:** Phase 1 (signing), Phase 2 (trust), Phase 3 (transport)

### Tasks - Inbox (`inbox.rs`)

- [x] Create `meerkat-comms/src/inbox.rs` (Done when: file exists)
- [x] Add `mod inbox;` to lib.rs (Done when: `cargo check -p meerkat-comms` exits 0)
- [x] Define `Inbox` struct wrapping `mpsc::UnboundedReceiver<InboxItem>` (Done when: `test_inbox_struct` passes)
- [x] Define `InboxSender` struct wrapping `mpsc::UnboundedSender<InboxItem>` (Done when: `test_inbox_sender_struct` passes)
- [x] Implement `Inbox::new() -> (Inbox, InboxSender)` (Done when: `test_inbox_new` passes)
- [x] Implement `InboxSender::send(item: InboxItem)` (Done when: `test_inbox_sender_send` passes)
- [x] Implement `Inbox::recv() -> Option<InboxItem>` async (Done when: `test_inbox_recv` passes)
- [x] Implement `Inbox::try_drain() -> Vec<InboxItem>` (Done when: `test_inbox_try_drain` passes draining multiple items)

### Tasks - IO Task (`io_task.rs`)

- [x] Create `meerkat-comms/src/io_task.rs` (Done when: file exists)
- [x] Add `mod io_task;` to lib.rs (Done when: `cargo check -p meerkat-comms` exits 0)
- [x] Implement `handle_connection(stream, keypair, trusted, inbox_sender)` async fn signature (Done when: `test_handle_connection_compiles` passes)
- [x] Implement envelope reading with framing in IO task (Done when: `test_io_task_reads_envelope` passes)
- [x] Implement signature verification in IO task (Done when: `test_io_task_verifies_signature` passes)
- [x] Implement trust check in IO task (Done when: `test_io_task_checks_trust` passes)
- [x] Implement ack creation and sending in IO task (Done when: `test_io_task_sends_ack` passes)
- [x] Implement inbox enqueue in IO task (Done when: `test_io_task_enqueues_to_inbox` passes)

### Tasks - Ack Rules

- [x] Implement: send Ack for `MessageKind::Message` (Done when: `test_ack_for_message` passes)
- [x] Implement: send Ack for `MessageKind::Request` (Done when: `test_ack_for_request` passes)
- [x] Implement: do NOT send Ack for `MessageKind::Ack` (Done when: `test_no_ack_for_ack` passes)
- [x] Implement: do NOT send Ack for `MessageKind::Response` (Done when: `test_no_ack_for_response` passes)

### Tasks - Rejection Behavior

- [x] Implement: drop message silently on invalid signature (Done when: `test_drop_invalid_signature` passes verifying no ack sent, no inbox item)
- [x] Implement: drop message silently on untrusted sender (Done when: `test_drop_untrusted_sender` passes verifying no ack sent, no inbox item)

### Phase 4 Gate Verification Commands

```bash
cargo test -p meerkat-comms
```

### Phase 4 Gate Review

Spawn reviewers IN PARALLEL:
- **Integration Sheriff**: Scope = IO task flow in `io_task.rs`. Verify read → validate → ack → inbox flow.
- **Spec Auditor**: Scope = DESIGN-COMMS.md "Ack Rules" table. Verify ack behavior matches exactly.

---

## Phase 5: Router (Send API)
PHASE_5_APPROVED

**Goal:** High-level send API with ack timeout handling.

**Dependencies:** Phase 4 (IO task and inbox work)

### Tasks - Router (`router.rs`)

- [x] Create `meerkat-comms/src/router.rs` (Done when: file exists)
- [x] Add `mod router;` to lib.rs (Done when: `cargo check -p meerkat-comms` exits 0)
- [x] Define `Router` struct holding keypair, trusted_peers, config (Done when: `test_router_struct` passes)
- [x] Define `SendError::PeerNotFound` variant (Done when: `test_send_error_peer_not_found` passes)
- [x] Define `SendError::PeerOffline` variant (Done when: `test_send_error_peer_offline` passes)
- [x] Define `SendError::Io` variant (Done when: `test_send_error_io` passes)
- [x] Define `CommsConfig` struct with `ack_timeout_secs`, `max_message_bytes` (Done when: `test_comms_config` passes)

### Tasks - Send Implementation

- [x] Implement `Router::send(peer_name: &str, kind: MessageKind) -> Result<(), SendError>` (Done when: `test_router_send` passes)
- [x] Implement peer name lookup via `TrustedPeers::get_by_name` (Done when: `test_router_resolves_peer_name` passes)
- [x] Implement connection to peer address (Done when: `test_router_connects_to_peer` passes)
- [x] Implement envelope creation and signing (Done when: `test_router_signs_envelope` passes)
- [x] Implement ack wait with timeout (Done when: `test_router_waits_for_ack` passes)
- [x] Implement timeout returns `SendError::PeerOffline` (Done when: `test_router_timeout_returns_offline` passes)

### Tasks - Send Helpers

- [x] Implement `Router::send_message(peer: &str, body: String)` (Done when: `test_send_message` passes)
- [x] Implement `Router::send_request(peer: &str, intent: String, params: JsonValue)` (Done when: `test_send_request` passes)
- [x] Implement `Router::send_response(peer: &str, in_reply_to: Uuid, status: Status, result: JsonValue)` (Done when: `test_send_response` passes)

### Tasks - Response Special Case

- [x] Implement: `send_response` does NOT wait for ack (Done when: `test_send_response_no_ack_wait` passes verifying immediate return)

### Phase 5 Gate Verification Commands

```bash
cargo test -p meerkat-comms
```

### Phase 5 Gate Review

Spawn reviewers IN PARALLEL:
- **Integration Sheriff**: Scope = send flow in `router.rs`. Verify send → ack → timeout flow.
- **Spec Auditor**: Scope = DESIGN-COMMS.md "On Send" section. Verify all steps implemented.

---

## Phase 6: MCP Tools
PHASE_6_APPROVED

**Goal:** Expose comms functionality as MCP tools.

**Dependencies:** Phase 5 (Router works)

### Tasks - Crate Setup

- [x] Create `meerkat-comms-mcp/Cargo.toml` (Done when: `cargo check -p meerkat-comms-mcp` exits 0)
- [x] Create `meerkat-comms-mcp/src/lib.rs` (Done when: file exists)
- [x] Create `meerkat-comms-mcp/src/tools.rs` (Done when: file exists)

### Tasks - Tool Implementations (`tools.rs`)

- [x] Implement `send_message` tool calling `Router::send_message` (Done when: `test_tool_send_message` passes)
- [x] Implement `send_request` tool calling `Router::send_request` (Done when: `test_tool_send_request` passes)
- [x] Implement `send_response` tool calling `Router::send_response` (Done when: `test_tool_send_response` passes)
- [x] Implement `list_peers` tool returning trusted peers (Done when: `test_tool_list_peers` passes)

### Tasks - MCP Registration

- [x] Register all tools with MCP server (Done when: `test_mcp_tool_discovery` passes listing all 4 tools)

### Phase 6 Gate Verification Commands

```bash
cargo test -p meerkat-comms-mcp
```

### Phase 6 Gate Review

Spawn reviewers IN PARALLEL:
- **Spec Auditor**: Scope = DESIGN-COMMS.md "MCP Tools" table. Verify all 4 tools implemented.

---

## Final Phase: E2E Scenarios
FINAL_PHASE_APPROVED

**Goal:** Full system working end-to-end with realistic scenarios.

**Dependencies:** All prior phases complete

### Tasks - E2E Test Infrastructure

- [x] Create E2E test harness spawning two Meerkat instances (Done when: `test_e2e_harness` passes)
- [x] Implement helper to set up mutual trust between two instances (Done when: `test_e2e_mutual_trust_setup` passes)

### Tasks - E2E Scenarios

- [x] E2E: Two peers exchange Message over UDS (Done when: `test_e2e_uds_message_exchange` passes)
- [x] E2E: Two peers exchange Message over TCP (Done when: `test_e2e_tcp_message_exchange` passes)
- [x] E2E: Full Request → Ack → Response flow (Done when: `test_e2e_request_response_flow` passes)
- [x] E2E: Untrusted peer connection rejected (Done when: `test_e2e_untrusted_rejected` passes)
- [x] E2E: Concurrent messages from 3 peers handled correctly (Done when: `test_e2e_concurrent_multi_peer` passes)

### Tasks - Final Verification

- [x] Run full test suite (Done when: `cargo test --workspace` exits 0)
- [x] All reviewers approve (Done when: RCT Guardian, Integration Sheriff, Spec Auditor, and Methodology Integrity all return APPROVE)

### Final Phase Gate Verification Commands

```bash
cargo test -p meerkat-comms
cargo test -p meerkat-comms-mcp
cargo test --workspace
cargo clippy -p meerkat-comms -p meerkat-comms-mcp -- -D warnings
```

### Final Phase Gate Review

Spawn reviewers IN PARALLEL:
- **RCT Guardian**: Scope = all types. Final verification of representation round-trips.
- **Integration Sheriff**: Scope = all phases. Verify all integration choke-points covered.
- **Spec Auditor**: Scope = full DESIGN-COMMS.md. Verify 100% coverage.
- **Methodology Integrity**: Check for stubs, XFAIL markers, deferred requirements.

---

## Success Criteria

1. [x] All existing meerkat workspace tests still pass
2. [x] Phase 0 RCT tests green (all round-trips pass)
3. [x] All E2E scenario tests pass
4. [x] No `todo!()`, `unimplemented!()`, or stub code in `meerkat-comms*/src/`
5. [x] Two Meerkats can exchange messages over UDS
6. [x] Two Meerkats can exchange messages over TCP
7. [x] Concurrent messages from multiple peers handled correctly
8. [x] 100% spec coverage verified by Spec Auditor
9. [x] `cargo clippy -p meerkat-comms -p meerkat-comms-mcp -- -D warnings` passes with no warnings

---

## Phase-to-Spec Mapping (for Spec Auditor)

| Phase | DESIGN-COMMS.md Sections |
|-------|--------------------------|
| Phase 0 | "Messages" (all type definitions), "Trust" (TrustedPeers struct) |
| Phase 1 | "Identity" (PubKey, PeerId format, keypair), "Signature" (canonical CBOR, field order) |
| Phase 2 | "Trust" (rules, JSON format, file location) |
| Phase 3 | "Transport" (framing, UDS, TCP, addresses), "Connection Lifecycle" |
| Phase 4 | "Concurrency Model" (IO task, inbox), "Ack Rules" (table), "On Receive" |
| Phase 5 | "On Send", "Liveness" |
| Phase 6 | "MCP Tools" (all 4 tools) |
| Final | "Example Flow", all sections for completeness |

---

## Appendix: Reviewer Agent Prompts

### RCT Guardian Prompt

```
You are the RCT Guardian reviewing Phase {PHASE} of the meerkat-comms implementation.

BE BRUTALLY HONEST. Your job is to find problems, not rubber-stamp work.

You have access to tools: Grep, Read, Bash. Use them to discover state independently.

Your scope is LIMITED to representation boundaries:
- Serialization/encoding strategy (CBOR, JSON)
- Round-trip correctness (serialize → deserialize → equals)
- Enum encoding stability (strings, not ordinals)
- Canonical CBOR determinism for signatures

## Phase-Specific Scope

{PHASE_SCOPE}

## Required Verification Steps

Perform these checks yourself using your tools:

1. **Run tests**: Use Bash to run `cargo test -p meerkat-comms`
2. **Stub detection**: Use Grep to search for `todo!` and `unimplemented!` in meerkat-comms*/src/
3. **Read test files**: Use Read to examine any failing tests
4. **Check round-trip tests**: Verify they exist and pass for types in scope

## Blocking Rules

You MUST block if:
- A round-trip test is failing (evidence_type: TEST_FAILING)
- Enum encodes as ordinal instead of string (evidence_type: SPEC_VIOLATION)
- signable_bytes produces non-deterministic output (evidence_type: TEST_FAILING)
- A representation boundary changed without test coverage (evidence_type: TEST_MISSING)
- Stubs (todo!, unimplemented!) found in code marked complete (evidence_type: STUB_MASKING)

You MUST NOT block for:
- Issues outside representation scope
- Behavior not yet implemented in this phase

## Output Format

verdict: APPROVE | BLOCK
gate: RCT_GUARDIAN
blocking:
  - id: RCT-001
    claim: "<specific claim>"
    evidence_type: TEST_FAILING | TEST_MISSING | SPEC_VIOLATION | STUB_MASKING
    evidence: "<test name or code location you found>"
    fix: "<actionable fix>"
non_blocking:
  - id: NB-001
    note: "<suggestion>"
```

### Integration Sheriff Prompt

```
You are the Integration Sheriff reviewing Phase {PHASE} of the meerkat-comms implementation.

BE BRUTALLY HONEST. Your job is to find wiring problems before they reach production.

You have access to tools: Grep, Read, Bash. Use them to discover state independently.

Your scope is LIMITED to cross-component wiring:
- Data flows correctly between components
- IO task → inbox → agent flow
- Send → connect → write → read ack flow
- Test failures at ASSERTION level, not IMPORT level

## Phase-Specific Scope

{PHASE_SCOPE}

## "Red OK" Rules

- Phases 0-3: E2E tests expected RED. Only block if wrong error type.
- Phases 4-5: Integration tests may be RED but must be executable.
- Final Phase: ALL tests must be GREEN.

## Required Verification Steps

Perform these checks yourself using your tools:

1. **Run tests**: Use Bash to run `cargo test -p meerkat-comms`
2. **XFAIL/ignore detection**: Use Grep to search for `#[ignore]` in meerkat-comms*/src/ and tests/
   - ONLY acceptable if comment references external bug number
   - Block if on feature tests without bug reference
3. **Stub detection**: Use Grep to search for `todo!` and `unimplemented!` in meerkat-comms*/src/
4. **Read integration tests**: Use Read to verify choke points have coverage

## Blocking Rules

You MUST block if:
- A test fails with wrong error type (evidence_type: TEST_FAILING)
- A choke point has no integration test (evidence_type: TEST_MISSING)
- Data doesn't flow correctly between components (evidence_type: WIRING_VIOLATION)
- #[ignore] markers on feature tests without external bug reference (evidence_type: XFAIL_ABUSE)
- Stubs (todo!, unimplemented!) in completed integration code (evidence_type: STUB_MASKING)

## Output Format

verdict: APPROVE | BLOCK
gate: INTEGRATION_SHERIFF
blocking:
  - id: INT-001
    claim: "<specific claim>"
    evidence_type: TEST_FAILING | TEST_MISSING | WIRING_VIOLATION | XFAIL_ABUSE | STUB_MASKING
    evidence: "<test name and error type you found>"
    fix: "<actionable fix>"
non_blocking:
  - id: NB-001
    note: "<suggestion>"
```

### Spec Auditor Prompt

```
You are the Spec Auditor reviewing Phase {PHASE} of the meerkat-comms implementation.

BE BRUTALLY HONEST. Your job is to ensure implementation matches DESIGN-COMMS.md.

You have access to tools: Grep, Read, Bash. Use them to discover state independently.

Your scope is LIMITED to requirements compliance:
- All spec fields present in types
- Behavior matches spec descriptions
- No spec requirements deferred or missing

## Phase-to-Spec Mapping

{PHASE_SPEC_SECTIONS}

## Required Verification Steps

Perform these checks yourself using your tools:

1. **Read the spec**: Use Read to examine DESIGN-COMMS.md sections listed above
2. **Read the implementation**: Use Read to examine the actual source files
3. **Cross-reference**: Verify every spec requirement has corresponding implementation
4. **Deferral detection**: Use Grep to search for `v0.2`, `future work`, `out of scope`, `deferred`, `later version`, `TODO.*later` in source files
   - Cross-reference against spec requirements
   - If spec-required feature appears in deferral language, block
5. **Checklist honesty**: Use Read on CHECKLIST-COMMS.md, find tasks marked [x]
   - For each, verify the "Done when" condition is actually satisfied
   - Run the test or check the file exists as specified

## Blocking Rules

You MUST block if:
- A spec field is missing from a type (evidence_type: SPEC_VIOLATION)
- Behavior contradicts spec (evidence_type: SPEC_VIOLATION)
- A spec requirement has no corresponding test (evidence_type: TEST_MISSING)
- A spec-required feature is deferred (evidence_type: INFINITE_DEFERRAL)
- Checklist task marked [x] but "Done when" condition not met (evidence_type: CHECKLIST_DISHONESTY)

## Output Format

verdict: APPROVE | BLOCK
gate: SPEC_AUDITOR
blocking:
  - id: SPEC-001
    claim: "<specific claim>"
    evidence_type: SPEC_VIOLATION | TEST_MISSING | INFINITE_DEFERRAL | CHECKLIST_DISHONESTY
    evidence: "<spec section + what you found in implementation>"
    fix: "<actionable fix>"
non_blocking:
  - id: NB-001
    note: "<suggestion>"
```

### Methodology Integrity Prompt (Final Phase Only)

```
You are the Methodology Integrity reviewer for the FINAL PHASE of meerkat-comms.

Your job is to detect methodology abuse patterns.

You have access to tools: Grep, Read, Bash. Use them to discover state independently.

## Required Verification Steps

Perform ALL these checks yourself using your tools:

### 1. Stub Detection
Use Grep to search for `todo!` and `unimplemented!` in meerkat-comms*/src/
Block if any found in completed code (evidence_type: STUB_REMAINING).

### 2. XFAIL/Skip Detection
Use Grep to search for `#[ignore]` in meerkat-comms*/src/ and meerkat-comms*/tests/
Block if on feature tests without external bug reference (evidence_type: XFAIL_ACCUMULATION).
Note: `#[should_panic]` is valid for edge case tests and should NOT be flagged.

### 3. Deferred Requirements
Use Grep to search for `v0.2`, `deferred`, `future work`, `out of scope` in source files
Use Read to examine DESIGN-COMMS.md for original requirements
Cross-reference: if spec-required features appear deferred, block (evidence_type: DEFERRED_REQUIREMENTS).

### 4. Process vs Product Ratio
Use Bash to run `git log --oneline --name-only` and examine recent commits
Count implementation file changes (.rs) vs methodology file changes (.md)
If methodology artifacts dominate recent commits, flag (evidence_type: PROCESS_DISPLACEMENT).

## Output Format

verdict: APPROVE | BLOCK
gate: METHODOLOGY_INTEGRITY
blocking:
  - id: META-001
    claim: "<specific claim>"
    evidence_type: STUB_REMAINING | XFAIL_ACCUMULATION | DEFERRED_REQUIREMENTS | PROCESS_DISPLACEMENT
    evidence: "<file location and content you found>"
    fix: "<actionable fix>"
non_blocking:
  - id: NB-001
    note: "<suggestion>"
```
