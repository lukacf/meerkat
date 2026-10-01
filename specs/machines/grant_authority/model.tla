---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for GrantAuthorityMachine.

\* RustU64Max is the TLA boundary for Expr::U64Max; production generated Rust renders u64::MAX.
CONSTANTS DerivedChildRestrictionsValues, EvidenceIdValues, GrantPrincipalValues, GrantRecordValues, NatValues, SetOfEvidenceIdValues, RustU64Max

None == [tag |-> "none", value |-> "none"]
Some(v) == [tag |-> "some", value |-> v]

DerivedChildRestrictionsValuesCi == {}
GrantRecordValuesCi == {}

DerivedChildRestrictionsValuesDeep == {[parent |-> [actions |-> "grantactionbounds_1", resource_domains |-> "grantresourcebounds_1", processors |-> "grantprocessorbounds_1", audiences |-> "grantaudiencebounds_1", lifetime |-> [bound |-> [tag |-> "Unrestricted"], unresolved |-> {"Absent"}, not_before_ms |-> 1, expires_at_ms |-> 1], delegation_depth |-> [bound |-> [tag |-> "Unrestricted"], unresolved |-> {"Absent"}, remaining_edges |-> 1]], requested |-> [actions |-> "grantactionbounds_1", resource_domains |-> "grantresourcebounds_1", processors |-> "grantprocessorbounds_1", audiences |-> "grantaudiencebounds_1", lifetime |-> [bound |-> [tag |-> "Unrestricted"], unresolved |-> {"Absent"}, not_before_ms |-> 1, expires_at_ms |-> 1], delegation_depth |-> [bound |-> [tag |-> "Unrestricted"], unresolved |-> {"Absent"}, remaining_edges |-> 1]], effective |-> [actions |-> "grantactionbounds_1", resource_domains |-> "grantresourcebounds_1", processors |-> "grantprocessorbounds_1", audiences |-> "grantaudiencebounds_1", lifetime |-> [bound |-> [tag |-> "Unrestricted"], unresolved |-> {"Absent"}, not_before_ms |-> 1, expires_at_ms |-> 1], delegation_depth |-> [bound |-> [tag |-> "Unrestricted"], unresolved |-> {"Absent"}, remaining_edges |-> 1]]], [parent |-> [actions |-> "grantactionbounds_2", resource_domains |-> "grantresourcebounds_2", processors |-> "grantprocessorbounds_2", audiences |-> "grantaudiencebounds_2", lifetime |-> [bound |-> [tag |-> "Empty"], unresolved |-> {"Unknown"}, not_before_ms |-> 2, expires_at_ms |-> 2], delegation_depth |-> [bound |-> [tag |-> "Remaining", edges |-> 1], unresolved |-> {"Unknown"}, remaining_edges |-> 2]], requested |-> [actions |-> "grantactionbounds_2", resource_domains |-> "grantresourcebounds_2", processors |-> "grantprocessorbounds_2", audiences |-> "grantaudiencebounds_2", lifetime |-> [bound |-> [tag |-> "Empty"], unresolved |-> {"Unknown"}, not_before_ms |-> 2, expires_at_ms |-> 2], delegation_depth |-> [bound |-> [tag |-> "Remaining", edges |-> 1], unresolved |-> {"Unknown"}, remaining_edges |-> 2]], effective |-> [actions |-> "grantactionbounds_2", resource_domains |-> "grantresourcebounds_2", processors |-> "grantprocessorbounds_2", audiences |-> "grantaudiencebounds_2", lifetime |-> [bound |-> [tag |-> "Empty"], unresolved |-> {"Unknown"}, not_before_ms |-> 2, expires_at_ms |-> 2], delegation_depth |-> [bound |-> [tag |-> "Remaining", edges |-> 1], unresolved |-> {"Unknown"}, remaining_edges |-> 2]]]}
GrantRecordValuesDeep == {[id |-> "evidenceid_1", parent |-> None, issuer |-> "grantprincipal_1", grantee |-> "grantprincipal_1", represented_subject |-> None, issued_revision |-> 1, restrictions |-> [actions |-> "grantactionbounds_1", resource_domains |-> "grantresourcebounds_1", processors |-> "grantprocessorbounds_1", audiences |-> "grantaudiencebounds_1", lifetime |-> [bound |-> [tag |-> "Unrestricted"], unresolved |-> {"Absent"}, not_before_ms |-> 1, expires_at_ms |-> 1], delegation_depth |-> [bound |-> [tag |-> "Unrestricted"], unresolved |-> {"Absent"}, remaining_edges |-> 1]]], [id |-> "evidenceid_2", parent |-> Some("evidenceid_1"), issuer |-> "grantprincipal_2", grantee |-> "grantprincipal_2", represented_subject |-> Some("grantprincipal_1"), issued_revision |-> 2, restrictions |-> [actions |-> "grantactionbounds_2", resource_domains |-> "grantresourcebounds_2", processors |-> "grantprocessorbounds_2", audiences |-> "grantaudiencebounds_2", lifetime |-> [bound |-> [tag |-> "Empty"], unresolved |-> {"Unknown"}, not_before_ms |-> 2, expires_at_ms |-> 2], delegation_depth |-> [bound |-> [tag |-> "Remaining", edges |-> 1], unresolved |-> {"Unknown"}, remaining_edges |-> 2]]]}

