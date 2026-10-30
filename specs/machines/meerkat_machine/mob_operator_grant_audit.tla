---- MODULE mob_operator_grant_audit ----
\* Hand-written bounded audit of incarnation-bound mob operator grants.
\*
\* Mob operator scope names a mob id and the incarnation the mob was created
\* at, which its restarts keep (`mob_operator_mob_incarnations`), so a grant
\* for a destroyed mob never reaches a later mob created under the same id.
\* There are three scopes: manage scope, spawn-profile scope and identity
\* creation scope (`mob_operator_identity_creation_scope`, the templates an
\* agent creator may create identities from). A grant for a bound id at
\* another incarnation rebinds it and drops the two other scopes held for the
\* old incarnation; a grant at the bound incarnation keeps them. A restore
\* refuses a scope that names an unbound mob. Resolving the create authority,
\* present or absent, clears every scope and binding. The identity creation
\* check (`ResolveMobOperatorIdentityCreation`, three arms) admits exactly the
\* templates the creation scope holds for a mob at the incarnation the mob is
\* bound to.
\*
\* This audit keeps the generated MeerkatMachine model unchanged and
\* explores, for two mob ids, two incarnations, one spawn profile and one
\* granted template: the create authority resolved present and absent, every
\* bind and rebind arm of the manage and spawn grants for both mobs and of
\* the creation grant for the first, and, from the initial state, a restore
\* with every binding map over the two ids and incarnations and every manage
\* and spawn scope over them, and a restore of the first mob's creation scope
\* alone, bound to either incarnation or unbound (the restore guard disables
\* the restores that scope an unbound mob). A restore writes every mob
\* operator fact, so restoring from the initial state reaches every restored
\* state. Only the first mob ever holds creation scope, so the second, with
\* manage and spawn scope at most, is where the check must never read other
\* scope as creation scope. In every reachable state the audit reads the
\* check's arms (ENABLED) for each mob, incarnation and template, the granted
\* one and one that no grant names. Properties:
\*
\*   mob_operator_scopes_are_bound_to_an_incarnation (generated invariant,
\*     kept from ci.cfg): every scoped mob id is bound.
\*   AuditGrantBindsTheNamedIncarnation (action property): a grant for mob m
\*     at incarnation i puts m in that grant's scope, bound to i, and leaves
\*     every other mob's binding as it was.
\*   AuditRebindDropsTheOtherScope (action property): a grant that rebinds m
\*     drops the two other scopes m held.
\*   AuditBoundIncarnationKeepsTheOtherScope (action property): a grant for
\*     an unbound m, or at the incarnation m is bound to, leaves the two
\*     other scopes as they were.
\*   AuditResolveClearsScope (action property): resolving the create
\*     authority, present or absent, leaves no scope and no binding.
\*   AuditCreationCheckIsTotal (invariant): exactly one arm of the creation
\*     check answers each mob, incarnation and template.
\*   AuditCreationAdmittedOnlyInScope (invariant): the check admits exactly
\*     when the authority is present, the mob is bound to the incarnation and
\*     its creation scope holds the template; manage and spawn-profile scope
\*     never admit a creation.
\*
\* AuditWitness* operators are reachability goals the runner checks as
\* expected violations, so the properties are not vacuous. With --mutants the
\* runner also checks that five seeded model defects are refused: a manage
\* rebind that keeps the stale spawn scope, a restore without its bound-scope
\* guard, a manage grant that records no binding, a manage rebind that keeps
\* the stale creation scope, and a creation check that admits without the
\* creation scope.
\*
\* The TLC config is DERIVED from the generated ci.cfg on every run by
\* mob_operator_grant_audit.sh. Run a deeper bound by hand with:
\*   specs/machines/meerkat_machine/mob_operator_grant_audit.sh 7
EXTENDS model

CONSTANT AuditMaxSteps

