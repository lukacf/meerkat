---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for GrantAuthorityMachine.

\* RustU64Max is the TLA boundary for Expr::U64Max; production generated Rust renders u64::MAX.
CONSTANTS BooleanValues, DecisionIdValues, DerivedChildRestrictionsValues, EvidenceIdValues, GrantAuthorityIncarnationValues, GrantOperationBindingValues, GrantPrincipalValues, GrantRecordValues, NatValues, OriginalWorkRefValues, SetOfDecisionIdValues, SetOfEvidenceIdValues, Sha256DigestValues, RustU64Max

None == [tag |-> "none", value |-> "none"]
Some(v) == [tag |-> "some", value |-> v]

DecisionIdValuesCi == {"decision_1"}
DerivedChildRestrictionsValuesCi == {[effective |-> [actions |-> "unrestricted_actions", audiences |-> "unrestricted_audiences", delegation_depth |-> [bound |-> [edges |-> 0, tag |-> "Remaining"], remaining_edges |-> 0, unresolved |-> {}], lifetime |-> [bound |-> [expires_at_ms |-> 2, not_before_ms |-> 1, tag |-> "Window"], expires_at_ms |-> 2, not_before_ms |-> 1, unresolved |-> {}], processors |-> "unrestricted_processors", resource_domains |-> "unrestricted_resources"], parent |-> [actions |-> "unrestricted_actions", audiences |-> "unrestricted_audiences", delegation_depth |-> [bound |-> [edges |-> 1, tag |-> "Remaining"], remaining_edges |-> 1, unresolved |-> {}], lifetime |-> [bound |-> [expires_at_ms |-> 3, not_before_ms |-> 0, tag |-> "Window"], expires_at_ms |-> 3, not_before_ms |-> 0, unresolved |-> {}], processors |-> "unrestricted_processors", resource_domains |-> "unrestricted_resources"], requested |-> [actions |-> "unrestricted_actions", audiences |-> "unrestricted_audiences", delegation_depth |-> [bound |-> [edges |-> 0, tag |-> "Remaining"], remaining_edges |-> 0, unresolved |-> {}], lifetime |-> [bound |-> [expires_at_ms |-> 2, not_before_ms |-> 1, tag |-> "Window"], expires_at_ms |-> 2, not_before_ms |-> 1, unresolved |-> {}], processors |-> "unrestricted_processors", resource_domains |-> "unrestricted_resources"]]}
EvidenceIdValuesCi == {"grant_root", "grant_child"}
GrantAuthorityIncarnationValuesCi == {"owner_incarnation"}
GrantOperationBindingValuesCi == {"operation_1"}
GrantPrincipalValuesCi == {"principal_a", "principal_b"}
GrantRecordValuesCi == {[approval |-> None, authority_incarnation |-> "owner_incarnation", child_approval |-> None, grantee |-> "principal_b", id |-> "grant_root", issued_revision |-> 1, issuer |-> "principal_a", parent |-> None, represented_subject |-> None, restrictions |-> [actions |-> "unrestricted_actions", audiences |-> "unrestricted_audiences", delegation_depth |-> [bound |-> [edges |-> 1, tag |-> "Remaining"], remaining_edges |-> 1, unresolved |-> {}], lifetime |-> [bound |-> [expires_at_ms |-> 3, not_before_ms |-> 0, tag |-> "Window"], expires_at_ms |-> 3, not_before_ms |-> 0, unresolved |-> {}], processors |-> "unrestricted_processors", resource_domains |-> "unrestricted_resources"]], [approval |-> None, authority_incarnation |-> "owner_incarnation", child_approval |-> None, grantee |-> "principal_a", id |-> "grant_child", issued_revision |-> 2, issuer |-> "principal_b", parent |-> Some("grant_root"), represented_subject |-> None, restrictions |-> [actions |-> "unrestricted_actions", audiences |-> "unrestricted_audiences", delegation_depth |-> [bound |-> [edges |-> 0, tag |-> "Remaining"], remaining_edges |-> 0, unresolved |-> {}], lifetime |-> [bound |-> [expires_at_ms |-> 2, not_before_ms |-> 1, tag |-> "Window"], expires_at_ms |-> 2, not_before_ms |-> 1, unresolved |-> {}], processors |-> "unrestricted_processors", resource_domains |-> "unrestricted_resources"]], [approval |-> None, authority_incarnation |-> "owner_incarnation", child_approval |-> None, grantee |-> "principal_a", id |-> "grant_root", issued_revision |-> 1, issuer |-> "principal_b", parent |-> None, represented_subject |-> None, restrictions |-> [actions |-> "unrestricted_actions", audiences |-> "unrestricted_audiences", delegation_depth |-> [bound |-> [edges |-> 1, tag |-> "Remaining"], remaining_edges |-> 1, unresolved |-> {}], lifetime |-> [bound |-> [expires_at_ms |-> 3, not_before_ms |-> 0, tag |-> "Window"], expires_at_ms |-> 3, not_before_ms |-> 0, unresolved |-> {}], processors |-> "unrestricted_processors", resource_domains |-> "unrestricted_resources"]], [approval |-> None, authority_incarnation |-> "owner_incarnation", child_approval |-> None, grantee |-> "principal_b", id |-> "grant_child", issued_revision |-> 2, issuer |-> "principal_a", parent |-> Some("grant_root"), represented_subject |-> None, restrictions |-> [actions |-> "unrestricted_actions", audiences |-> "unrestricted_audiences", delegation_depth |-> [bound |-> [edges |-> 0, tag |-> "Remaining"], remaining_edges |-> 0, unresolved |-> {}], lifetime |-> [bound |-> [expires_at_ms |-> 2, not_before_ms |-> 1, tag |-> "Window"], expires_at_ms |-> 2, not_before_ms |-> 1, unresolved |-> {}], processors |-> "unrestricted_processors", resource_domains |-> "unrestricted_resources"]], [approval |-> None, authority_incarnation |-> "owner_incarnation", child_approval |-> Some([accepts_device_trust |-> "Refused", approver_id |-> "approver_owner"]), grantee |-> "principal_b", id |-> "grant_root", issued_revision |-> 1, issuer |-> "principal_a", parent |-> None, represented_subject |-> None, restrictions |-> [actions |-> "unrestricted_actions", audiences |-> "unrestricted_audiences", delegation_depth |-> [bound |-> [edges |-> 1, tag |-> "Remaining"], remaining_edges |-> 1, unresolved |-> {}], lifetime |-> [bound |-> [expires_at_ms |-> 3, not_before_ms |-> 0, tag |-> "Window"], expires_at_ms |-> 3, not_before_ms |-> 0, unresolved |-> {}], processors |-> "unrestricted_processors", resource_domains |-> "unrestricted_resources"]], [approval |-> Some([approver_id |-> "approver_owner", assurance |-> "PersonAuthenticated", decision_id |-> "decision_1", operation |-> None, request_digest |-> "digest_1", requester |-> "principal_a", review_before_return |-> None, tier |-> "Human", uses |-> 1, work |-> "work_1"]), authority_incarnation |-> "owner_incarnation", child_approval |-> None, grantee |-> "principal_a", id |-> "grant_child", issued_revision |-> 2, issuer |-> "principal_b", parent |-> Some("grant_root"), represented_subject |-> None, restrictions |-> [actions |-> "unrestricted_actions", audiences |-> "unrestricted_audiences", delegation_depth |-> [bound |-> [edges |-> 0, tag |-> "Remaining"], remaining_edges |-> 0, unresolved |-> {}], lifetime |-> [bound |-> [expires_at_ms |-> 2, not_before_ms |-> 1, tag |-> "Window"], expires_at_ms |-> 2, not_before_ms |-> 1, unresolved |-> {}], processors |-> "unrestricted_processors", resource_domains |-> "unrestricted_resources"]]}
OriginalWorkRefValuesCi == {"work_1"}
Sha256DigestValuesCi == {"digest_1"}

