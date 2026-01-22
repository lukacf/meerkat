# Phase 0: Representation Contracts - Detailed Template

This template provides the detailed breakdown for Phase 0, which corresponds to RCT Gate 0 and MUST BE GREEN before proceeding.

## Purpose of Phase 0

Phase 0 establishes the "contract" between all system boundaries:
- Database ↔ Application types
- Wire formats ↔ Internal representations
- External systems ↔ Internal models

If these contracts aren't locked down with tests first, later phases will encounter "integration hell" where independently working components fail to communicate.

---

## Step-by-Step Phase 0 Creation

### 1. Extract All Nouns from Spec

Read the specification and create a list of every:
- **Table/collection** - Database entities
- **Struct** - Data structures
- **Enum** - Enumerated types with variants
- **Index** - Database indexes

### 2. Generate Migration Tasks

For each table:
```markdown
- [ ] Add DEFINE TABLE [table_name] (Done when: migration includes DEFINE TABLE [table_name])
```

For each index:
```markdown
- [ ] Add INDEX on [table].[field] (Done when: migration includes DEFINE INDEX)
```

### 3. Generate Type Tasks

For each struct:
```markdown
- [ ] Implement [StructName] struct in `[file].rs` (Done when: compiles with all fields from spec §[section])
```

For each enum:
```markdown
- [ ] Implement [EnumName] enum in `[file].rs` (Done when: enum compiles)
```

### 4. Generate Repository Tasks

For each persistent entity:
```markdown
- [ ] Create [Entity]Repository with CRUD (Done when: create/get/update/delete tests pass)
```

### 5. Generate RCT Test Tasks

For each type that crosses a boundary:
```markdown
- [ ] Write round-trip test for [Type] (Done when: test_[type]_roundtrip passes)
```

For each invariant from the spec:
```markdown
- [ ] Write invariant test: [description] (Done when: test_[invariant_name] passes)
```

---

## Common Invariant Tests

Include these invariant tests when applicable:

### Encoding Stability
```markdown
- [ ] Write invariant test: enum encoding stability (Done when: test_enum_encoding_stability passes)
```
Verifies all enums serialize as strings, not ordinals.

### NULL vs NONE Semantics
```markdown
- [ ] Write invariant test: NULL vs NONE semantics (Done when: test_null_none_semantics passes)
```
Verifies optional fields behave correctly across boundaries.

### Link/Reference Stability
```markdown
- [ ] Write invariant test: link stability (Done when: test_link_stability passes)
```
Verifies foreign key references serialize/deserialize correctly.

### Uniqueness Constraints
```markdown
- [ ] Write invariant test: [field] uniqueness (Done when: test_[field]_uniqueness passes)
```
Verifies unique constraints are enforced.

### Immutability
```markdown
- [ ] Write invariant test: [entity] immutability (Done when: test_[entity]_immutability passes)
```
Verifies immutable records cannot be modified.

### Security Inheritance
```markdown
- [ ] Write invariant test: security inheritance (Done when: test_security_inheritance passes)
```
Verifies derived records inherit security envelopes.

### Determinism
```markdown
- [ ] Write invariant test: [computation] determinism (Done when: test_[computation]_determinism passes)
```
Verifies deterministic computations produce same results.

### Temporal Consistency
```markdown
- [ ] Write invariant test: validity window consistency (Done when: test_validity_window_consistency passes)
```
Verifies temporal ranges are valid (start ≤ end).

---

## Example Phase 0 (Backend System)

```markdown
## Phase 0: Representation Contracts (RCT Gate 0 - MUST BE GREEN)

**Goal:** Lock down all core nouns across DB ↔ types ↔ storage proxies. No behavior yet.

**Dependencies:** None (this is the foundation)

### Tasks - Migration

- [ ] Create `migrations/001_schema.sql` (Done when: file exists)
- [ ] Add CREATE TABLE users (Done when: migration includes CREATE TABLE users)
- [ ] Add CREATE TABLE orders (Done when: migration includes CREATE TABLE orders)
- [ ] Add CREATE TABLE order_items (Done when: migration includes CREATE TABLE order_items)
- [ ] Add INDEX on orders.user_id (Done when: migration includes CREATE INDEX)
- [ ] Add INDEX on orders.status (Done when: migration includes CREATE INDEX)

### Tasks - Types

- [ ] Create `src/types/mod.rs` (Done when: module compiles with submodule declarations)
- [ ] Implement User struct in `user.rs` (Done when: compiles with all fields from spec §2.1)
- [ ] Implement Order struct in `order.rs` (Done when: compiles with all fields from spec §2.2)
- [ ] Implement OrderItem struct in `order.rs` (Done when: compiles)
- [ ] Implement OrderStatus enum in `order.rs` (Done when: enum compiles)
- [ ] Implement PaymentMethod enum in `payment.rs` (Done when: enum compiles)

### Tasks - Repositories

- [ ] Create UserRepository with CRUD (Done when: create/get/update/delete tests pass)
- [ ] Create OrderRepository with CRUD (Done when: create/get/update/delete tests pass)
- [ ] Create OrderItemRepository with CRUD (Done when: create/get/update/delete tests pass)

### Tasks - RCT Tests

- [ ] Write round-trip test for User (Done when: test_user_roundtrip passes)
- [ ] Write round-trip test for Order (Done when: test_order_roundtrip passes)
- [ ] Write round-trip test for OrderItem (Done when: test_order_item_roundtrip passes)
- [ ] Write invariant test: OrderStatus enum encoding (Done when: test_order_status_encoding passes)
- [ ] Write invariant test: order.user_id foreign key (Done when: test_order_user_fk passes)
- [ ] Write invariant test: order_item.order_id foreign key (Done when: test_order_item_fk passes)

### Phase 0 Gate Verification Commands

\`\`\`bash
cargo test -p types
cargo test -p storage -- --test-threads=1
\`\`\`

### Phase 0 Gate Review

Spawn reviewers IN PARALLEL:
- **RCT Guardian**: Verify representation round-trips for User, Order, OrderItem, all enums
- **Spec Auditor**: Verify all §2 fields present on all tables
```

---

## Phase 0 Completion Criteria

Phase 0 is complete when:

1. All migration tasks are done (tables and indexes exist)
2. All types compile with correct fields
3. All repositories have passing CRUD tests
4. All round-trip tests pass
5. All invariant tests pass
6. Gate verification commands succeed
7. All reviewers APPROVE (no BLOCK verdicts)

**Only then proceed to Phase 1.**
