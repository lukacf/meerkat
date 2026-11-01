---- MODULE mob_operator_grant_audit ----
\* Hand-written bounded audit of incarnation-bound mob operator grants.
\*
\* Mob operator scope names a mob id and the incarnation the mob was created
\* at, which its restarts keep (`mob_operator_mob_incarnations`), so a grant
\* for a destroyed mob never reaches a later mob created under the same id. A
\* grant for a bound id at another incarnation rebinds it and drops the other
\* scope held for the old incarnation; a grant at the bound incarnation keeps
\* it. A restore refuses a scope that names an unbound mob. Resolving the
\* create authority, present or absent, clears every scope and binding.
\*
\* This audit keeps the generated MeerkatMachine model unchanged and
\* explores, for two mob ids, two incarnations and one spawn profile: the
\* create authority resolved present and absent, every bind and rebind arm
\* of both grant inputs, and a restore with every binding map over the two
\* ids and incarnations and every manage and spawn scope over the two ids
\* (the restore guard disables the ones that scope an unbound mob).
\* Properties:
\*
\*   mob_operator_scopes_are_bound_to_an_incarnation (generated invariant,
\*     kept from ci.cfg): every scoped mob id is bound.
\*   AuditGrantBindsTheNamedIncarnation (action property): a grant for mob m
\*     at incarnation i puts m in that grant's scope, bound to i, and leaves
\*     every other mob's binding as it was.
\*   AuditRebindDropsTheOtherScope (action property): a manage grant that
\*     rebinds m drops m's spawn scope; a spawn grant that rebinds m drops
\*     m's manage scope.
\*   AuditBoundIncarnationKeepsTheOtherScope (action property): a grant for
\*     an unbound m, or at the incarnation m is bound to, leaves the other
\*     scope as it was.
\*   AuditResolveClearsScope (action property): resolving the create
\*     authority, present or absent, leaves no scope and no binding.
\*
\* AuditWitness* operators are reachability goals the runner checks as
\* expected violations, so the properties are not vacuous. With --mutants the
\* runner also checks that three seeded model defects are refused: a manage
\* rebind that keeps the stale spawn scope, a restore without its bound-scope
\* guard, and a manage grant that records no binding.
\*
\* The TLC config is DERIVED from the generated ci.cfg on every run by
\* mob_operator_grant_audit.sh. Run a deeper bound by hand with:
\*   specs/machines/meerkat_machine/mob_operator_grant_audit.sh 7
EXTENDS model

CONSTANT AuditMaxSteps

VARIABLES
    audit_rebound,               \* a grant moved a bound mob to another incarnation
    audit_rebind_dropped_spawn,  \* a manage rebind dropped spawn scope the mob had
    audit_rebind_dropped_manage, \* a spawn rebind dropped manage scope the mob had
    audit_restored_bound,        \* a restore landed a non-empty binding map
    audit_cleared_after_grant    \* a resolve cleared scope a grant had given

auditVars == <<audit_rebound, audit_rebind_dropped_spawn,
               audit_rebind_dropped_manage, audit_restored_bound,
               audit_cleared_after_grant>>

AuditMobs == {"mobid_1", "mobid_2"}
AuditIncarnations == MobIncarnationIdValues
AuditProfileSets == {{"profile_1"}}
AuditPrincipal == "opaqueprincipaltoken_1"

Bound(m) == m \in DOMAIN mob_operator_mob_incarnations
BoundNext(m, i) ==
    /\ m \in DOMAIN mob_operator_mob_incarnations'
    /\ mob_operator_mob_incarnations'[m] = i
HasScope ==
    mob_operator_managed_mob_scope # {} \/ DOMAIN mob_operator_spawn_profile_scope # {}

ManageGrant(m, i) == GrantMobOperatorManageMob(m, i) \/ RebindMobOperatorManageMob(m, i)
SpawnBind(m, i) == \E ps \in AuditProfileSets : SetMobOperatorSpawnProfilesInMob(m, i, ps)
SpawnRebind(m, i) == \E ps \in AuditProfileSets : RebindMobOperatorSpawnProfilesInMob(m, i, ps)
SpawnGrant(m, i) == SpawnBind(m, i) \/ SpawnRebind(m, i)

AuditResolve ==
    \/ ResolveMobOperatorCreateAuthority("Enable", AuditPrincipal, None, None)
    \/ ResolveMobOperatorCreateAuthorityAbsent("Disable", AuditPrincipal, None, None)

\* Every binding map over the audit mobs and incarnations.
AuditBindingMaps == UNION {[d -> AuditIncarnations] : d \in SUBSET AuditMobs}

AuditRestore ==
    \E managed \in SUBSET AuditMobs, spawned \in SUBSET AuditMobs,
       bindings \in AuditBindingMaps :
        RestoreMobOperatorAuthority(AuditPrincipal, TRUE, TRUE, managed,
            [m \in spawned |-> {"profile_1"}], bindings, None, None)