VARIABLES
    audit_rebound,                      \* a grant moved a bound mob to another incarnation
    audit_rebind_dropped_spawn,         \* a manage rebind dropped spawn scope the mob had
    audit_rebind_dropped_manage,        \* a spawn rebind dropped manage scope the mob had
    audit_restored_bound,               \* a restore landed a non-empty binding map
    audit_cleared_after_grant,          \* a resolve cleared scope a grant had given
    audit_rebind_dropped_creation,      \* a manage or spawn rebind dropped creation scope the mob had
    audit_creation_rebind_dropped_other \* a creation rebind dropped manage or spawn scope the mob had

auditVars == <<audit_rebound, audit_rebind_dropped_spawn,
               audit_rebind_dropped_manage, audit_restored_bound,
               audit_cleared_after_grant, audit_rebind_dropped_creation,
               audit_creation_rebind_dropped_other>>

AuditMobs == {"mobid_1", "mobid_2"}
\* The mob that is granted and restored identity creation scope.
AuditCreationMob == "mobid_1"
AuditIncarnations == MobIncarnationIdValues
AuditProfileSets == {{"profile_1"}}
AuditTemplateSets == {{"template_1"}}
\* The granted template and one that no grant names.
AuditTemplates == {"template_1", "template_2"}
AuditPrincipal == "opaqueprincipaltoken_1"

Bound(m) == m \in DOMAIN mob_operator_mob_incarnations
BoundTo(m, i) == Bound(m) /\ mob_operator_mob_incarnations[m] = i
BoundNext(m, i) ==
    /\ m \in DOMAIN mob_operator_mob_incarnations'
    /\ mob_operator_mob_incarnations'[m] = i
HasCreationScope(m) == m \in DOMAIN mob_operator_identity_creation_scope
HasScope ==
    \/ mob_operator_managed_mob_scope # {}
    \/ DOMAIN mob_operator_spawn_profile_scope # {}
    \/ DOMAIN mob_operator_identity_creation_scope # {}

ManageGrant(m, i) == GrantMobOperatorManageMob(m, i) \/ RebindMobOperatorManageMob(m, i)
SpawnBind(m, i) == \E ps \in AuditProfileSets : SetMobOperatorSpawnProfilesInMob(m, i, ps)
SpawnRebind(m, i) == \E ps \in AuditProfileSets : RebindMobOperatorSpawnProfilesInMob(m, i, ps)
SpawnGrant(m, i) == SpawnBind(m, i) \/ SpawnRebind(m, i)
CreationBind(m, i) ==
    \E ts \in AuditTemplateSets : SetMobOperatorIdentityCreationInMob(m, i, ts)
CreationRebind(m, i) ==
    \E ts \in AuditTemplateSets : RebindMobOperatorIdentityCreationInMob(m, i, ts)
CreationGrant(m, i) == CreationBind(m, i) \/ CreationRebind(m, i)

\* The three arms of the identity creation check.
CreationAdmitted(m, i, t) == ResolveMobOperatorIdentityCreationAdmitted(m, i, t)
CreationUnbound(m, i, t) == ResolveMobOperatorIdentityCreationUnbound(m, i, t)
CreationNotInScope(m, i, t) == ResolveMobOperatorIdentityCreationTemplateNotInScope(m, i, t)

\* What the check must admit: the authority is present, m is bound to i and
\* its creation scope holds t.
InCreationScope(m, i, t) ==
    /\ mob_operator_authority_present
    /\ BoundTo(m, i)
    /\ HasCreationScope(m)
    /\ t \in mob_operator_identity_creation_scope[m]

AuditResolve ==
    \/ ResolveMobOperatorCreateAuthority("Enable", AuditPrincipal, None, None)
    \/ ResolveMobOperatorCreateAuthorityAbsent("Disable", AuditPrincipal, None, None)

\* Every binding map over the audit mobs and incarnations.
AuditBindingMaps == UNION {[d -> AuditIncarnations] : d \in SUBSET AuditMobs}
NoScope == [m \in {} |-> {}]

