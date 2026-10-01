---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for GrantAuthorityMachine.

\* RustU64Max is the TLA boundary for Expr::U64Max; production generated Rust renders u64::MAX.
CONSTANTS DerivedChildRestrictionsValues, EvidenceIdValues, GrantAuthorityIncarnationValues, GrantPrincipalValues, GrantRecordValues, NatValues, SetOfEvidenceIdValues, RustU64Max

None == [tag |-> "none", value |-> "none"]
Some(v) == [tag |-> "some", value |-> v]

DerivedChildRestrictionsValuesCi == {[effective |-> [actions |-> "unrestricted_actions", audiences |-> "unrestricted_audiences", delegation_depth |-> [bound |-> [edges |-> 0, tag |-> "Remaining"], remaining_edges |-> 0, unresolved |-> {}], lifetime |-> [bound |-> [expires_at_ms |-> 2, not_before_ms |-> 1, tag |-> "Window"], expires_at_ms |-> 2, not_before_ms |-> 1, unresolved |-> {}], processors |-> "unrestricted_processors", resource_domains |-> "unrestricted_resources"], parent |-> [actions |-> "unrestricted_actions", audiences |-> "unrestricted_audiences", delegation_depth |-> [bound |-> [edges |-> 1, tag |-> "Remaining"], remaining_edges |-> 1, unresolved |-> {}], lifetime |-> [bound |-> [expires_at_ms |-> 3, not_before_ms |-> 0, tag |-> "Window"], expires_at_ms |-> 3, not_before_ms |-> 0, unresolved |-> {}], processors |-> "unrestricted_processors", resource_domains |-> "unrestricted_resources"], requested |-> [actions |-> "unrestricted_actions", audiences |-> "unrestricted_audiences", delegation_depth |-> [bound |-> [edges |-> 0, tag |-> "Remaining"], remaining_edges |-> 0, unresolved |-> {}], lifetime |-> [bound |-> [expires_at_ms |-> 2, not_before_ms |-> 1, tag |-> "Window"], expires_at_ms |-> 2, not_before_ms |-> 1, unresolved |-> {}], processors |-> "unrestricted_processors", resource_domains |-> "unrestricted_resources"]]}
EvidenceIdValuesCi == {"grant_root", "grant_child"}
GrantAuthorityIncarnationValuesCi == {"owner_incarnation"}
GrantPrincipalValuesCi == {"principal_a", "principal_b"}
GrantRecordValuesCi == {[authority_incarnation |-> "owner_incarnation", grantee |-> "principal_b", id |-> "grant_root", issued_revision |-> 1, issuer |-> "principal_a", parent |-> None, represented_subject |-> None, restrictions |-> [actions |-> "unrestricted_actions", audiences |-> "unrestricted_audiences", delegation_depth |-> [bound |-> [edges |-> 1, tag |-> "Remaining"], remaining_edges |-> 1, unresolved |-> {}], lifetime |-> [bound |-> [expires_at_ms |-> 3, not_before_ms |-> 0, tag |-> "Window"], expires_at_ms |-> 3, not_before_ms |-> 0, unresolved |-> {}], processors |-> "unrestricted_processors", resource_domains |-> "unrestricted_resources"]], [authority_incarnation |-> "owner_incarnation", grantee |-> "principal_a", id |-> "grant_child", issued_revision |-> 2, issuer |-> "principal_b", parent |-> Some("grant_root"), represented_subject |-> None, restrictions |-> [actions |-> "unrestricted_actions", audiences |-> "unrestricted_audiences", delegation_depth |-> [bound |-> [edges |-> 0, tag |-> "Remaining"], remaining_edges |-> 0, unresolved |-> {}], lifetime |-> [bound |-> [expires_at_ms |-> 2, not_before_ms |-> 1, tag |-> "Window"], expires_at_ms |-> 2, not_before_ms |-> 1, unresolved |-> {}], processors |-> "unrestricted_processors", resource_domains |-> "unrestricted_resources"]], [authority_incarnation |-> "owner_incarnation", grantee |-> "principal_a", id |-> "grant_root", issued_revision |-> 1, issuer |-> "principal_b", parent |-> None, represented_subject |-> None, restrictions |-> [actions |-> "unrestricted_actions", audiences |-> "unrestricted_audiences", delegation_depth |-> [bound |-> [edges |-> 1, tag |-> "Remaining"], remaining_edges |-> 1, unresolved |-> {}], lifetime |-> [bound |-> [expires_at_ms |-> 3, not_before_ms |-> 0, tag |-> "Window"], expires_at_ms |-> 3, not_before_ms |-> 0, unresolved |-> {}], processors |-> "unrestricted_processors", resource_domains |-> "unrestricted_resources"]], [authority_incarnation |-> "owner_incarnation", grantee |-> "principal_b", id |-> "grant_child", issued_revision |-> 2, issuer |-> "principal_a", parent |-> Some("grant_root"), represented_subject |-> None, restrictions |-> [actions |-> "unrestricted_actions", audiences |-> "unrestricted_audiences", delegation_depth |-> [bound |-> [edges |-> 0, tag |-> "Remaining"], remaining_edges |-> 0, unresolved |-> {}], lifetime |-> [bound |-> [expires_at_ms |-> 2, not_before_ms |-> 1, tag |-> "Window"], expires_at_ms |-> 2, not_before_ms |-> 1, unresolved |-> {}], processors |-> "unrestricted_processors", resource_domains |-> "unrestricted_resources"]]}