MapEvidenceIdGrantRecordValues == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in EvidenceIdValues, v \in GrantRecordValues }
OptionEvidenceIdValues == {None} \cup {Some(x) : x \in EvidenceIdValues}
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

VARIABLES phase, model_step_count, root, namespace, generation, revision, records, revoked

vars == << phase, model_step_count, root, namespace, generation, revision, records, revoked >>

child_rank(parent, child) == (IF (parent.delegation_depth.bound.tag = "Unrestricted") THEN TRUE ELSE ((parent.delegation_depth.bound.tag = "Remaining") /\ (child.delegation_depth.bound.tag = "Remaining") /\ (child.delegation_depth.remaining_edges < parent.delegation_depth.remaining_edges)))
lifetime_current(restrictions, now_ms) == ((Len(restrictions.lifetime.unresolved) = 0) /\ (Len(restrictions.delegation_depth.unresolved) = 0) /\ (IF (restrictions.lifetime.bound.tag = "Unrestricted") THEN TRUE ELSE ((restrictions.lifetime.bound.tag = "Window") /\ (restrictions.lifetime.not_before_ms <= now_ms) /\ (now_ms < restrictions.lifetime.expires_at_ms))))
chain_links(chain, arg_root, now_ms) == ((Len(chain) > 0) /\ (Len(chain) <= 64) /\ (\A link \in SeqElements(chain) : (lifetime_current(link.restrictions, now_ms) /\ (IF (link.parent = None) THEN (Some(link.issuer) = arg_root) ELSE (\E parent \in SeqElements(chain) : ((link.parent = Some(parent.id)) /\ (link.issuer = parent.grantee) /\ (link.represented_subject = parent.represented_subject) /\ (parent.issued_revision < link.issued_revision) /\ child_rank(parent.restrictions, link.restrictions)))))))

Init ==
    /\ phase = "Unconfigured"
    /\ model_step_count = 0
    /\ root = None
    /\ namespace = None
    /\ generation = 0
    /\ revision = 0
    /\ records = [x \in {} |-> None]
    /\ revoked = {}

\* Named UNCHANGED frames. One definition per distinct frame; every action
\* that leaves those variables unchanged references the definition by name.
UnchangedFrame_00c7c6781b869e99 == UNCHANGED << revision, records, revoked >>
UnchangedFrame_0feea0d3f95c4d9e == UNCHANGED << root, namespace, generation, revision, records, revoked >>
UnchangedFrame_6f79c3b2ec7cb7d1 == UNCHANGED << root, namespace, generation, revoked >>
UnchangedFrame_b0f37960469bf115 == UNCHANGED << root, namespace, generation, records >>

Configure(arg_root, arg_namespace, arg_generation) ==
    /\ phase = "Unconfigured"
    /\ (arg_generation > 0)
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ root' = Some(arg_root)
    /\ namespace' = Some(arg_namespace)
    /\ generation' = arg_generation
    /\ UnchangedFrame_00c7c6781b869e99


IssueRoot(actor, record) ==
    /\ phase = "Active"
    /\ (Some(actor) = root)
    /\ ((record.issuer = actor) /\ (record.parent = None))
    /\ (((record.id \in DOMAIN records) = FALSE) /\ (revision < RustU64Max))
    /\ (record.issued_revision = (revision + 1))
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ records' = MapSet(records, record.id, record)
    /\ UnchangedFrame_6f79c3b2ec7cb7d1


