# Meerkat Comms Implementation Checklist

**Specification:** See `DESIGN-COMMS.md`

---

## RCT Methodology

**Representations first. Behavior second. Internals last.**

- Phase 0: Types + round-trip tests (MUST BE GREEN)
- Phase 1+: Behavior implementation
- Final: E2E scenarios

---

## Phase 0: Representation Contracts (RCT Gate 0 - MUST BE GREEN)

**Goal:** All types compile and round-trip through CBOR serialization.

**Dependencies:** None

### Tasks - Crate Setup

- [ ] Create `meerkat-comms/Cargo.toml` (Done when: `cargo check -p meerkat-comms` succeeds)
- [ ] Add dependencies: `serde`, `ciborium`, `uuid`, `ed25519-dalek`, `serde_json`, `tokio` (Done when: Cargo.toml has deps)
- [ ] Create `meerkat-comms/src/lib.rs` with module declarations (Done when: `mod types; mod identity;` compiles)

### Tasks - Identity Types (`identity.rs`)

- [ ] Implement `PubKey` newtype as 32-byte array (Done when: compiles)
- [ ] Implement `Signature` newtype as 64-byte array (Done when: compiles)
- [ ] Implement `PubKey::to_peer_id() -> String` returning "ed25519:" + base64 (Done when: compiles)
- [ ] Implement `PubKey::from_peer_id(s: &str) -> Result<PubKey>` (Done when: compiles)
- [ ] Derive Serialize/Deserialize for `PubKey` (Done when: serde derives compile)
- [ ] Derive Serialize/Deserialize for `Signature` (Done when: serde derives compile)

### Tasks - Core Types (`types.rs`)

- [ ] Implement `Status` enum with Accepted, Completed, Failed (Done when: compiles)
- [ ] Implement `MessageKind::Message` variant with body field (Done when: compiles)
- [ ] Implement `MessageKind::Request` variant with intent, params fields (Done when: compiles)
- [ ] Implement `MessageKind::Response` variant with in_reply_to, status, result fields (Done when: compiles)
- [ ] Implement `MessageKind::Ack` variant with in_reply_to field (Done when: compiles)
- [ ] Implement `Envelope` struct with id, from, to, kind, sig fields (Done when: compiles)
- [ ] Derive Serialize/Deserialize for `Status` (Done when: serde derives compile)
- [ ] Derive Serialize/Deserialize for `MessageKind` (Done when: serde derives compile)
- [ ] Derive Serialize/Deserialize for `Envelope` (Done when: serde derives compile)

### Tasks - Trust Types (`trust.rs`)

- [ ] Implement `TrustedPeer` struct with name, pubkey, addr fields (Done when: compiles)
- [ ] Implement `TrustedPeers` struct with peers vec (Done when: compiles)
- [ ] Derive Serialize/Deserialize for trust types (Done when: serde derives compile)

### Tasks - Inbox Types (`types.rs`)

- [ ] Implement `InboxItem::External` variant with envelope field (Done when: compiles)
- [ ] Implement `InboxItem::SubagentResult` variant with subagent_id, result, summary fields (Done when: compiles)
- [ ] Derive Serialize/Deserialize for `InboxItem` (Done when: serde derives compile)

### Tasks - RCT Round-Trip Tests

- [ ] Write round-trip test for `PubKey` CBOR (Done when: test_pubkey_cbor_roundtrip passes)
- [ ] Write round-trip test for `Signature` CBOR (Done when: test_signature_cbor_roundtrip passes)
- [ ] Write round-trip test for `Status` CBOR (Done when: test_status_cbor_roundtrip passes)
- [ ] Write round-trip test for `MessageKind::Message` (Done when: test_message_kind_message_roundtrip passes)
- [ ] Write round-trip test for `MessageKind::Request` (Done when: test_message_kind_request_roundtrip passes)
- [ ] Write round-trip test for `MessageKind::Response` (Done when: test_message_kind_response_roundtrip passes)
- [ ] Write round-trip test for `MessageKind::Ack` (Done when: test_message_kind_ack_roundtrip passes)
- [ ] Write round-trip test for `Envelope` (Done when: test_envelope_cbor_roundtrip passes)
- [ ] Write round-trip test for `TrustedPeers` JSON (Done when: test_trusted_peers_json_roundtrip passes)
- [ ] Write round-trip test for `InboxItem` (Done when: test_inbox_item_roundtrip passes)

### Tasks - RCT Invariant Tests

- [ ] Write invariant test: Status encodes as string not ordinal (Done when: test_status_string_encoding passes)
- [ ] Write invariant test: PubKey peer_id format "ed25519:base64" (Done when: test_pubkey_peer_id_format passes)
- [ ] Write invariant test: PubKey peer_id round-trip (Done when: test_pubkey_peer_id_roundtrip passes)