DerivedChildRestrictionsValuesDeep == {[effective |-> [actions |-> "unrestricted_actions", audiences |-> "unrestricted_audiences", delegation_depth |-> [bound |-> [edges |-> 0, tag |-> "Remaining"], remaining_edges |-> 0, unresolved |-> {}], lifetime |-> [bound |-> [expires_at_ms |-> 2, not_before_ms |-> 1, tag |-> "Window"], expires_at_ms |-> 2, not_before_ms |-> 1, unresolved |-> {}], processors |-> "unrestricted_processors", resource_domains |-> "unrestricted_resources"], parent |-> [actions |-> "unrestricted_actions", audiences |-> "unrestricted_audiences", delegation_depth |-> [bound |-> [edges |-> 1, tag |-> "Remaining"], remaining_edges |-> 1, unresolved |-> {}], lifetime |-> [bound |-> [expires_at_ms |-> 3, not_before_ms |-> 0, tag |-> "Window"], expires_at_ms |-> 3, not_before_ms |-> 0, unresolved |-> {}], processors |-> "unrestricted_processors", resource_domains |-> "unrestricted_resources"], requested |-> [actions |-> "unrestricted_actions", audiences |-> "unrestricted_audiences", delegation_depth |-> [bound |-> [edges |-> 0, tag |-> "Remaining"], remaining_edges |-> 0, unresolved |-> {}], lifetime |-> [bound |-> [expires_at_ms |-> 2, not_before_ms |-> 1, tag |-> "Window"], expires_at_ms |-> 2, not_before_ms |-> 1, unresolved |-> {}], processors |-> "unrestricted_processors", resource_domains |-> "unrestricted_resources"]]}
EvidenceIdValuesDeep == {"grant_root", "grant_child"}
GrantAuthorityIncarnationValuesDeep == {"owner_incarnation"}
GrantPrincipalValuesDeep == {"principal_a", "principal_b"}
GrantRecordValuesDeep == {[authority_incarnation |-> "owner_incarnation", grantee |-> "principal_b", id |-> "grant_root", issued_revision |-> 1, issuer |-> "principal_a", parent |-> None, represented_subject |-> None, restrictions |-> [actions |-> "unrestricted_actions", audiences |-> "unrestricted_audiences", delegation_depth |-> [bound |-> [edges |-> 1, tag |-> "Remaining"], remaining_edges |-> 1, unresolved |-> {}], lifetime |-> [bound |-> [expires_at_ms |-> 3, not_before_ms |-> 0, tag |-> "Window"], expires_at_ms |-> 3, not_before_ms |-> 0, unresolved |-> {}], processors |-> "unrestricted_processors", resource_domains |-> "unrestricted_resources"]], [authority_incarnation |-> "owner_incarnation", grantee |-> "principal_a", id |-> "grant_child", issued_revision |-> 2, issuer |-> "principal_b", parent |-> Some("grant_root"), represented_subject |-> None, restrictions |-> [actions |-> "unrestricted_actions", audiences |-> "unrestricted_audiences", delegation_depth |-> [bound |-> [edges |-> 0, tag |-> "Remaining"], remaining_edges |-> 0, unresolved |-> {}], lifetime |-> [bound |-> [expires_at_ms |-> 2, not_before_ms |-> 1, tag |-> "Window"], expires_at_ms |-> 2, not_before_ms |-> 1, unresolved |-> {}], processors |-> "unrestricted_processors", resource_domains |-> "unrestricted_resources"]], [authority_incarnation |-> "owner_incarnation", grantee |-> "principal_a", id |-> "grant_root", issued_revision |-> 1, issuer |-> "principal_b", parent |-> None, represented_subject |-> None, restrictions |-> [actions |-> "unrestricted_actions", audiences |-> "unrestricted_audiences", delegation_depth |-> [bound |-> [edges |-> 1, tag |-> "Remaining"], remaining_edges |-> 1, unresolved |-> {}], lifetime |-> [bound |-> [expires_at_ms |-> 3, not_before_ms |-> 0, tag |-> "Window"], expires_at_ms |-> 3, not_before_ms |-> 0, unresolved |-> {}], processors |-> "unrestricted_processors", resource_domains |-> "unrestricted_resources"]], [authority_incarnation |-> "owner_incarnation", grantee |-> "principal_b", id |-> "grant_child", issued_revision |-> 2, issuer |-> "principal_a", parent |-> Some("grant_root"), represented_subject |-> None, restrictions |-> [actions |-> "unrestricted_actions", audiences |-> "unrestricted_audiences", delegation_depth |-> [bound |-> [edges |-> 0, tag |-> "Remaining"], remaining_edges |-> 0, unresolved |-> {}], lifetime |-> [bound |-> [expires_at_ms |-> 2, not_before_ms |-> 1, tag |-> "Window"], expires_at_ms |-> 2, not_before_ms |-> 1, unresolved |-> {}], processors |-> "unrestricted_processors", resource_domains |-> "unrestricted_resources"]], [authority_incarnation |-> "owner_incarnation", grantee |-> "principal_b", id |-> "grant_root", issued_revision |-> 1, issuer |-> "principal_a", parent |-> None, represented_subject |-> None, restrictions |-> [actions |-> "unrestricted_actions", audiences |-> "unrestricted_audiences", delegation_depth |-> [bound |-> [edges |-> 1, tag |-> "Remaining"], remaining_edges |-> 1, unresolved |-> {}], lifetime |-> [bound |-> [tag |-> "Unrestricted"], expires_at_ms |-> RustU64Max, not_before_ms |-> 0, unresolved |-> {"Unknown"}], processors |-> "unrestricted_processors", resource_domains |-> "unrestricted_resources"]], [authority_incarnation |-> "foreign_incarnation", grantee |-> "principal_b", id |-> "grant_root", issued_revision |-> 1, issuer |-> "principal_a", parent |-> None, represented_subject |-> None, restrictions |-> [actions |-> "unrestricted_actions", audiences |-> "unrestricted_audiences", delegation_depth |-> [bound |-> [edges |-> 1, tag |-> "Remaining"], remaining_edges |-> 1, unresolved |-> {}], lifetime |-> [bound |-> [expires_at_ms |-> 3, not_before_ms |-> 0, tag |-> "Window"], expires_at_ms |-> 3, not_before_ms |-> 0, unresolved |-> {}], processors |-> "unrestricted_processors", resource_domains |-> "unrestricted_resources"]]}

