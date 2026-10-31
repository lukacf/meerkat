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

/** The words for a permission key; unknown keys come back unchanged. */
export function accessActionLabel(action: string): string {
  return ACCESS_ACTION_WORDS[action] ?? action;
}

/** Whether the console has words for a permission key. */
export function isKnownAccessAction(action: string): boolean {
  return Object.prototype.hasOwnProperty.call(ACCESS_ACTION_WORDS, action);
}