\* A restore writes every mob operator fact, so the state it lands in does
\* not depend on the state it starts from: restoring from the initial state
\* alone reaches every restored state, and a restore at a later step would
\* only add copies of them at other step counts.
AuditRestore ==
    /\ model_step_count = 0
    /\ \/ \E managed \in SUBSET AuditMobs, spawned \in SUBSET AuditMobs,
             bindings \in AuditBindingMaps :
              RestoreMobOperatorAuthority(AuditPrincipal, TRUE, TRUE, managed,
                  [m \in spawned |-> {"profile_1"}], NoScope, bindings, None, None)
       \* The creation scope of the first mob alone, bound to either
       \* incarnation or to none (which the restore guard refuses).
       \/ \E bindings \in {[m \in {} |-> None]}
                          \cup {[m \in {AuditCreationMob} |-> i] : i \in AuditIncarnations} :
              RestoreMobOperatorAuthority(AuditPrincipal, TRUE, TRUE, {}, NoScope,
                  [m \in {AuditCreationMob} |-> {"template_1"}], bindings, None, None)

KeepHistory == UNCHANGED auditVars

AuditNext ==
    \/ AuditResolve
       /\ audit_cleared_after_grant' = (audit_cleared_after_grant \/ HasScope)
       /\ UNCHANGED <<audit_rebound, audit_rebind_dropped_spawn,
                      audit_rebind_dropped_manage, audit_restored_bound,
                      audit_rebind_dropped_creation,
                      audit_creation_rebind_dropped_other>>
    \/ AuditRestore
       /\ audit_restored_bound' =
            (audit_restored_bound \/ DOMAIN mob_operator_mob_incarnations' # {})
       /\ UNCHANGED <<audit_rebound, audit_rebind_dropped_spawn,
                      audit_rebind_dropped_manage, audit_cleared_after_grant,
                      audit_rebind_dropped_creation,
                      audit_creation_rebind_dropped_other>>
    \/ \E m \in AuditMobs, i \in AuditIncarnations :
        \/ GrantMobOperatorManageMob(m, i) /\ KeepHistory
        \/ SpawnBind(m, i) /\ KeepHistory
        \/ RebindMobOperatorManageMob(m, i)
           /\ audit_rebound' = TRUE
           /\ audit_rebind_dropped_spawn' =
                (audit_rebind_dropped_spawn \/ m \in DOMAIN mob_operator_spawn_profile_scope)
           /\ audit_rebind_dropped_creation' =
                (audit_rebind_dropped_creation \/ HasCreationScope(m))
           /\ UNCHANGED <<audit_rebind_dropped_manage, audit_restored_bound,
                          audit_cleared_after_grant,
                          audit_creation_rebind_dropped_other>>
        \/ SpawnRebind(m, i)
           /\ audit_rebound' = TRUE
           /\ audit_rebind_dropped_manage' =
                (audit_rebind_dropped_manage \/ m \in mob_operator_managed_mob_scope)
           /\ audit_rebind_dropped_creation' =
                (audit_rebind_dropped_creation \/ HasCreationScope(m))
           /\ UNCHANGED <<audit_rebind_dropped_spawn, audit_restored_bound,
                          audit_cleared_after_grant,
                          audit_creation_rebind_dropped_other>>
    \/ \E i \in AuditIncarnations :
        \/ CreationBind(AuditCreationMob, i) /\ KeepHistory
        \/ CreationRebind(AuditCreationMob, i)
           /\ audit_rebound' = TRUE
           /\ audit_creation_rebind_dropped_other' =
                (audit_creation_rebind_dropped_other
                    \/ AuditCreationMob \in mob_operator_managed_mob_scope
                    \/ AuditCreationMob \in DOMAIN mob_operator_spawn_profile_scope)
           /\ UNCHANGED <<audit_rebind_dropped_spawn, audit_rebind_dropped_manage,
                          audit_restored_bound, audit_cleared_after_grant,
                          audit_rebind_dropped_creation>>

AuditInit ==
    /\ Init
    /\ audit_rebound = FALSE
    /\ audit_rebind_dropped_spawn = FALSE
    /\ audit_rebind_dropped_manage = FALSE
    /\ audit_restored_bound = FALSE
    /\ audit_cleared_after_grant = FALSE
    /\ audit_rebind_dropped_creation = FALSE
    /\ audit_creation_rebind_dropped_other = FALSE

AuditSpec == AuditInit /\ [][AuditNext]_<<vars, auditVars>>

AuditStateConstraint == model_step_count <= AuditMaxSteps

AuditGrantBindsTheNamedIncarnation ==
    [][\A m \in AuditMobs, i \in AuditIncarnations :
         /\ ManageGrant(m, i) => (m \in mob_operator_managed_mob_scope' /\ BoundNext(m, i))
         /\ SpawnGrant(m, i) => (m \in DOMAIN mob_operator_spawn_profile_scope' /\ BoundNext(m, i))
         /\ CreationGrant(m, i) =>
              (m \in DOMAIN mob_operator_identity_creation_scope' /\ BoundNext(m, i))
         /\ (ManageGrant(m, i) \/ SpawnGrant(m, i) \/ CreationGrant(m, i)) =>
              \A o \in AuditMobs \ {m} :
                  /\ (o \in DOMAIN mob_operator_mob_incarnations') = Bound(o)
                  /\ Bound(o) => mob_operator_mob_incarnations'[o] = mob_operator_mob_incarnations[o]
      ]_<<vars, auditVars>>

AuditRebindDropsTheOtherScope ==
    [][\A m \in AuditMobs, i \in AuditIncarnations :
         /\ RebindMobOperatorManageMob(m, i) =>
              /\ m \notin DOMAIN mob_operator_spawn_profile_scope'
              /\ m \notin DOMAIN mob_operator_identity_creation_scope'
         /\ SpawnRebind(m, i) =>
              /\ m \notin mob_operator_managed_mob_scope'
              /\ m \notin DOMAIN mob_operator_identity_creation_scope'
         /\ CreationRebind(m, i) =>
              /\ m \notin mob_operator_managed_mob_scope'
              /\ m \notin DOMAIN mob_operator_spawn_profile_scope'
      ]_<<vars, auditVars>>

AuditBoundIncarnationKeepsTheOtherScope ==
    [][\A m \in AuditMobs, i \in AuditIncarnations :
         /\ GrantMobOperatorManageMob(m, i) =>
              /\ mob_operator_spawn_profile_scope' = mob_operator_spawn_profile_scope
              /\ mob_operator_identity_creation_scope' = mob_operator_identity_creation_scope
         /\ SpawnBind(m, i) =>
              /\ mob_operator_managed_mob_scope' = mob_operator_managed_mob_scope
              /\ mob_operator_identity_creation_scope' = mob_operator_identity_creation_scope
         /\ CreationBind(m, i) =>
              /\ mob_operator_managed_mob_scope' = mob_operator_managed_mob_scope
              /\ mob_operator_spawn_profile_scope' = mob_operator_spawn_profile_scope
      ]_<<vars, auditVars>>

AuditResolveClearsScope ==
    [][AuditResolve =>
         /\ mob_operator_managed_mob_scope' = {}
         /\ DOMAIN mob_operator_spawn_profile_scope' = {}
         /\ DOMAIN mob_operator_identity_creation_scope' = {}
         /\ DOMAIN mob_operator_mob_incarnations' = {}
      ]_<<vars, auditVars>>

\* 1 when the arm is enabled, else 0: each arm's ENABLED is evaluated once.
EnabledCount(enabled) == IF enabled THEN 1 ELSE 0

AuditCreationCheckIsTotal ==
    \A m \in AuditMobs, i \in AuditIncarnations, t \in AuditTemplates :
        EnabledCount(ENABLED CreationAdmitted(m, i, t))
            + EnabledCount(ENABLED CreationUnbound(m, i, t))
            + EnabledCount(ENABLED CreationNotInScope(m, i, t)) = 1

AuditCreationAdmittedOnlyInScope ==
    \A m \in AuditMobs, i \in AuditIncarnations, t \in AuditTemplates :
        (ENABLED CreationAdmitted(m, i, t)) <=> InCreationScope(m, i, t)

\* Reachability goals, each checked as an expected violation of its negation.
\* A bound mob was granted again at its other incarnation.
AuditWitnessRebound == audit_rebound
\* A manage rebind dropped the spawn scope its mob held.
AuditWitnessRebindDroppedSpawn == audit_rebind_dropped_spawn
\* A spawn rebind dropped the manage scope its mob held.
AuditWitnessRebindDroppedManage == audit_rebind_dropped_manage
\* One mob holds both scopes at one incarnation.
AuditWitnessBothScopes ==
    \E m \in AuditMobs :
        m \in mob_operator_managed_mob_scope /\ m \in DOMAIN mob_operator_spawn_profile_scope
\* Two mobs are bound to different incarnations.
AuditWitnessTwoIncarnations ==
    /\ Bound("mobid_1") /\ Bound("mobid_2")
    /\ mob_operator_mob_incarnations["mobid_1"] # mob_operator_mob_incarnations["mobid_2"]
\* A restore landed bindings.
AuditWitnessRestoredBindings == audit_restored_bound
\* A resolve cleared scope a grant had given.
AuditWitnessResolveCleared == audit_cleared_after_grant
\* A manage or spawn rebind dropped the creation scope its mob held, and a
\* creation rebind dropped the manage or spawn-profile scope its mob held.
AuditWitnessRebindsDropTheCreationScope ==
    audit_rebind_dropped_creation /\ audit_creation_rebind_dropped_other
\* One state answers the creation check all three ways: it admits a template
\* the first mob's creation scope holds at the mob's incarnation, refuses the
\* same template at the other incarnation, and refuses the second mob,
\* bound with manage and spawn-profile scope and no creation scope, at its
\* own incarnation.
AuditWitnessCreationVerdicts ==
    /\ \E i \in AuditIncarnations, t \in AuditTemplates :
          ENABLED CreationAdmitted("mobid_1", i, t)
    /\ \E i \in AuditIncarnations, t \in AuditTemplates :
          /\ Bound("mobid_1")
          /\ mob_operator_mob_incarnations["mobid_1"] # i
          /\ HasCreationScope("mobid_1")
          /\ t \in mob_operator_identity_creation_scope["mobid_1"]
          /\ ENABLED CreationUnbound("mobid_1", i, t)
    /\ \E i \in AuditIncarnations, t \in AuditTemplates :
          /\ BoundTo("mobid_2", i)
          /\ "mobid_2" \in mob_operator_managed_mob_scope
          /\ "mobid_2" \in DOMAIN mob_operator_spawn_profile_scope
          /\ ~HasCreationScope("mobid_2")
          /\ ENABLED CreationNotInScope("mobid_2", i, t)

NotAuditWitnessRebound == ~AuditWitnessRebound
NotAuditWitnessRebindDroppedSpawn == ~AuditWitnessRebindDroppedSpawn
NotAuditWitnessRebindDroppedManage == ~AuditWitnessRebindDroppedManage
NotAuditWitnessBothScopes == ~AuditWitnessBothScopes
NotAuditWitnessTwoIncarnations == ~AuditWitnessTwoIncarnations
NotAuditWitnessRestoredBindings == ~AuditWitnessRestoredBindings
NotAuditWitnessResolveCleared == ~AuditWitnessResolveCleared
NotAuditWitnessRebindsDropTheCreationScope == ~AuditWitnessRebindsDropTheCreationScope
NotAuditWitnessCreationVerdicts == ~AuditWitnessCreationVerdicts
====