MapEvidenceIdGrantRecordValues == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in EvidenceIdValues, v \in GrantRecordValues }
OptionEvidenceIdValues == {None} \cup {Some(x) : x \in EvidenceIdValues}
OptionGrantAuthorityIncarnationValues == {None} \cup {Some(x) : x \in GrantAuthorityIncarnationValues}
OptionGrantPrincipalValues == {None} \cup {Some(x) : x \in GrantPrincipalValues}
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

VARIABLES phase, model_step_count, root, namespace, generation, incarnation, revision, records, revoked

vars == << phase, model_step_count, root, namespace, generation, incarnation, revision, records, revoked >>

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

\* Named UNCHANGED frames. One definition per distinct frame; every action
\* that leaves those variables unchanged references the definition by name.
UnchangedFrame_00c7c6781b869e99 == UNCHANGED << revision, records, revoked >>
UnchangedFrame_015cc440bac5e227 == UNCHANGED << root, namespace, generation, incarnation, records >>
UnchangedFrame_44d4642097625110 == UNCHANGED << root, namespace, generation, incarnation, revision, records, revoked >>
UnchangedFrame_74d70b51478fdc0f == UNCHANGED << root, namespace, generation, incarnation, revoked >>

Configure(arg_root, arg_namespace, arg_generation, arg_incarnation) ==
    /\ phase = "Unconfigured"
    /\ (arg_generation > 0)
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ root' = Some(arg_root)
    /\ namespace' = Some(arg_namespace)
    /\ generation' = arg_generation
    /\ incarnation' = Some(arg_incarnation)
    /\ UnchangedFrame_00c7c6781b869e99


