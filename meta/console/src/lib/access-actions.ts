// Console access permissions in words. The access controller's vocabulary
// is a closed list of keys (meerkat-meta access/model.rs); people read the
// words, and the key stays available as the tooltip and the option value.
// A key this console does not know is shown as it is.

const ACCESS_ACTION_WORDS: Readonly<Record<string, string>> = {
  "agent.view": "View agents",
  "agent.send": "Message agents",
  "agent.memory.read": "Read agent memory",
  "agent.memory.write": "Write agent memory",
  "agent.memory.delete": "Delete agent memory",
  "agent.memory.admin": "Administer agent memory",
  "operator.memory.read": "Read operator memory",
  "mob.memory.read": "Read team memory",
  "mob.memory.propose": "Propose team memory",
  "mob.memory.commit": "Commit team memory",
  "memory.quarantine.review": "Review quarantined memory",
  "agent.spawn": "Create agents",
  "agent.respawn": "Restart agents",
  "agent.retire": "Retire agents",
  "agent.reset": "Reset agents",
  "gating.view": "View approvals",
  "gating.decide": "Decide approvals",
  "workgraph.view": "View work items",
  "workgraph.manage": "Manage work items",
  "mob.observe": "Watch team event streams",
  "runtime.admin": "Manage routing and wiring",
  "topology.view": "View topology",
  "topology.audit": "Read topology changes",
  "topology.connect": "Connect agents",
  "topology.disconnect": "Disconnect agents",
  "topology.reconnect": "Reconnect agents",
  "topology.bulk": "Change many connections at once",
  "topology.cross_authority": "Connect agents across teams",
  "mobpack.author": "Author team packages",
  "mobpack.deploy": "Deploy team packages",
  "access.admin": "Administer console access",
  "extension.read": "Read extension state",
  "extension.edit": "Edit extension state",
  "extension.manage": "Manage extension state",
};

const FAMILY_WORDS: Readonly<Record<string, string>> = {
  agent: "agent",
  "agent.memory": "agent memory",
  operator: "operator",
  "operator.memory": "operator memory",
  mob: "team",
  "mob.memory": "team memory",
  memory: "memory",
  "memory.quarantine": "quarantine",
  gating: "approval",
  workgraph: "work item",
  runtime: "routing and wiring",
  topology: "topology",
  mobpack: "team package",
  access: "console access",
  extension: "extension",
};

/** The words for a permission key, or for a pattern a rule may hold ("*"
 * reads "All actions", "agent.*" "All agent actions"); unknown keys come
 * back unchanged. */
export function accessActionLabel(action: string): string {
  if (action === "*") return "All actions";
  if (action.endsWith(".*")) {
    const prefix = action.slice(0, -2);
    const words = FAMILY_WORDS[prefix];
    return words ? `All ${words} actions` : `All ${prefix} actions`;
  }
  return ACCESS_ACTION_WORDS[action] ?? action;
}

/** Whether a rule's action entry is a pattern ("*", "agent.*"). */
export function isAccessActionPattern(action: string): boolean {
  return action === "*" || action.endsWith(".*");
}

/** Whether the console has words for a permission key or pattern. */
export function isKnownAccessAction(action: string): boolean {
  return isAccessActionPattern(action) || Object.prototype.hasOwnProperty.call(ACCESS_ACTION_WORDS, action);
}