DecisionIdValuesDeep == {"decision_1"}
DerivedChildRestrictionsValuesDeep == {[effective |-> [actions |-> "unrestricted_actions", audiences |-> "unrestricted_audiences", delegation_depth |-> [bound |-> [edges |-> 0, tag |-> "Remaining"], remaining_edges |-> 0, unresolved |-> {}], lifetime |-> [bound |-> [expires_at_ms |-> 2, not_before_ms |-> 1, tag |-> "Window"], expires_at_ms |-> 2, not_before_ms |-> 1, unresolved |-> {}], processors |-> "unrestricted_processors", resource_domains |-> "unrestricted_resources"], parent |-> [actions |-> "unrestricted_actions", audiences |-> "unrestricted_audiences", delegation_depth |-> [bound |-> [edges |-> 1, tag |-> "Remaining"], remaining_edges |-> 1, unresolved |-> {}], lifetime |-> [bound |-> [expires_at_ms |-> 3, not_before_ms |-> 0, tag |-> "Window"], expires_at_ms |-> 3, not_before_ms |-> 0, unresolved |-> {}], processors |-> "unrestricted_processors", resource_domains |-> "unrestricted_resources"], requested |-> [actions |-> "unrestricted_actions", audiences |-> "unrestricted_audiences", delegation_depth |-> [bound |-> [edges |-> 0, tag |-> "Remaining"], remaining_edges |-> 0, unresolved |-> {}], lifetime |-> [bound |-> [expires_at_ms |-> 2, not_before_ms |-> 1, tag |-> "Window"], expires_at_ms |-> 2, not_before_ms |-> 1, unresolved |-> {}], processors |-> "unrestricted_processors", resource_domains |-> "unrestricted_resources"]]}
EvidenceIdValuesDeep == {"grant_root", "grant_child"}
GrantAuthorityIncarnationValuesDeep == {"owner_incarnation"}
GrantOperationBindingValuesDeep == {"operation_1"}
GrantPrincipalValuesDeep == {"principal_a", "principal_b"}
GrantRecordValuesDeep == {[approval |-> None, authority_incarnation |-> "owner_incarnation", child_approval |-> None, grantee |-> "principal_b", id |-> "grant_root", issued_revision |-> 1, issuer |-> "principal_a", parent |-> None, represented_subject |-> None, restrictions |-> [actions |-> "unrestricted_actions", audiences |-> "unrestricted_audiences", delegation_depth |-> [bound |-> [edges |-> 1, tag |-> "Remaining"], remaining_edges |-> 1, unresolved |-> {}], lifetime |-> [bound |-> [expires_at_ms |-> 3, not_before_ms |-> 0, tag |-> "Window"], expires_at_ms |-> 3, not_before_ms |-> 0, unresolved |-> {}], processors |-> "unrestricted_processors", resource_domains |-> "unrestricted_resources"]], [approval |-> None, authority_incarnation |-> "owner_incarnation", child_approval |-> None, grantee |-> "principal_a", id |-> "grant_child", issued_revision |-> 2, issuer |-> "principal_b", parent |-> Some("grant_root"), represented_subject |-> None, restrictions |-> [actions |-> "unrestricted_actions", audiences |-> "unrestricted_audiences", delegation_depth |-> [bound |-> [edges |-> 0, tag |-> "Remaining"], remaining_edges |-> 0, unresolved |-> {}], lifetime |-> [bound |-> [expires_at_ms |-> 2, not_before_ms |-> 1, tag |-> "Window"], expires_at_ms |-> 2, not_before_ms |-> 1, unresolved |-> {}], processors |-> "unrestricted_processors", resource_domains |-> "unrestricted_resources"]], [approval |-> None, authority_incarnation |-> "owner_incarnation", child_approval |-> None, grantee |-> "principal_a", id |-> "grant_root", issued_revision |-> 1, issuer |-> "principal_b", parent |-> None, represented_subject |-> None, restrictions |-> [actions |-> "unrestricted_actions", audiences |-> "unrestricted_audiences", delegation_depth |-> [bound |-> [edges |-> 1, tag |-> "Remaining"], remaining_edges |-> 1, unresolved |-> {}], lifetime |-> [bound |-> [expires_at_ms |-> 3, not_before_ms |-> 0, tag |-> "Window"], expires_at_ms |-> 3, not_before_ms |-> 0, unresolved |-> {}], processors |-> "unrestricted_processors", resource_domains |-> "unrestricted_resources"]], [approval |-> None, authority_incarnation |-> "owner_incarnation", child_approval |-> None, grantee |-> "principal_b", id |-> "grant_child", issued_revision |-> 2, issuer |-> "principal_a", parent |-> Some("grant_root"), represented_subject |-> None, restrictions |-> [actions |-> "unrestricted_actions", audiences |-> "unrestricted_audiences", delegation_depth |-> [bound |-> [edges |-> 0, tag |-> "Remaining"], remaining_edges |-> 0, unresolved |-> {}], lifetime |-> [bound |-> [expires_at_ms |-> 2, not_before_ms |-> 1, tag |-> "Window"], expires_at_ms |-> 2, not_before_ms |-> 1, unresolved |-> {}], processors |-> "unrestricted_processors", resource_domains |-> "unrestricted_resources"]], [approval |-> None, authority_incarnation |-> "owner_incarnation", child_approval |-> Some([accepts_device_trust |-> "Refused", approver_id |-> "approver_owner"]), grantee |-> "principal_b", id |-> "grant_root", issued_revision |-> 1, issuer |-> "principal_a", parent |-> None, represented_subject |-> None, restrictions |-> [actions |-> "unrestricted_actions", audiences |-> "unrestricted_audiences", delegation_depth |-> [bound |-> [edges |-> 1, tag |-> "Remaining"], remaining_edges |-> 1, unresolved |-> {}], lifetime |-> [bound |-> [expires_at_ms |-> 3, not_before_ms |-> 0, tag |-> "Window"], expires_at_ms |-> 3, not_before_ms |-> 0, unresolved |-> {}], processors |-> "unrestricted_processors", resource_domains |-> "unrestricted_resources"]], [approval |-> Some([approver_id |-> "approver_owner", assurance |-> "PersonAuthenticated", decision_id |-> "decision_1", operation |-> None, request_digest |-> "digest_1", requester |-> "principal_a", review_before_return |-> None, tier |-> "Human", uses |-> 1, work |-> "work_1"]), authority_incarnation |-> "owner_incarnation", child_approval |-> None, grantee |-> "principal_a", id |-> "grant_child", issued_revision |-> 2, issuer |-> "principal_b", parent |-> Some("grant_root"), represented_subject |-> None, restrictions |-> [actions |-> "unrestricted_actions", audiences |-> "unrestricted_audiences", delegation_depth |-> [bound |-> [edges |-> 0, tag |-> "Remaining"], remaining_edges |-> 0, unresolved |-> {}], lifetime |-> [bound |-> [expires_at_ms |-> 2, not_before_ms |-> 1, tag |-> "Window"], expires_at_ms |-> 2, not_before_ms |-> 1, unresolved |-> {}], processors |-> "unrestricted_processors", resource_domains |-> "unrestricted_resources"]], [approval |-> None, authority_incarnation |-> "owner_incarnation", child_approval |-> None, grantee |-> "principal_b", id |-> "grant_root", issued_revision |-> 1, issuer |-> "principal_a", parent |-> None, represented_subject |-> None, restrictions |-> [actions |-> "unrestricted_actions", audiences |-> "unrestricted_audiences", delegation_depth |-> [bound |-> [edges |-> 1, tag |-> "Remaining"], remaining_edges |-> 1, unresolved |-> {}], lifetime |-> [bound |-> [tag |-> "Unrestricted"], expires_at_ms |-> RustU64Max, not_before_ms |-> 0, unresolved |-> {"Unknown"}], processors |-> "unrestricted_processors", resource_domains |-> "unrestricted_resources"]], [approval |-> None, authority_incarnation |-> "foreign_incarnation", child_approval |-> None, grantee |-> "principal_b", id |-> "grant_root", issued_revision |-> 1, issuer |-> "principal_a", parent |-> None, represented_subject |-> None, restrictions |-> [actions |-> "unrestricted_actions", audiences |-> "unrestricted_audiences", delegation_depth |-> [bound |-> [edges |-> 1, tag |-> "Remaining"], remaining_edges |-> 1, unresolved |-> {}], lifetime |-> [bound |-> [expires_at_ms |-> 3, not_before_ms |-> 0, tag |-> "Window"], expires_at_ms |-> 3, not_before_ms |-> 0, unresolved |-> {}], processors |-> "unrestricted_processors", resource_domains |-> "unrestricted_resources"]]}
OriginalWorkRefValuesDeep == {"work_1"}
Sha256DigestValuesDeep == {"digest_1"}