IssueRoot(actor, record) ==
    /\ phase = "Active"
    /\ (Some(actor) = root)
    /\ (Some(record.authority_incarnation) = incarnation)
    /\ ((record.issuer = actor) /\ (record.parent = None))
    /\ (((record.id \in DOMAIN records) = FALSE) /\ (revision < RustU64Max))
    /\ (record.issued_revision = (revision + 1))
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ records' = MapSet(records, record.id, record)
    /\ UnchangedFrame_74d70b51478fdc0f


IssueChild(actor, record, derived, chain, now_ms) ==
    /\ phase = "Active"
    /\ (revision < RustU64Max)
    /\ (Some(record.authority_incarnation) = incarnation)
    /\ (((record.id \in DOMAIN records) = FALSE) /\ (record.issued_revision = (revision + 1)))
    /\ ((64 > Len(chain)) /\ chain_links(chain, root, now_ms))
    /\ (\A link \in SeqElements(chain) : (((IF (link.id \in DOMAIN records) THEN Some((IF link.id \in DOMAIN records THEN records[link.id] ELSE "None")) ELSE None) = Some(link)) /\ ((link.id \in revoked) = FALSE)))
    /\ ((record.issuer = actor) /\ (record.restrictions = derived.effective) /\ (\E parent \in SeqElements(chain) : ((record.parent = Some(parent.id)) /\ (parent.grantee = actor) /\ (record.represented_subject = parent.represented_subject) /\ (derived.parent = parent.restrictions) /\ child_rank(parent.restrictions, record.restrictions))))
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ records' = MapSet(records, record.id, record)
    /\ UnchangedFrame_74d70b51478fdc0f


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
    /\ UnchangedFrame_015cc440bac5e227


RevokeAlready(actor, record) ==
    /\ phase = "Active"
    /\ ((IF (record.id \in DOMAIN records) THEN Some((IF record.id \in DOMAIN records THEN records[record.id] ELSE "None")) ELSE None) = Some(record))
    /\ (IF (Some(actor) = root) THEN TRUE ELSE (actor = record.issuer))
    /\ (record.id \in revoked)
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44d4642097625110


ResolveUse(arg_namespace, arg_generation, arg_incarnation, executor, represented_subject, leaf, chain, now_ms) ==
    /\ phase = "Active"
    /\ ((Some(arg_namespace) = namespace) /\ (arg_generation = generation) /\ (Some(arg_incarnation) = incarnation))
    /\ ((leaf.grantee = executor) /\ (leaf.represented_subject = represented_subject) /\ (leaf \in SeqElements(chain)))
    /\ chain_links(chain, root, now_ms)
    /\ (\A link \in SeqElements(chain) : (((IF (link.id \in DOMAIN records) THEN Some((IF link.id \in DOMAIN records THEN records[link.id] ELSE "None")) ELSE None) = Some(link)) /\ ((link.id \in revoked) = FALSE)))
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_44d4642097625110


