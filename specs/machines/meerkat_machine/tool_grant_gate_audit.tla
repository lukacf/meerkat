---- MODULE tool_grant_gate_audit ----
\* Hand-written bounded audit of the scoped tool grant gate's server gate.
\*
\* Every build installs its gate (ReplaceToolGrantGate): the names it gated,
\* the MCP servers it gated, whether it gates every server it does not
\* configure ungated (the unconfigured-servers mode), the servers it
\* configures ungated and the role entries it ignored as not mounted. A
\* model-request boundary extends the gate with the names a gated server
\* provides now (ExtendToolGrantGate), and the grant path stages a server only
\* when the gate gates it (SurfaceStageGatedAdd). A server is gated when the
\* gate names it, or, while the mode is on, when it is not one of the
\* ungated servers.
\*
\* This audit keeps the generated MeerkatMachine model unchanged and
\* explores, for one session, two tool names and two servers: every gate a
\* build can be asked to install, every extension, every open set, and the
\* gated and the ordinary stage of either server, in Idle. Properties:
\*
\*   tool_grant_open_names_are_gated, tool_grant_not_mounted_are_not_gated,
\*   tool_grant_ungated_servers_are_not_gated (generated invariants, kept
\*     from ci.cfg).
\*   AuditGatedNamesShrinkOnlyOnReplace (action property): only a build's
\*     gate takes a name out of the gated names.
\*   AuditServersChangeOnlyOnReplace (action property): only a build's gate
\*     changes the gated servers, the mode or the ungated servers.
\*   AuditExtendNeverOpens (action property): an extension never changes the
\*     open names.
\*   AuditExtendOnlyForGatedServers (action property): an extension is taken
\*     only for a server the gate gates.
\*   AuditGatedStageOnlyStagesGatedServers (action property): the gated
\*     stage stages only a server the gate gates.
\*
\* AuditWitness* operators are reachability goals the runner checks as
\* expected violations, so the properties are not vacuous. With --mutants the
\* runner also checks that nine seeded model defects are refused: an
\* extension that opens its names, an extension or a gated stage without its
\* server guard, an open set without its gated-names guard, an extension that
\* leaves an added name ignored, a gate install without either disjointness
\* guard, and a server predicate without its ungated exception or without its
\* mode test.
\*
\* The TLC config is DERIVED from the generated ci.cfg on every run by
\* tool_grant_gate_audit.sh. Run a deeper bound by hand with:
\*   specs/machines/meerkat_machine/tool_grant_gate_audit.sh 7
EXTENDS model

CONSTANT AuditMaxSteps

VARIABLES
    audit_step,                 \* the kind of the last audit step
    audit_step_server,          \* the server of the last extension or stage
    audit_extended_names,       \* names an extension added since the last gate
    audit_extend_changed,       \* an extension added a name
    audit_extend_unchanged,     \* an extension found every name gated
    audit_extend_by_mode,       \* an extension for a server only the mode gates
    audit_extend_gated_ignored, \* an extension gated a name the build ignored
    audit_staged_named,         \* a gated stage of a server the gate names
    audit_staged_by_mode        \* a gated stage of a server only the mode gates

auditVars == <<audit_step, audit_step_server, audit_extended_names,
               audit_extend_changed, audit_extend_unchanged, audit_extend_by_mode,
               audit_extend_gated_ignored, audit_staged_named, audit_staged_by_mode>>

AuditSession == "sessionid_1"
AuditNames == ToolNameValues
AuditServers == {"server_1", "server_2"}

\* The server predicate, written out as the machine's guards read it.
ServerGated(s) ==
    \/ s \in tool_grant_gated_servers
    \/ (tool_grant_gates_unconfigured_servers /\ s \notin tool_grant_ungated_servers)

KeepFlags ==
    UNCHANGED <<audit_extended_names, audit_extend_changed, audit_extend_unchanged,
                audit_extend_by_mode, audit_extend_gated_ignored, audit_staged_named,
                audit_staged_by_mode>>

\* Initialize and register the one session the gate inputs need.
AuditPrefix ==
    \/ /\ model_step_count = 0
       /\ Initialize
       /\ audit_step' = "prefix"
       /\ UNCHANGED audit_step_server
       /\ KeepFlags
    \/ /\ model_step_count = 1
       /\ RegisterSessionIdle(AuditSession, None, {})
       /\ audit_step' = "prefix"
       /\ UNCHANGED audit_step_server
       /\ KeepFlags

AuditReplace ==
    \E g \in SUBSET AuditNames, gs \in SUBSET AuditServers, m \in BOOLEAN,
       us \in SUBSET AuditServers, nm \in SUBSET AuditNames :
        /\ ReplaceToolGrantGateIdle(g, gs, m, us, nm)
        /\ audit_step' = "replace"
        /\ audit_extended_names' = {}
        /\ UNCHANGED <<audit_step_server, audit_extend_changed, audit_extend_unchanged,
                       audit_extend_by_mode, audit_extend_gated_ignored, audit_staged_named,
                       audit_staged_by_mode>>