### Phase 0 Gate Verification

```bash
cargo test -p meerkat-comms
```

### Phase 0 Gate Review

- **RCT Guardian**: Verify all types round-trip through CBOR/JSON
- **Spec Auditor**: Verify all fields match DESIGN-COMMS.md

---

## Phase 1: Cryptographic Identity

**Goal:** Key generation, signing, and verification work.

**Dependencies:** Phase 0

### Tasks - Keypair (`identity.rs`)

- [ ] Implement `Keypair` struct (Done when: holds secret + public key)
- [ ] Implement `Keypair::generate()` (Done when: test_keypair_generate passes)
- [ ] Implement `Keypair::sign(data: &[u8]) -> Signature` (Done when: returns signature)
- [ ] Implement `PubKey::verify(data: &[u8], sig: &Signature) -> bool` (Done when: returns bool)
- [ ] Implement `Keypair::public_key() -> PubKey` (Done when: returns pubkey)

### Tasks - Envelope Signing (`types.rs`)

- [ ] Implement `Envelope::signable_bytes() -> Vec<u8>` using canonical CBOR array [id, from, to, kind] (Done when: returns deterministic bytes)
- [ ] Implement `Envelope::sign(&mut self, keypair: &Keypair)` (Done when: sets sig field)
- [ ] Implement `Envelope::verify(&self) -> bool` (Done when: verifies sig against from pubkey)

### Tasks - Key Persistence (`identity.rs`)

- [ ] Implement `Keypair::load(dir: &Path) -> Result<Keypair>` (Done when: loads identity.key + identity.pub)
- [ ] Implement `Keypair::save(&self, dir: &Path) -> Result<()>` (Done when: saves to files)
- [ ] Implement `Keypair::load_or_generate(dir: &Path) -> Keypair` (Done when: loads or creates new)

### Tasks - Tests

- [ ] Write test: sign then verify succeeds (Done when: test_sign_verify passes)
- [ ] Write test: tampered data fails verify (Done when: test_tamper_fails passes)
- [ ] Write test: wrong key fails verify (Done when: test_wrong_key_fails passes)
- [ ] Write test: keypair save and load roundtrip (Done when: test_keypair_persistence passes)
- [ ] Write test: signable_bytes is deterministic (Done when: test_signable_bytes_deterministic passes)

### Phase 1 Gate Verification

```bash
cargo test -p meerkat-comms
```

### Phase 1 Gate Review

- **RCT Guardian**: Verify sign/verify round-trip, canonical CBOR determinism
- **Integration Sheriff**: Verify tamper detection works

---

## Phase 2: Trust Management

**Goal:** Load trusted peers, check membership.

**Dependencies:** Phase 1

### Tasks - Trust Module (`trust.rs`)

- [ ] Implement `TrustedPeers::load(path: &Path) -> Result<TrustedPeers>` (Done when: loads JSON file)
- [ ] Implement `TrustedPeers::save(&self, path: &Path) -> Result<()>` (Done when: saves JSON file)
- [ ] Implement `TrustedPeers::is_trusted(&self, pubkey: &PubKey) -> bool` (Done when: checks membership)
- [ ] Implement `TrustedPeers::get_peer(&self, pubkey: &PubKey) -> Option<&TrustedPeer>` (Done when: returns peer info)
- [ ] Implement `TrustedPeers::get_by_name(&self, name: &str) -> Option<&TrustedPeer>` (Done when: finds by name)

### Tasks - Tests

- [ ] Write test: load trusted peers from JSON (Done when: test_load_trusted_peers passes)
- [ ] Write test: is_trusted returns true for listed peer (Done when: test_is_trusted passes)
- [ ] Write test: is_trusted returns false for unknown peer (Done when: test_untrusted_rejected passes)
- [ ] Write test: get_by_name finds peer (Done when: test_get_by_name passes)

### Phase 2 Gate Verification

```bash
cargo test -p meerkat-comms
```

### Phase 2 Gate Review

- **Spec Auditor**: Verify JSON format matches spec trusted_peers.json example

---

## Phase 3: Transport Layer

**Goal:** UDS and TCP transports with length-prefix framing.

**Dependencies:** Phase 0 (types)

### Tasks - Framing (`transport/mod.rs`)

- [ ] Implement `write_envelope(writer, envelope)` with length-prefix (Done when: writes 4-byte len + CBOR)
- [ ] Implement `read_envelope(reader) -> Result<Envelope>` with length-prefix (Done when: reads and deserializes)
- [ ] Implement max payload size check (1 MB) (Done when: rejects oversized messages)
- [ ] Define `TransportError` enum with Io, Timeout, MessageTooLarge variants (Done when: compiles)