MapEvidenceIdGrantRecordValues == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in EvidenceIdValues, v \in GrantRecordValues }
MapEvidenceIdU64Values == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in EvidenceIdValues, v \in NatValues }
OptionEvidenceIdValues == {None} \cup {Some(x) : x \in EvidenceIdValues}
OptionGrantAuthorityIncarnationValues == {None} \cup {Some(x) : x \in GrantAuthorityIncarnationValues}
OptionGrantOperationBindingValues == {None} \cup {Some(x) : x \in GrantOperationBindingValues}
OptionGrantPrincipalValues == {None} \cup {Some(x) : x \in GrantPrincipalValues}
OptionOriginalWorkRefValues == {None} \cup {Some(x) : x \in OriginalWorkRefValues}
SeqOfGrantRecordValues == {<<>>} \cup {<<x>> : x \in GrantRecordValues} \cup {<<x, y>> : x \in GrantRecordValues, y \in GrantRecordValues}

MapLookup(map, key) == IF key \in DOMAIN map THEN map[key] ELSE None
MapSet(map, key, value) == [x \in DOMAIN map \cup {key} |-> IF x = key THEN value ELSE map[x]]
MapIncrement(map, key, amount) == [x \in DOMAIN map \cup {key} |-> IF x = key THEN (IF key \in DOMAIN map THEN map[key] ELSE 0) + amount ELSE map[x]]
MapDecrement(map, key, amount) == [x \in DOMAIN map \cup {key} |-> IF x = key THEN (IF key \in DOMAIN map THEN map[key] ELSE 0) - amount ELSE map[x]]
MapRemove(map, key) == [x \in DOMAIN map \ {key} |-> map[x]]
StartsWith(seq, prefix) == /\ Len(prefix) <= Len(seq) /\ SubSeq(seq, 1, Len(prefix)) = prefix
SeqElements(seq) == {seq[i] : i \in 1..Len(seq)}
Count(seq, value) == Cardinality({i \in DOMAIN seq : seq[i] = value})
RECURSIVE SeqRemove(_, _)
SeqRemove(seq, value) == IF Len(seq) = 0 THEN <<>> ELSE IF Head(seq) = value THEN SeqRemove(Tail(seq), value) ELSE <<Head(seq)>> \o SeqRemove(Tail(seq), value)
RECURSIVE SeqRemoveAll(_, _)
SeqRemoveAll(seq, values) == IF Len(values) = 0 THEN seq ELSE SeqRemoveAll(SeqRemove(seq, Head(values)), Tail(values))

VARIABLES phase, model_step_count, root, namespace, generation, incarnation, revision, records, revoked, spent, consumed, carried_decisions