KeepHistory == UNCHANGED auditVars

AuditNext ==
    \/ AuditResolve
       /\ audit_cleared_after_grant' = (audit_cleared_after_grant \/ HasScope)
       /\ UNCHANGED <<audit_rebound, audit_rebind_dropped_spawn,
                      audit_rebind_dropped_manage, audit_restored_bound>>
    \/ AuditRestore
       /\ audit_restored_bound' =
            (audit_restored_bound \/ DOMAIN mob_operator_mob_incarnations' # {})
       /\ UNCHANGED <<audit_rebound, audit_rebind_dropped_spawn,
                      audit_rebind_dropped_manage, audit_cleared_after_grant>>
    \/ \E m \in AuditMobs, i \in AuditIncarnations :
        \/ GrantMobOperatorManageMob(m, i) /\ KeepHistory
        \/ SpawnBind(m, i) /\ KeepHistory
        \/ RebindMobOperatorManageMob(m, i)
           /\ audit_rebound' = TRUE
           /\ audit_rebind_dropped_spawn' =
                (audit_rebind_dropped_spawn \/ m \in DOMAIN mob_operator_spawn_profile_scope)
           /\ UNCHANGED <<audit_rebind_dropped_manage, audit_restored_bound,
                          audit_cleared_after_grant>>
        \/ SpawnRebind(m, i)
           /\ audit_rebound' = TRUE
           /\ audit_rebind_dropped_manage' =
                (audit_rebind_dropped_manage \/ m \in mob_operator_managed_mob_scope)
           /\ UNCHANGED <<audit_rebind_dropped_spawn, audit_restored_bound,
                          audit_cleared_after_grant>>

AuditInit ==
    /\ Init
    /\ audit_rebound = FALSE
    /\ audit_rebind_dropped_spawn = FALSE
    /\ audit_rebind_dropped_manage = FALSE
    /\ audit_restored_bound = FALSE
    /\ audit_cleared_after_grant = FALSE

AuditSpec == AuditInit /\ [][AuditNext]_<<vars, auditVars>>

AuditStateConstraint == model_step_count <= AuditMaxSteps

AuditGrantBindsTheNamedIncarnation ==
    [][\A m \in AuditMobs, i \in AuditIncarnations :
         /\ ManageGrant(m, i) => (m \in mob_operator_managed_mob_scope' /\ BoundNext(m, i))
         /\ SpawnGrant(m, i) => (m \in DOMAIN mob_operator_spawn_profile_scope' /\ BoundNext(m, i))
         /\ (ManageGrant(m, i) \/ SpawnGrant(m, i)) =>
              \A o \in AuditMobs \ {m} :
                  /\ (o \in DOMAIN mob_operator_mob_incarnations') = Bound(o)
                  /\ Bound(o) => mob_operator_mob_incarnations'[o] = mob_operator_mob_incarnations[o]
      ]_<<vars, auditVars>>

AuditRebindDropsTheOtherScope ==
    [][\A m \in AuditMobs, i \in AuditIncarnations :
         /\ RebindMobOperatorManageMob(m, i) => m \notin DOMAIN mob_operator_spawn_profile_scope'
         /\ SpawnRebind(m, i) => m \notin mob_operator_managed_mob_scope'
      ]_<<vars, auditVars>>

AuditBoundIncarnationKeepsTheOtherScope ==
    [][\A m \in AuditMobs, i \in AuditIncarnations :
         /\ GrantMobOperatorManageMob(m, i) =>
              mob_operator_spawn_profile_scope' = mob_operator_spawn_profile_scope
         /\ SpawnBind(m, i) => mob_operator_managed_mob_scope' = mob_operator_managed_mob_scope
      ]_<<vars, auditVars>>

AuditResolveClearsScope ==
    [][AuditResolve =>
         /\ mob_operator_managed_mob_scope' = {}
         /\ DOMAIN mob_operator_spawn_profile_scope' = {}
         /\ DOMAIN mob_operator_mob_incarnations' = {}
      ]_<<vars, auditVars>>

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

NotAuditWitnessRebound == ~AuditWitnessRebound
NotAuditWitnessRebindDroppedSpawn == ~AuditWitnessRebindDroppedSpawn
NotAuditWitnessRebindDroppedManage == ~AuditWitnessRebindDroppedManage
NotAuditWitnessBothScopes == ~AuditWitnessBothScopes
NotAuditWitnessTwoIncarnations == ~AuditWitnessTwoIncarnations
NotAuditWitnessRestoredBindings == ~AuditWitnessRestoredBindings
NotAuditWitnessResolveCleared == ~AuditWitnessResolveCleared
====