### Tasks - UDS Transport (`transport/uds.rs`)

- [ ] Implement `UdsListener::bind(path)` (Done when: creates listening socket)
- [ ] Implement `UdsListener::accept() -> UdsStream` (Done when: accepts connection)
- [ ] Implement `UdsStream::connect(path)` (Done when: connects to socket)
- [ ] Implement send/recv envelope over UdsStream using framing (Done when: uses write_envelope/read_envelope)

### Tasks - TCP Transport (`transport/tcp.rs`)

- [ ] Implement `TcpListener::bind(addr)` (Done when: creates listening socket)
- [ ] Implement `TcpListener::accept() -> TcpStream` (Done when: accepts connection)
- [ ] Implement `TcpStream::connect(addr)` (Done when: connects)
- [ ] Implement send/recv envelope over TcpStream using framing (Done when: uses write_envelope/read_envelope)

### Tasks - Address Parsing

- [ ] Implement `PeerAddr` enum with Uds(PathBuf), Tcp(SocketAddr) (Done when: compiles)
- [ ] Implement `PeerAddr::parse(s: &str) -> Result<PeerAddr>` (Done when: parses "uds://..." and "tcp://...")

### Tasks - Tests

- [ ] Write test: framing round-trip (Done when: test_framing_roundtrip passes)
- [ ] Write test: reject oversized message (Done when: test_reject_oversized passes)
- [ ] Write test: UDS send and recv envelope (Done when: test_uds_roundtrip passes)
- [ ] Write test: TCP send and recv envelope (Done when: test_tcp_roundtrip passes)
- [ ] Write test: parse UDS address (Done when: test_parse_uds_addr passes)
- [ ] Write test: parse TCP address (Done when: test_parse_tcp_addr passes)

### Phase 3 Gate Verification

```bash
cargo test -p meerkat-comms
```

### Phase 3 Gate Review

- **RCT Guardian**: Verify envelope survives framing + transport round-trip
- **Integration Sheriff**: Verify both UDS and TCP work

---

## Phase 4: IO Task and Inbox

**Goal:** Thread-safe inbox, IO task handles validation and ack.

**Dependencies:** Phase 1 (signing), Phase 2 (trust), Phase 3 (transport)

### Tasks - Inbox (`inbox.rs`)

- [ ] Implement `Inbox` struct wrapping mpsc channel (Done when: compiles)
- [ ] Implement `Inbox::new() -> (Inbox, InboxSender)` (Done when: returns inbox + sender handle)
- [ ] Implement `InboxSender::send(item: InboxItem)` (Done when: sends to channel)
- [ ] Implement `Inbox::recv() -> InboxItem` async (Done when: awaits next item)
- [ ] Implement `Inbox::try_drain() -> Vec<InboxItem>` (Done when: drains available items)

### Tasks - IO Task (`io_task.rs`)

- [ ] Implement `handle_connection(stream, keypair, trusted, inbox_sender)` async (Done when: handles one connection)
- [ ] Implement read envelope in IO task (Done when: reads with framing)
- [ ] Implement signature verification in IO task (Done when: invalid sig drops message)
- [ ] Implement trust check in IO task (Done when: untrusted sender drops message)
- [ ] Implement ack sending in IO task (Done when: sends Ack for Message/Request, not for Ack/Response)
- [ ] Implement enqueue to inbox in IO task (Done when: pushes InboxItem::External)

### Tasks - Ack Rules