IssueChild(actor, record, derived, chain, now_ms) ==
    /\ phase = "Active"
    /\ (revision < RustU64Max)
    /\ (((record.id \in DOMAIN records) = FALSE) /\ (record.issued_revision = (revision + 1)))
    /\ ((64 > Len(chain)) /\ chain_links(chain, root, now_ms))
    /\ (\A link \in SeqElements(chain) : (((IF (link.id \in DOMAIN records) THEN Some((IF link.id \in DOMAIN records THEN records[link.id] ELSE "None")) ELSE None) = Some(link)) /\ ((link.id \in revoked) = FALSE)))
    /\ ((record.issuer = actor) /\ (record.restrictions = derived.effective) /\ (\E parent \in SeqElements(chain) : ((record.parent = Some(parent.id)) /\ (parent.grantee = actor) /\ (record.represented_subject = parent.represented_subject) /\ (derived.parent = parent.restrictions) /\ child_rank(parent.restrictions, record.restrictions))))
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ revision' = (revision) + 1
    /\ records' = MapSet(records, record.id, record)
    /\ UnchangedFrame_6f79c3b2ec7cb7d1


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
    /\ UnchangedFrame_b0f37960469bf115


RevokeAlready(actor, record) ==
    /\ phase = "Active"
    /\ ((IF (record.id \in DOMAIN records) THEN Some((IF record.id \in DOMAIN records THEN records[record.id] ELSE "None")) ELSE None) = Some(record))
    /\ (IF (Some(actor) = root) THEN TRUE ELSE (actor = record.issuer))
    /\ (record.id \in revoked)
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0feea0d3f95c4d9e


ResolveUse(arg_namespace, arg_generation, executor, represented_subject, leaf, chain, now_ms) ==
    /\ phase = "Active"
    /\ ((Some(arg_namespace) = namespace) /\ (arg_generation = generation))
    /\ ((leaf.grantee = executor) /\ (leaf.represented_subject = represented_subject) /\ (leaf \in SeqElements(chain)))
    /\ chain_links(chain, root, now_ms)
    /\ (\A link \in SeqElements(chain) : (((IF (link.id \in DOMAIN records) THEN Some((IF link.id \in DOMAIN records THEN records[link.id] ELSE "None")) ELSE None) = Some(link)) /\ ((link.id \in revoked) = FALSE)))
    /\ phase' = "Active"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_0feea0d3f95c4d9e


Next ==
    \/ \E arg_root \in GrantPrincipalValues : \E arg_namespace \in EvidenceIdValues : \E arg_generation \in 0..2 : Configure(arg_root, arg_namespace, arg_generation)
    \/ \E actor \in GrantPrincipalValues : \E record \in GrantRecordValues : IssueRoot(actor, record)
    \/ \E actor \in GrantPrincipalValues : \E record \in GrantRecordValues : \E derived \in DerivedChildRestrictionsValues : \E chain \in SeqOfGrantRecordValues : \E now_ms \in 0..2 : IssueChild(actor, record, derived, chain, now_ms)
    \/ \E actor \in GrantPrincipalValues : \E record \in GrantRecordValues : RevokeNew(actor, record)
    \/ \E actor \in GrantPrincipalValues : \E record \in GrantRecordValues : RevokeAlready(actor, record)
    \/ \E arg_namespace \in EvidenceIdValues : \E arg_generation \in 0..2 : \E executor \in GrantPrincipalValues : \E represented_subject \in OptionGrantPrincipalValues : \E leaf \in GrantRecordValues : \E chain \in SeqOfGrantRecordValues : \E now_ms \in 0..2 : ResolveUse(arg_namespace, arg_generation, executor, represented_subject, leaf, chain, now_ms)

configured_identity_is_present == (IF (phase = "Unconfigured") THEN TRUE ELSE ((root # None) /\ (namespace # None) /\ (generation > 0)))
revoked_records_remain_present == (\A id \in revoked : (id \in DOMAIN records))

CiStateConstraint == /\ model_step_count <= 4 /\ Cardinality(DOMAIN records) <= 1 /\ Cardinality(revoked) <= 1
DeepStateConstraint == /\ model_step_count <= 8 /\ Cardinality(DOMAIN records) <= 2 /\ Cardinality(revoked) <= 2

Spec == Init /\ [][Next]_vars

THEOREM Spec => []configured_identity_is_present
THEOREM Spec => []revoked_records_remain_present

=============================================================================