vars == << phase, model_step_count, root, namespace, generation, incarnation, revision, records, revoked, spent, consumed, carried_decisions >>

approval_use_bound(leaf, requester, work, operation) == (IF (leaf.approval = None) THEN TRUE ELSE ((Some((IF "value" \in DOMAIN leaf.approval THEN leaf.approval["value"] ELSE None).requester) = requester) /\ (Some((IF "value" \in DOMAIN leaf.approval THEN leaf.approval["value"] ELSE None).work) = work) /\ (IF ((IF "value" \in DOMAIN leaf.approval THEN leaf.approval["value"] ELSE None).operation = None) THEN TRUE ELSE ((IF "value" \in DOMAIN leaf.approval THEN leaf.approval["value"] ELSE None).operation = operation))))
approval_bounded(record) == (IF (record.approval = None) THEN TRUE ELSE ((Cardinality(record.restrictions.delegation_depth.unresolved) = 0) /\ (record.restrictions.delegation_depth.bound.tag = "Remaining") /\ (record.restrictions.delegation_depth.remaining_edges = 0) /\ (Cardinality(record.restrictions.lifetime.unresolved) = 0) /\ (record.restrictions.lifetime.bound.tag = "Window") /\ ((IF "value" \in DOMAIN record.approval THEN record.approval["value"] ELSE None).uses >= 1)))
approval_rule_admits(parent, record) == ((IF (record.approval = None) THEN TRUE ELSE (parent.child_approval # None)) /\ (IF (parent.child_approval = None) THEN TRUE ELSE ((record.approval # None) /\ ((IF "value" \in DOMAIN record.approval THEN record.approval["value"] ELSE None).approver_id = (IF "value" \in DOMAIN parent.child_approval THEN parent.child_approval["value"] ELSE None).approver_id) /\ ((IF "value" \in DOMAIN record.approval THEN record.approval["value"] ELSE None).tier = "Human") /\ (IF ((IF "value" \in DOMAIN record.approval THEN record.approval["value"] ELSE None).assurance = "PersonAuthenticated") THEN TRUE ELSE (((IF "value" \in DOMAIN record.approval THEN record.approval["value"] ELSE None).assurance = "DeviceTrusted") /\ ((IF "value" \in DOMAIN parent.child_approval THEN parent.child_approval["value"] ELSE None).accepts_device_trust = "Accepted"))))))
child_rank(parent, child) == (IF (parent.delegation_depth.bound.tag = "Unrestricted") THEN TRUE ELSE ((parent.delegation_depth.bound.tag = "Remaining") /\ (child.delegation_depth.bound.tag = "Remaining") /\ (child.delegation_depth.remaining_edges < parent.delegation_depth.remaining_edges)))
lifetime_current(restrictions, now_ms) == ((Cardinality(restrictions.lifetime.unresolved) = 0) /\ (Cardinality(restrictions.delegation_depth.unresolved) = 0) /\ (IF (restrictions.lifetime.bound.tag = "Unrestricted") THEN TRUE ELSE ((restrictions.lifetime.bound.tag = "Window") /\ (restrictions.lifetime.not_before_ms <= now_ms) /\ (now_ms < restrictions.lifetime.expires_at_ms))))
chain_links(chain, arg_root, now_ms) == ((Len(chain) > 0) /\ (Len(chain) <= 64) /\ (\A link \in SeqElements(chain) : (lifetime_current(link.restrictions, now_ms) /\ (IF (link.parent = None) THEN (Some(link.issuer) = arg_root) ELSE (\E parent \in SeqElements(chain) : ((link.parent = Some(parent.id)) /\ (link.issuer = parent.grantee) /\ (link.represented_subject = parent.represented_subject) /\ (parent.issued_revision < link.issued_revision) /\ child_rank(parent.restrictions, link.restrictions)))))))

Init ==
    /\ phase = "Unconfigured"
    /\ model_step_count = 0
    /\ root = None
    /\ namespace = None
    /\ generation = 0
    /\ incarnation = None
    /\ revision = 0
    /\ records = [x \in {} |-> None]
    /\ revoked = {}
    /\ spent = [x \in {} |-> None]
    /\ consumed = 0
    /\ carried_decisions = {}

TerminalStutter ==
    /\ phase = "Released"
    /\ UNCHANGED vars

\* Named UNCHANGED frames. One definition per distinct frame; every action
\* that leaves those variables unchanged references the definition by name.
UnchangedFrame_0f02c09694d21da7 == UNCHANGED << root, namespace, generation, incarnation, revoked, spent, consumed, carried_decisions >>
UnchangedFrame_6bb9de584729bfd0 == UNCHANGED << root, namespace, generation, incarnation, revision, records, revoked, spent, consumed, carried_decisions >>
UnchangedFrame_a4d9d8b47701daa3 == UNCHANGED << root, namespace, generation, incarnation, records, revoked, carried_decisions >>
UnchangedFrame_a7a7718dd5c94c3d == UNCHANGED << revision, records, revoked, spent, consumed, carried_decisions >>
UnchangedFrame_c2aefe11d855202f == UNCHANGED << root, namespace, generation, incarnation, records, spent, consumed, carried_decisions >>
UnchangedFrame_f186755f3b9b8bf1 == UNCHANGED << root, namespace, generation, incarnation, revoked, consumed >>

Configure(arg_root, arg_namespace, arg_generation, arg_incarnation) ==
    /\ phase = "Unconfigured"
    /\ (arg_generation > 0)
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ root' = Some(arg_root)
    /\ namespace' = Some(arg_namespace)
    /\ generation' = arg_generation
    /\ incarnation' = Some(arg_incarnation)
    /\ UnchangedFrame_a7a7718dd5c94c3d


IssueRoot(actor, record) ==
    /\ phase = "Active"
    /\ (Some(actor) = root)
    /\ (Some(record.authority_incarnation) = incarnation)
    /\ ((record.issuer = actor) /\ (record.parent = None))
    /\ (record.approval = None)
    /\ (((record.id \in DOMAIN records) = FALSE) /\ (revision < RustU64Max))
    /\ (record.issued_revision = (revision + 1))
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ records' = MapSet(records, record.id, record)
    /\ UnchangedFrame_0f02c09694d21da7


IssueChild(actor, record, derived, chain, now_ms, terms_digest) ==
    /\ phase = "Active"
    /\ (revision < RustU64Max)
    /\ (Some(record.authority_incarnation) = incarnation)
    /\ (((record.id \in DOMAIN records) = FALSE) /\ (record.issued_revision = (revision + 1)))
    /\ ((64 > Len(chain)) /\ chain_links(chain, root, now_ms))
    /\ (\A link \in SeqElements(chain) : (((IF (link.id \in DOMAIN records) THEN Some((IF link.id \in DOMAIN records THEN records[link.id] ELSE "None")) ELSE None) = Some(link)) /\ ((link.id \in revoked) = FALSE)))
    /\ ((record.issuer = actor) /\ (record.restrictions = derived.effective) /\ (\E parent \in SeqElements(chain) : ((record.parent = Some(parent.id)) /\ (parent.grantee = actor) /\ (record.represented_subject = parent.represented_subject) /\ (derived.parent = parent.restrictions) /\ child_rank(parent.restrictions, record.restrictions) /\ approval_rule_admits(parent, record))))
    /\ approval_bounded(record)
    /\ (IF (record.approval = None) THEN TRUE ELSE (terms_digest = (IF "value" \in DOMAIN record.approval THEN record.approval["value"] ELSE None).request_digest))
    /\ (IF (record.approval = None) THEN TRUE ELSE (((IF "value" \in DOMAIN record.approval THEN record.approval["value"] ELSE None).decision_id \in carried_decisions) = FALSE))
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ records' = MapSet(records, record.id, record)
    /\ spent' = IF (record.approval # None) THEN MapSet(spent, record.id, 0) ELSE spent
    /\ carried_decisions' = IF (record.approval # None) THEN (carried_decisions \cup {(IF "value" \in DOMAIN record.approval THEN record.approval["value"] ELSE None).decision_id}) ELSE carried_decisions
    /\ UnchangedFrame_f186755f3b9b8bf1


RevokeNew(actor, record) ==
    /\ phase = "Active"
    /\ (revision < RustU64Max)
    /\ ((IF (record.id \in DOMAIN records) THEN Some((IF record.id \in DOMAIN records THEN records[record.id] ELSE "None")) ELSE None) = Some(record))
    /\ (IF (Some(actor) = root) THEN TRUE ELSE (actor = record.issuer))
    /\ ((record.id \in revoked) = FALSE)
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ revoked' = (revoked \cup {record.id})
    /\ UnchangedFrame_c2aefe11d855202f


RevokeAlready(actor, record) ==
    /\ phase = "Active"
    /\ ((IF (record.id \in DOMAIN records) THEN Some((IF record.id \in DOMAIN records THEN records[record.id] ELSE "None")) ELSE None) = Some(record))
    /\ (IF (Some(actor) = root) THEN TRUE ELSE (actor = record.issuer))
    /\ (record.id \in revoked)
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_6bb9de584729bfd0


ResolveUse(arg_namespace, arg_generation, arg_incarnation, executor, represented_subject, requester, work, operation, leaf, chain, now_ms) ==
    /\ phase = "Active"
    /\ ((Some(arg_namespace) = namespace) /\ (arg_generation = generation) /\ (Some(arg_incarnation) = incarnation))
    /\ ((leaf.grantee = executor) /\ (leaf.represented_subject = represented_subject) /\ (leaf \in SeqElements(chain)))
    /\ chain_links(chain, root, now_ms)
    /\ (\A link \in SeqElements(chain) : (((IF (link.id \in DOMAIN records) THEN Some((IF link.id \in DOMAIN records THEN records[link.id] ELSE "None")) ELSE None) = Some(link)) /\ ((link.id \in revoked) = FALSE)))
    /\ approval_use_bound(leaf, requester, work, operation)
    /\ ((IF (leaf.approval = None) THEN TRUE ELSE (leaf.id \in DOMAIN spent)) /\ (IF (leaf.approval = None) THEN TRUE ELSE ((spent)[leaf.id] < (IF "value" \in DOMAIN leaf.approval THEN leaf.approval["value"] ELSE None).uses)))
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_6bb9de584729bfd0


ConsumeUse(arg_namespace, arg_generation, arg_incarnation, executor, represented_subject, requester, work, operation, leaf, chain, now_ms, approver_current) ==
    /\ phase = "Active"
    /\ ((Some(arg_namespace) = namespace) /\ (arg_generation = generation) /\ (Some(arg_incarnation) = incarnation))
    /\ (revision < RustU64Max)
    /\ ((leaf.grantee = executor) /\ (leaf.represented_subject = represented_subject) /\ (leaf \in SeqElements(chain)))
    /\ chain_links(chain, root, now_ms)
    /\ (\A link \in SeqElements(chain) : (((IF (link.id \in DOMAIN records) THEN Some((IF link.id \in DOMAIN records THEN records[link.id] ELSE "None")) ELSE None) = Some(link)) /\ ((link.id \in revoked) = FALSE)))
    /\ ((leaf.approval # None) /\ approval_use_bound(leaf, requester, work, operation))
    /\ ((leaf.id \in DOMAIN spent) /\ ((spent)[leaf.id] < (IF "value" \in DOMAIN leaf.approval THEN leaf.approval["value"] ELSE None).uses))
    /\ approver_current
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ spent' = MapSet(spent, leaf.id, ((IF "value" \in DOMAIN (IF (leaf.id \in DOMAIN spent) THEN Some((IF leaf.id \in DOMAIN spent THEN spent[leaf.id] ELSE 0)) ELSE None) THEN (IF (leaf.id \in DOMAIN spent) THEN Some((IF leaf.id \in DOMAIN spent THEN spent[leaf.id] ELSE 0)) ELSE None)["value"] ELSE None) + 1))
    /\ consumed' = (consumed) + 1
    /\ UnchangedFrame_a4d9d8b47701daa3


ConsumeUseRefusedApproverRevoked(arg_namespace, arg_generation, arg_incarnation, executor, represented_subject, requester, work, operation, leaf, chain, now_ms, approver_current) ==
    /\ phase = "Active"
    /\ ((Some(arg_namespace) = namespace) /\ (arg_generation = generation) /\ (Some(arg_incarnation) = incarnation))
    /\ ((leaf.grantee = executor) /\ (leaf.represented_subject = represented_subject) /\ (leaf \in SeqElements(chain)))
    /\ chain_links(chain, root, now_ms)
    /\ (\A link \in SeqElements(chain) : (((IF (link.id \in DOMAIN records) THEN Some((IF link.id \in DOMAIN records THEN records[link.id] ELSE "None")) ELSE None) = Some(link)) /\ ((link.id \in revoked) = FALSE)))
    /\ ((leaf.approval # None) /\ approval_use_bound(leaf, requester, work, operation))
    /\ ((leaf.id \in DOMAIN spent) /\ ((spent)[leaf.id] < (IF "value" \in DOMAIN leaf.approval THEN leaf.approval["value"] ELSE None).uses))
    /\ (approver_current = FALSE)
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_6bb9de584729bfd0


ReleaseCustody ==
    /\ phase = "Active"
    /\ phase' = "Released"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_6bb9de584729bfd0


RecordUncertainAppend ==
    /\ phase = "Active"
    /\ phase' = "Uncertain"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_6bb9de584729bfd0


ReleaseUncertainCustody ==
    /\ phase = "Uncertain"
    /\ phase' = "Released"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_6bb9de584729bfd0


ReleaseCustodyAgain ==
    /\ phase = "Released"
    /\ phase' = "Released"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_6bb9de584729bfd0


Next ==
    \/ (phase = "Unconfigured") /\ \E arg_root \in GrantPrincipalValues : \E arg_namespace \in EvidenceIdValues : \E arg_generation \in 0..2 : \E arg_incarnation \in GrantAuthorityIncarnationValues : Configure(arg_root, arg_namespace, arg_generation, arg_incarnation)
    \/ (phase = "Active") /\ \E actor \in GrantPrincipalValues : \E record \in GrantRecordValues : IssueRoot(actor, record)
    \/ (phase = "Active") /\ \E actor \in GrantPrincipalValues : \E record \in GrantRecordValues : \E derived \in DerivedChildRestrictionsValues : \E chain \in SeqOfGrantRecordValues : \E now_ms \in 0..2 : \E terms_digest \in Sha256DigestValues : IssueChild(actor, record, derived, chain, now_ms, terms_digest)
    \/ (phase = "Active") /\ \E actor \in GrantPrincipalValues : \E record \in GrantRecordValues : RevokeNew(actor, record)
    \/ (phase = "Active") /\ \E actor \in GrantPrincipalValues : \E record \in GrantRecordValues : RevokeAlready(actor, record)
    \/ (phase = "Active") /\ \E arg_namespace \in EvidenceIdValues : \E arg_generation \in 0..2 : \E arg_incarnation \in GrantAuthorityIncarnationValues : \E executor \in GrantPrincipalValues : \E represented_subject \in OptionGrantPrincipalValues : \E requester \in OptionGrantPrincipalValues : \E work \in OptionOriginalWorkRefValues : \E operation \in OptionGrantOperationBindingValues : \E leaf \in GrantRecordValues : \E chain \in SeqOfGrantRecordValues : \E now_ms \in 0..2 : ResolveUse(arg_namespace, arg_generation, arg_incarnation, executor, represented_subject, requester, work, operation, leaf, chain, now_ms)
    \/ (phase = "Active") /\ \E arg_namespace \in EvidenceIdValues : \E arg_generation \in 0..2 : \E arg_incarnation \in GrantAuthorityIncarnationValues : \E executor \in GrantPrincipalValues : \E represented_subject \in OptionGrantPrincipalValues : \E requester \in OptionGrantPrincipalValues : \E work \in OptionOriginalWorkRefValues : \E operation \in OptionGrantOperationBindingValues : \E leaf \in GrantRecordValues : \E chain \in SeqOfGrantRecordValues : \E now_ms \in 0..2 : ConsumeUse(arg_namespace, arg_generation, arg_incarnation, executor, represented_subject, requester, work, operation, leaf, chain, now_ms, TRUE)
    \/ (phase = "Active") /\ \E arg_namespace \in EvidenceIdValues : \E arg_generation \in 0..2 : \E arg_incarnation \in GrantAuthorityIncarnationValues : \E executor \in GrantPrincipalValues : \E represented_subject \in OptionGrantPrincipalValues : \E requester \in OptionGrantPrincipalValues : \E work \in OptionOriginalWorkRefValues : \E operation \in OptionGrantOperationBindingValues : \E leaf \in GrantRecordValues : \E chain \in SeqOfGrantRecordValues : \E now_ms \in 0..2 : ConsumeUseRefusedApproverRevoked(arg_namespace, arg_generation, arg_incarnation, executor, represented_subject, requester, work, operation, leaf, chain, now_ms, FALSE)
    \/ ReleaseCustody
    \/ RecordUncertainAppend
    \/ ReleaseUncertainCustody
    \/ ReleaseCustodyAgain
    \/ TerminalStutter

configured_identity_is_present == (IF (phase = "Unconfigured") THEN TRUE ELSE ((root # None) /\ (namespace # None) /\ (generation > 0) /\ (incarnation # None)))
unconfigured_state_is_empty == (IF (phase # "Unconfigured") THEN TRUE ELSE ((root = None) /\ (namespace = None) /\ (generation = 0) /\ (incarnation = None) /\ (revision = 0) /\ (Cardinality(DOMAIN records) = 0) /\ (Cardinality(revoked) = 0) /\ (Cardinality(DOMAIN spent) = 0) /\ (consumed = 0) /\ (Cardinality(carried_decisions) = 0)))
revision_accounts_for_retained_mutations == ((revision >= Cardinality(DOMAIN records)) /\ ((revision - Cardinality(DOMAIN records)) = (Cardinality(revoked) + consumed)))
issued_records_have_exact_identity_and_revision == (\A id \in DOMAIN records : (((IF "value" \in DOMAIN (IF (id \in DOMAIN records) THEN Some((IF id \in DOMAIN records THEN records[id] ELSE "None")) ELSE None) THEN (IF (id \in DOMAIN records) THEN Some((IF id \in DOMAIN records THEN records[id] ELSE "None")) ELSE None)["value"] ELSE None).id = id) /\ ((IF "value" \in DOMAIN (IF (id \in DOMAIN records) THEN Some((IF id \in DOMAIN records THEN records[id] ELSE "None")) ELSE None) THEN (IF (id \in DOMAIN records) THEN Some((IF id \in DOMAIN records THEN records[id] ELSE "None")) ELSE None)["value"] ELSE None).issued_revision > 0) /\ ((IF "value" \in DOMAIN (IF (id \in DOMAIN records) THEN Some((IF id \in DOMAIN records THEN records[id] ELSE "None")) ELSE None) THEN (IF (id \in DOMAIN records) THEN Some((IF id \in DOMAIN records THEN records[id] ELSE "None")) ELSE None)["value"] ELSE None).issued_revision <= revision)))
issued_records_belong_to_this_incarnation == (\A id \in DOMAIN records : (Some((IF "value" \in DOMAIN (IF (id \in DOMAIN records) THEN Some((IF id \in DOMAIN records THEN records[id] ELSE "None")) ELSE None) THEN (IF (id \in DOMAIN records) THEN Some((IF id \in DOMAIN records THEN records[id] ELSE "None")) ELSE None)["value"] ELSE None).authority_incarnation) = incarnation))
revoked_records_remain_present == (\A id \in revoked : (id \in DOMAIN records))
spent_only_for_approval_children == ((\A id \in DOMAIN spent : ((id \in DOMAIN records) /\ ((IF "value" \in DOMAIN (IF (id \in DOMAIN records) THEN Some((IF id \in DOMAIN records THEN records[id] ELSE "None")) ELSE None) THEN (IF (id \in DOMAIN records) THEN Some((IF id \in DOMAIN records THEN records[id] ELSE "None")) ELSE None)["value"] ELSE None).approval # None) /\ ((IF "value" \in DOMAIN (IF (id \in DOMAIN spent) THEN Some((IF id \in DOMAIN spent THEN spent[id] ELSE 0)) ELSE None) THEN (IF (id \in DOMAIN spent) THEN Some((IF id \in DOMAIN spent THEN spent[id] ELSE 0)) ELSE None)["value"] ELSE None) <= (IF "value" \in DOMAIN (IF "value" \in DOMAIN (IF (id \in DOMAIN records) THEN Some((IF id \in DOMAIN records THEN records[id] ELSE "None")) ELSE None) THEN (IF (id \in DOMAIN records) THEN Some((IF id \in DOMAIN records THEN records[id] ELSE "None")) ELSE None)["value"] ELSE None).approval THEN (IF "value" \in DOMAIN (IF (id \in DOMAIN records) THEN Some((IF id \in DOMAIN records THEN records[id] ELSE "None")) ELSE None) THEN (IF (id \in DOMAIN records) THEN Some((IF id \in DOMAIN records THEN records[id] ELSE "None")) ELSE None)["value"] ELSE None).approval["value"] ELSE None).uses))) /\ (\A id \in DOMAIN records : (IF ((IF "value" \in DOMAIN (IF (id \in DOMAIN records) THEN Some((IF id \in DOMAIN records THEN records[id] ELSE "None")) ELSE None) THEN (IF (id \in DOMAIN records) THEN Some((IF id \in DOMAIN records THEN records[id] ELSE "None")) ELSE None)["value"] ELSE None).approval = None) THEN TRUE ELSE (id \in DOMAIN spent))))
approval_children_are_bounded == (\A id \in DOMAIN records : approval_bounded((IF "value" \in DOMAIN (IF (id \in DOMAIN records) THEN Some((IF id \in DOMAIN records THEN records[id] ELSE "None")) ELSE None) THEN (IF (id \in DOMAIN records) THEN Some((IF id \in DOMAIN records THEN records[id] ELSE "None")) ELSE None)["value"] ELSE None)))
approval_children_have_a_parent_rule == (\A id \in DOMAIN records : (IF ((IF "value" \in DOMAIN (IF (id \in DOMAIN records) THEN Some((IF id \in DOMAIN records THEN records[id] ELSE "None")) ELSE None) THEN (IF (id \in DOMAIN records) THEN Some((IF id \in DOMAIN records THEN records[id] ELSE "None")) ELSE None)["value"] ELSE None).approval = None) THEN TRUE ELSE (((IF "value" \in DOMAIN (IF (id \in DOMAIN records) THEN Some((IF id \in DOMAIN records THEN records[id] ELSE "None")) ELSE None) THEN (IF (id \in DOMAIN records) THEN Some((IF id \in DOMAIN records THEN records[id] ELSE "None")) ELSE None)["value"] ELSE None).parent # None) /\ ((IF "value" \in DOMAIN (IF "value" \in DOMAIN (IF (id \in DOMAIN records) THEN Some((IF id \in DOMAIN records THEN records[id] ELSE "None")) ELSE None) THEN (IF (id \in DOMAIN records) THEN Some((IF id \in DOMAIN records THEN records[id] ELSE "None")) ELSE None)["value"] ELSE None).parent THEN (IF "value" \in DOMAIN (IF (id \in DOMAIN records) THEN Some((IF id \in DOMAIN records THEN records[id] ELSE "None")) ELSE None) THEN (IF (id \in DOMAIN records) THEN Some((IF id \in DOMAIN records THEN records[id] ELSE "None")) ELSE None)["value"] ELSE None).parent["value"] ELSE None) \in DOMAIN records) /\ ((IF "value" \in DOMAIN (IF ((IF "value" \in DOMAIN (IF "value" \in DOMAIN (IF (id \in DOMAIN records) THEN Some((IF id \in DOMAIN records THEN records[id] ELSE "None")) ELSE None) THEN (IF (id \in DOMAIN records) THEN Some((IF id \in DOMAIN records THEN records[id] ELSE "None")) ELSE None)["value"] ELSE None).parent THEN (IF "value" \in DOMAIN (IF (id \in DOMAIN records) THEN Some((IF id \in DOMAIN records THEN records[id] ELSE "None")) ELSE None) THEN (IF (id \in DOMAIN records) THEN Some((IF id \in DOMAIN records THEN records[id] ELSE "None")) ELSE None)["value"] ELSE None).parent["value"] ELSE None) \in DOMAIN records) THEN Some((IF (IF "value" \in DOMAIN (IF "value" \in DOMAIN (IF (id \in DOMAIN records) THEN Some((IF id \in DOMAIN records THEN records[id] ELSE "None")) ELSE None) THEN (IF (id \in DOMAIN records) THEN Some((IF id \in DOMAIN records THEN records[id] ELSE "None")) ELSE None)["value"] ELSE None).parent THEN (IF "value" \in DOMAIN (IF (id \in DOMAIN records) THEN Some((IF id \in DOMAIN records THEN records[id] ELSE "None")) ELSE None) THEN (IF (id \in DOMAIN records) THEN Some((IF id \in DOMAIN records THEN records[id] ELSE "None")) ELSE None)["value"] ELSE None).parent["value"] ELSE None) \in DOMAIN records THEN records[(IF "value" \in DOMAIN (IF "value" \in DOMAIN (IF (id \in DOMAIN records) THEN Some((IF id \in DOMAIN records THEN records[id] ELSE "None")) ELSE None) THEN (IF (id \in DOMAIN records) THEN Some((IF id \in DOMAIN records THEN records[id] ELSE "None")) ELSE None)["value"] ELSE None).parent THEN (IF "value" \in DOMAIN (IF (id \in DOMAIN records) THEN Some((IF id \in DOMAIN records THEN records[id] ELSE "None")) ELSE None) THEN (IF (id \in DOMAIN records) THEN Some((IF id \in DOMAIN records THEN records[id] ELSE "None")) ELSE None)["value"] ELSE None).parent["value"] ELSE None)] ELSE "None")) ELSE None) THEN (IF ((IF "value" \in DOMAIN (IF "value" \in DOMAIN (IF (id \in DOMAIN records) THEN Some((IF id \in DOMAIN records THEN records[id] ELSE "None")) ELSE None) THEN (IF (id \in DOMAIN records) THEN Some((IF id \in DOMAIN records THEN records[id] ELSE "None")) ELSE None)["value"] ELSE None).parent THEN (IF "value" \in DOMAIN (IF (id \in DOMAIN records) THEN Some((IF id \in DOMAIN records THEN records[id] ELSE "None")) ELSE None) THEN (IF (id \in DOMAIN records) THEN Some((IF id \in DOMAIN records THEN records[id] ELSE "None")) ELSE None)["value"] ELSE None).parent["value"] ELSE None) \in DOMAIN records) THEN Some((IF (IF "value" \in DOMAIN (IF "value" \in DOMAIN (IF (id \in DOMAIN records) THEN Some((IF id \in DOMAIN records THEN records[id] ELSE "None")) ELSE None) THEN (IF (id \in DOMAIN records) THEN Some((IF id \in DOMAIN records THEN records[id] ELSE "None")) ELSE None)["value"] ELSE None).parent THEN (IF "value" \in DOMAIN (IF (id \in DOMAIN records) THEN Some((IF id \in DOMAIN records THEN records[id] ELSE "None")) ELSE None) THEN (IF (id \in DOMAIN records) THEN Some((IF id \in DOMAIN records THEN records[id] ELSE "None")) ELSE None)["value"] ELSE None).parent["value"] ELSE None) \in DOMAIN records THEN records[(IF "value" \in DOMAIN (IF "value" \in DOMAIN (IF (id \in DOMAIN records) THEN Some((IF id \in DOMAIN records THEN records[id] ELSE "None")) ELSE None) THEN (IF (id \in DOMAIN records) THEN Some((IF id \in DOMAIN records THEN records[id] ELSE "None")) ELSE None)["value"] ELSE None).parent THEN (IF "value" \in DOMAIN (IF (id \in DOMAIN records) THEN Some((IF id \in DOMAIN records THEN records[id] ELSE "None")) ELSE None) THEN (IF (id \in DOMAIN records) THEN Some((IF id \in DOMAIN records THEN records[id] ELSE "None")) ELSE None)["value"] ELSE None).parent["value"] ELSE None)] ELSE "None")) ELSE None)["value"] ELSE None).child_approval # None))))
carried_decisions_match_records == ((\A decision \in carried_decisions : (\E id \in DOMAIN records : (((IF "value" \in DOMAIN (IF (id \in DOMAIN records) THEN Some((IF id \in DOMAIN records THEN records[id] ELSE "None")) ELSE None) THEN (IF (id \in DOMAIN records) THEN Some((IF id \in DOMAIN records THEN records[id] ELSE "None")) ELSE None)["value"] ELSE None).approval # None) /\ ((IF "value" \in DOMAIN (IF "value" \in DOMAIN (IF (id \in DOMAIN records) THEN Some((IF id \in DOMAIN records THEN records[id] ELSE "None")) ELSE None) THEN (IF (id \in DOMAIN records) THEN Some((IF id \in DOMAIN records THEN records[id] ELSE "None")) ELSE None)["value"] ELSE None).approval THEN (IF "value" \in DOMAIN (IF (id \in DOMAIN records) THEN Some((IF id \in DOMAIN records THEN records[id] ELSE "None")) ELSE None) THEN (IF (id \in DOMAIN records) THEN Some((IF id \in DOMAIN records THEN records[id] ELSE "None")) ELSE None)["value"] ELSE None).approval["value"] ELSE None).decision_id = decision)))) /\ (\A id \in DOMAIN records : (IF ((IF "value" \in DOMAIN (IF (id \in DOMAIN records) THEN Some((IF id \in DOMAIN records THEN records[id] ELSE "None")) ELSE None) THEN (IF (id \in DOMAIN records) THEN Some((IF id \in DOMAIN records THEN records[id] ELSE "None")) ELSE None)["value"] ELSE None).approval = None) THEN TRUE ELSE ((IF "value" \in DOMAIN (IF "value" \in DOMAIN (IF (id \in DOMAIN records) THEN Some((IF id \in DOMAIN records THEN records[id] ELSE "None")) ELSE None) THEN (IF (id \in DOMAIN records) THEN Some((IF id \in DOMAIN records THEN records[id] ELSE "None")) ELSE None)["value"] ELSE None).approval THEN (IF "value" \in DOMAIN (IF (id \in DOMAIN records) THEN Some((IF id \in DOMAIN records THEN records[id] ELSE "None")) ELSE None) THEN (IF (id \in DOMAIN records) THEN Some((IF id \in DOMAIN records THEN records[id] ELSE "None")) ELSE None)["value"] ELSE None).approval["value"] ELSE None).decision_id \in carried_decisions))))

CiStateConstraint == /\ model_step_count <= 6 /\ Cardinality(DOMAIN records) <= 2 /\ Cardinality(revoked) <= 2 /\ Cardinality(DOMAIN spent) <= 2 /\ Cardinality(carried_decisions) <= 2
DeepStateConstraint == /\ model_step_count <= 8 /\ Cardinality(DOMAIN records) <= 2 /\ Cardinality(revoked) <= 2 /\ Cardinality(DOMAIN spent) <= 2 /\ Cardinality(carried_decisions) <= 2

Spec == Init /\ [][Next]_vars

THEOREM Spec => []configured_identity_is_present
THEOREM Spec => []unconfigured_state_is_empty
THEOREM Spec => []revision_accounts_for_retained_mutations
THEOREM Spec => []issued_records_have_exact_identity_and_revision
THEOREM Spec => []issued_records_belong_to_this_incarnation
THEOREM Spec => []revoked_records_remain_present
THEOREM Spec => []spent_only_for_approval_children
THEOREM Spec => []approval_children_are_bounded
THEOREM Spec => []approval_children_have_a_parent_rule
THEOREM Spec => []carried_decisions_match_records

=============================================================================