- [ ] Implement "never ack an Ack" rule (Done when: Ack messages don't trigger Ack response)
- [ ] Implement "don't ack a Response" rule (Done when: Response messages don't trigger Ack)

### Tasks - Tests

- [ ] Write test: inbox send and recv (Done when: test_inbox_send_recv passes)
- [ ] Write test: inbox try_drain (Done when: test_inbox_drain passes)
- [ ] Write test: IO task sends ack for Message (Done when: test_io_task_acks_message passes)
- [ ] Write test: IO task sends ack for Request (Done when: test_io_task_acks_request passes)
- [ ] Write test: IO task does NOT ack an Ack (Done when: test_io_task_no_ack_for_ack passes)
- [ ] Write test: IO task does NOT ack a Response (Done when: test_io_task_no_ack_for_response passes)
- [ ] Write test: IO task rejects invalid signature (Done when: test_io_task_rejects_invalid_sig passes)
- [ ] Write test: IO task rejects untrusted sender (Done when: test_io_task_rejects_untrusted passes)

### Phase 4 Gate Verification

```bash
cargo test -p meerkat-comms
```

### Phase 4 Gate Review

- **Integration Sheriff**: Verify IO task → validate → ack → inbox flow
- **Spec Auditor**: Verify ack rules match spec table

---

## Phase 5: Router (Send API)

**Goal:** High-level send API with ack timeout.

**Dependencies:** Phase 4

### Tasks - Router (`router.rs`)

- [ ] Implement `Router` struct (Done when: holds keypair, trusted_peers, config)
- [ ] Implement `Router::send(peer: &str, kind: MessageKind) -> Result<(), SendError>` (Done when: sends and waits for ack)
- [ ] Implement connect to peer by name lookup (Done when: resolves name to address)
- [ ] Implement ack timeout (Done when: returns error after configured timeout)
- [ ] Implement `SendError` enum with PeerNotFound, PeerOffline, Io variants (Done when: compiles)

### Tasks - Send Helpers

- [ ] Implement `Router::send_message(peer: &str, body: String)` (Done when: wraps send with Message kind)
- [ ] Implement `Router::send_request(peer: &str, intent: String, params: JsonValue)` (Done when: wraps send with Request kind)
- [ ] Implement `Router::send_response(peer: &str, in_reply_to: Uuid, status: Status, result: JsonValue)` (Done when: wraps send with Response kind)

### Tasks - Tests

- [ ] Write test: send message and receive ack (Done when: test_send_receives_ack passes)
- [ ] Write test: send to unknown peer returns PeerNotFound (Done when: test_send_unknown_peer passes)
- [ ] Write test: timeout returns PeerOffline (Done when: test_send_timeout passes)
- [ ] Write test: send_response does not wait for ack (Done when: test_send_response_no_ack_wait passes)

### Phase 5 Gate Verification

```bash
cargo test -p meerkat-comms
```

### Phase 5 Gate Review

- **Integration Sheriff**: Verify full send → ack → timeout flow
- **Spec Auditor**: Verify send behavior matches spec

---

## Phase 6: MCP Tools

**Goal:** Expose comms tools via MCP.

**Dependencies:** Phase 5

### Tasks - Crate Setup

- [ ] Create `meerkat-comms-mcp/Cargo.toml` (Done when: `cargo check -p meerkat-comms-mcp` succeeds)
- [ ] Create `meerkat-comms-mcp/src/lib.rs` (Done when: compiles)

### Tasks - Tools (`tools.rs`)

- [ ] Implement `send_message(peer, body)` tool (Done when: calls Router::send_message)
- [ ] Implement `send_request(peer, intent, params)` tool (Done when: calls Router::send_request)
- [ ] Implement `send_response(request_id, status, result)` tool (Done when: calls Router::send_response)
- [ ] Implement `list_peers()` tool (Done when: returns trusted peers list with names and addresses)

### Tasks - MCP Integration

- [ ] Register tools with MCP server (Done when: tools appear in tool list)

### Tasks - Tests

- [ ] Write test: send_message via MCP (Done when: test_mcp_send_message passes)
- [ ] Write test: list_peers via MCP (Done when: test_mcp_list_peers passes)

### Phase 6 Gate Verification

```bash
cargo test -p meerkat-comms-mcp
```

### Phase 6 Gate Review

- **Spec Auditor**: Verify all tools from spec implemented

---

## Final Phase: E2E Scenarios

**Goal:** Full system working end-to-end.

**Dependencies:** All prior phases

### Tasks - E2E Tests

- [ ] Implement E2E: Two peers exchange messages over UDS (Done when: test_e2e_uds_message passes)
- [ ] Implement E2E: Two peers exchange messages over TCP (Done when: test_e2e_tcp_message passes)
- [ ] Implement E2E: Request → Ack → Response flow (Done when: test_e2e_request_response passes)
- [ ] Implement E2E: Untrusted peer rejected (Done when: test_e2e_untrusted_rejected passes)
- [ ] Implement E2E: Concurrent messages from multiple peers (Done when: test_e2e_concurrent_messages passes)

### Tasks - Final Verification

- [ ] Run full test suite (Done when: `cargo test --workspace` passes)
- [ ] Verify no `todo!()` or `unimplemented!()` (Done when: grep returns empty)

### Final Phase Gate Verification

```bash
cargo test -p meerkat-comms
cargo test -p meerkat-comms-mcp
```

### Final Phase Gate Review

- **RCT Guardian**: All types round-trip correctly
- **Integration Sheriff**: All flows work end-to-end, concurrent messages handled
- **Spec Auditor**: 100% spec coverage

---

## Success Criteria

1. [ ] All existing meerkat tests still pass
2. [ ] Phase 0 RCT tests green
3. [ ] All E2E tests pass
4. [ ] No stubs in completed code
5. [ ] Two Meerkats can exchange messages over UDS
6. [ ] Two Meerkats can exchange messages over TCP
7. [ ] Concurrent messages from multiple peers handled correctly