AuditExtend ==
    \E s \in AuditServers, g \in SUBSET AuditNames :
        \/ /\ ExtendToolGrantGateChangedIdle(s, g)
           /\ audit_step' = "extend"
           /\ audit_step_server' = s
           /\ audit_extended_names' = audit_extended_names \cup (g \ tool_grant_gated_names)
           /\ audit_extend_changed' = TRUE
           /\ audit_extend_by_mode' = (audit_extend_by_mode \/ s \notin tool_grant_gated_servers)
           /\ audit_extend_gated_ignored' =
                (audit_extend_gated_ignored \/ (g \cap tool_grant_not_mounted_names) # {})
           /\ UNCHANGED <<audit_extend_unchanged, audit_staged_named, audit_staged_by_mode>>
        \/ /\ ExtendToolGrantGateUnchangedIdle(s, g)
           /\ audit_step' = "extend"
           /\ audit_step_server' = s
           /\ audit_extend_unchanged' = TRUE
           /\ UNCHANGED <<audit_extended_names, audit_extend_changed, audit_extend_by_mode,
                          audit_extend_gated_ignored, audit_staged_named, audit_staged_by_mode>>

AuditOpen ==
    \E o \in SUBSET AuditNames :
        /\ (ApplyToolGrantOpenSetChangedIdle(o) \/ ApplyToolGrantOpenSetUnchangedIdle(o))
        /\ audit_step' = "open"
        /\ UNCHANGED audit_step_server
        /\ KeepFlags

AuditStage ==
    \E s \in AuditServers :
        \/ /\ SurfaceStageGatedAddIdle(s, 0)
           /\ audit_step' = "gated_stage"
           /\ audit_step_server' = s
           /\ audit_staged_named' = (audit_staged_named \/ s \in tool_grant_gated_servers)
           /\ audit_staged_by_mode' = (audit_staged_by_mode \/ s \notin tool_grant_gated_servers)
           /\ UNCHANGED <<audit_extended_names, audit_extend_changed, audit_extend_unchanged,
                          audit_extend_by_mode, audit_extend_gated_ignored>>
        \/ /\ SurfaceStageAddIdle(s, 0)
           /\ audit_step' = "stage"
           /\ audit_step_server' = s
           /\ KeepFlags

AuditNext ==
    \/ AuditPrefix
    \/ model_step_count >= 2 /\ (AuditReplace \/ AuditExtend \/ AuditOpen \/ AuditStage)

AuditInit ==
    /\ Init
    /\ audit_step = "init"
    /\ audit_step_server = "none"
    /\ audit_extended_names = {}
    /\ audit_extend_changed = FALSE
    /\ audit_extend_unchanged = FALSE
    /\ audit_extend_by_mode = FALSE
    /\ audit_extend_gated_ignored = FALSE
    /\ audit_staged_named = FALSE
    /\ audit_staged_by_mode = FALSE

AuditSpec == AuditInit /\ [][AuditNext]_<<vars, auditVars>>

AuditStateConstraint == model_step_count <= AuditMaxSteps

AuditGatedNamesShrinkOnlyOnReplace ==
    [][audit_step' # "replace" => tool_grant_gated_names \subseteq tool_grant_gated_names']_<<vars, auditVars>>

AuditServersChangeOnlyOnReplace ==
    [][audit_step' # "replace" =>
         /\ tool_grant_gated_servers' = tool_grant_gated_servers
         /\ tool_grant_gates_unconfigured_servers' = tool_grant_gates_unconfigured_servers
         /\ tool_grant_ungated_servers' = tool_grant_ungated_servers
      ]_<<vars, auditVars>>

AuditExtendNeverOpens ==
    [][audit_step' = "extend" => tool_grant_open_names' = tool_grant_open_names]_<<vars, auditVars>>

AuditExtendOnlyForGatedServers ==
    [][audit_step' = "extend" => ServerGated(audit_step_server')]_<<vars, auditVars>>

AuditGatedStageOnlyStagesGatedServers ==
    [][audit_step' = "gated_stage" => ServerGated(audit_step_server')]_<<vars, auditVars>>

\* Reachability goals, each checked as an expected violation of its negation.
\* A build gated a server.
AuditWitnessGatedServer == tool_grant_gated_servers # {}
\* A build ignored a role entry as not mounted.
AuditWitnessIgnoredName == tool_grant_not_mounted_names # {}
\* A build gates every unconfigured server and configures one ungated.
AuditWitnessModeWithUngated ==
    tool_grant_gates_unconfigured_servers /\ tool_grant_ungated_servers # {}
\* An extension added a name.
AuditWitnessExtendChanged == audit_extend_changed
\* An extension found every name gated already.
AuditWitnessExtendUnchanged == audit_extend_unchanged
\* An extension for a server only the mode gates.
AuditWitnessExtendByMode == audit_extend_by_mode
\* An extension gated a name the build had ignored.
AuditWitnessExtendGatedIgnored == audit_extend_gated_ignored
\* A live grant opened a name an extension added.
AuditWitnessOpenedExtended == (tool_grant_open_names \cap audit_extended_names) # {}
\* The grant path staged a server the gate names.
AuditWitnessStagedNamed == audit_staged_named
\* The grant path staged a server only the mode gates.
AuditWitnessStagedByMode == audit_staged_by_mode

NotAuditWitnessGatedServer == ~AuditWitnessGatedServer
NotAuditWitnessIgnoredName == ~AuditWitnessIgnoredName
NotAuditWitnessModeWithUngated == ~AuditWitnessModeWithUngated
NotAuditWitnessExtendChanged == ~AuditWitnessExtendChanged
NotAuditWitnessExtendUnchanged == ~AuditWitnessExtendUnchanged
NotAuditWitnessExtendByMode == ~AuditWitnessExtendByMode
NotAuditWitnessExtendGatedIgnored == ~AuditWitnessExtendGatedIgnored
NotAuditWitnessOpenedExtended == ~AuditWitnessOpenedExtended
NotAuditWitnessStagedNamed == ~AuditWitnessStagedNamed
NotAuditWitnessStagedByMode == ~AuditWitnessStagedByMode
====
