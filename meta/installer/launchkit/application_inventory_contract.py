"""Shared builder and generated-package guidance for the existing native input."""

HELPERLESS_APPLICATION_CONTRACT = """When deployment.json selects applicationInventory, use the selected Connect
application's native class route, with or without its real setup helper. Create
application/native-inventory.json by copying native-inventory.example.json.
Preserve schema, mob_id, the complete compiled roster and any selected
helper_identity, then author member_classes from the application implementation. The
example includes one class row per zero-instance role with its compiled profile
and empty callbacks/grants. Replace those empty lists with the actual callback
and grant declarations required by the implementation. Keep member_classes
sorted by id without duplicates. Do not create identities or call tools_for_agent
on placeholders. An empty member_classes list is valid only without zero-instance roles.

Each ordinary class uses these existing CompiledMemberClass fields:
- id: the blueprint class id.
- profile: that id's exact deployment.json applicationProfiles value, never a seed role.
- purpose: ordinary (also the default when omitted).
- factory: a stable application-owned declaration id, for example application:project.
  It identifies the implementation supplied by this accepted application. It is
  not a module path, executable loader or a name to discover in a platform registry.
  Forge supplies its existing Application identity customizer to the SDK; Connect
  wraps that customizer and checks its actual dispatcher against this declaration
  on native construction and reconstruction. Implement tools_for_agent for each
  actual adopted identity. No separate factory registration API is required here.
- callbacks: the exact callbacks from tools_for_agent and selected host tools,
  as name, description and input_schema, sorted by name without duplicates.
  Match the real handlers'
  names, descriptions and schemas. Use [] only if the class has no callbacks.
- grants: explicit native rows {tool_name, action: "invoke", consequence: "r0" |
  "r1" | "r2" | "r3"}, sorted by tool_name without duplicates. Choose the actual
  tool names and consequences from the implemented behavior. Use [] only for no grants.
- callback_account: optional existing native account declaration. Absence means
  unknown. Use {kind: "declared_none"} only for callbacks actually using no account,
  or {kind: "selected", authority: ..., subject: ...} for their actual selected account.

For example, a project class whose exact applicationProfiles value is
application__project and whose implementation has no callbacks or grants is:
```json
{"id":"project","purpose":"ordinary","profile":"application__project",
 "factory":"application:project","callbacks":[],"grants":[]}
```
This is a declaration example, not a placeholder member. Adapt it to the actual
class and implementation; do not empty required callbacks or grants to pass checks.
The existing gateway customizer also supports a class with no callbacks. Keep the
paired desired_roster/desired_edges hooks so Forge supplies the customizer, even
before the first instance. Native governed Add and held completion still own
admission; a declaration or desired row does not create or authorize a member.

For the selected SDK's MemberClassAddRequest, route describes a requested local
Console source WITHOUT issued application_parent or controller_parent. Omit those
parents or leave them null; the SDK rejects non-null parents. controller carries
the issuance intent (target, issuance_id and optional controller_route), not
pre-issued credentials. Read the exact selected member_classes reference through
read_runtime_reference(name="roster") and call
context.runtime.member_classes.begin_add(request, publish=...) on that runtime.
Supply the real member, class/profile, principal and route facts from the trusted
application entry. Native validates these facts and owns issuance. Do not invent
an actor or treat a model-supplied class id or a Google identity as native authority.
If that authenticated-entry mapping is missing, name the exact missing input;
do not report that begin_add requires pre-issued parents or does not exist.

Keep admitting rows separate from listed owners until native held completion calls
the application's durable publication callback. Implement the class's real scoped
callbacks and owner continuity; saved assignments cannot stand in for native
owners. A requested refresh cadence, override or pause needs the corresponding
native schedule operation and readback before being reported effective. A schedule
or admitted occurrence alone is not a completed refresh or published result.

Also copy protected application/tool-policy.json to
application/native-tool-policy.json. Preserve provider_id, policy_id, revision
and the complete fixed-seed member set; author each seed's explicit grants there.
Preserve the selected helper and caller setup grants; a class declaration does not
select a setup caller or authorize installation. When selected, the real Dedicated
setup helper starts its native session and MCP connections with no initial message;
setup work begins only through the actual selected caller and protected review.
This seed policy can coexist with a dynamic roster. Later class
instances get their separately declared class grants through Connect's prepared
policy and governed Add, not by adding future identities to the seed policy.
The legacy raw fixed-policy route still refuses dynamic rosters.

Forge retains both authored files as exact accepted source, passes the same class
declarations through schema1 inventory and schema4 native compilation, and reuses
the pins on Restore. Keep JSON integers within native i64/u64 and numbers finite.
Source checks do not prove native admission or a working application. Organizational
access descriptions and skills remain ordinary input supplied intact to the agent
and existing Connect flow; they do not need a catalog or parser.
Check that the exact selected composition supplies an agent-driven discovery/install
entry before claiming that this text can reach one. Governed Add and protected
attachment of known MCP services do not themselves provide discovery or installation.
Keep a missing setup composition explicit while implementing the supported owner,
callback and schedule behavior; do not replace it with guessed grants or a new helper.
"""