Next ==
    \/ \E arg_root \in GrantPrincipalValues : \E arg_namespace \in EvidenceIdValues : \E arg_generation \in 0..2 : \E arg_incarnation \in GrantAuthorityIncarnationValues : Configure(arg_root, arg_namespace, arg_generation, arg_incarnation)
    \/ \E actor \in GrantPrincipalValues : \E record \in GrantRecordValues : IssueRoot(actor, record)
    \/ \E actor \in GrantPrincipalValues : \E record \in GrantRecordValues : \E derived \in DerivedChildRestrictionsValues : \E chain \in SeqOfGrantRecordValues : \E now_ms \in 0..2 : IssueChild(actor, record, derived, chain, now_ms)
    \/ \E actor \in GrantPrincipalValues : \E record \in GrantRecordValues : RevokeNew(actor, record)
    \/ \E actor \in GrantPrincipalValues : \E record \in GrantRecordValues : RevokeAlready(actor, record)
    \/ \E arg_namespace \in EvidenceIdValues : \E arg_generation \in 0..2 : \E arg_incarnation \in GrantAuthorityIncarnationValues : \E executor \in GrantPrincipalValues : \E represented_subject \in OptionGrantPrincipalValues : \E leaf \in GrantRecordValues : \E chain \in SeqOfGrantRecordValues : \E now_ms \in 0..2 : ResolveUse(arg_namespace, arg_generation, arg_incarnation, executor, represented_subject, leaf, chain, now_ms)

configured_identity_is_present == (IF (phase = "Unconfigured") THEN TRUE ELSE ((root # None) /\ (namespace # None) /\ (generation > 0) /\ (incarnation # None)))
unconfigured_state_is_empty == (IF (phase # "Unconfigured") THEN TRUE ELSE ((root = None) /\ (namespace = None) /\ (generation = 0) /\ (incarnation = None) /\ (revision = 0) /\ (Cardinality(DOMAIN records) = 0) /\ (Cardinality(revoked) = 0)))
revision_accounts_for_retained_mutations == ((revision >= Cardinality(DOMAIN records)) /\ ((revision - Cardinality(DOMAIN records)) = Cardinality(revoked)))
issued_records_have_exact_identity_and_revision == (\A id \in DOMAIN records : (((IF "value" \in DOMAIN (IF (id \in DOMAIN records) THEN Some((IF id \in DOMAIN records THEN records[id] ELSE "None")) ELSE None) THEN (IF (id \in DOMAIN records) THEN Some((IF id \in DOMAIN records THEN records[id] ELSE "None")) ELSE None)["value"] ELSE None).id = id) /\ ((IF "value" \in DOMAIN (IF (id \in DOMAIN records) THEN Some((IF id \in DOMAIN records THEN records[id] ELSE "None")) ELSE None) THEN (IF (id \in DOMAIN records) THEN Some((IF id \in DOMAIN records THEN records[id] ELSE "None")) ELSE None)["value"] ELSE None).issued_revision > 0) /\ ((IF "value" \in DOMAIN (IF (id \in DOMAIN records) THEN Some((IF id \in DOMAIN records THEN records[id] ELSE "None")) ELSE None) THEN (IF (id \in DOMAIN records) THEN Some((IF id \in DOMAIN records THEN records[id] ELSE "None")) ELSE None)["value"] ELSE None).issued_revision <= revision)))
issued_records_belong_to_this_incarnation == (\A id \in DOMAIN records : (Some((IF "value" \in DOMAIN (IF (id \in DOMAIN records) THEN Some((IF id \in DOMAIN records THEN records[id] ELSE "None")) ELSE None) THEN (IF (id \in DOMAIN records) THEN Some((IF id \in DOMAIN records THEN records[id] ELSE "None")) ELSE None)["value"] ELSE None).authority_incarnation) = incarnation))
revoked_records_remain_present == (\A id \in revoked : (id \in DOMAIN records))

CiStateConstraint == /\ model_step_count <= 6 /\ Cardinality(DOMAIN records) <= 2 /\ Cardinality(revoked) <= 2
DeepStateConstraint == /\ model_step_count <= 8 /\ Cardinality(DOMAIN records) <= 2 /\ Cardinality(revoked) <= 2

Spec == Init /\ [][Next]_vars

THEOREM Spec => []configured_identity_is_present
THEOREM Spec => []unconfigured_state_is_empty
THEOREM Spec => []revision_accounts_for_retained_mutations
THEOREM Spec => []issued_records_have_exact_identity_and_revision
THEOREM Spec => []issued_records_belong_to_this_incarnation
THEOREM Spec => []revoked_records_remain_present

=============================================================================
