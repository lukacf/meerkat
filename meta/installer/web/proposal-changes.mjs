import limits from '../launchkit/blueprint-limits.json' with {type: 'json'};

function classes(plan) {
  if (!Array.isArray(plan?.agent_classes)) return null;
  const rows = new Map();
  for (const item of plan.agent_classes) {
    if (!item || typeof item !== 'object' || Array.isArray(item)
        || typeof item.id !== 'string' || !item.id.trim() || rows.has(item.id)
        || typeof item.name !== 'string' || !item.name.trim()
        || !Number.isSafeInteger(item.instances) || item.instances < 0
        || item.instances > limits.maxInstancesPerClass) return null;
    rows.set(item.id, item);
  }
  return rows;
}

const startingAgents = count => `${count} starting ${count === 1 ? 'agent' : 'agents'}`;

/** Presentation only. The caller owns the displayed/current proposal binding. */
export function proposalClassChanges(previousPlan, currentPlan) {
  const previous = classes(previousPlan), current = classes(currentPlan);
  if (!previous || !current) return [];
  const changes = [];
  for (const item of current.values()) {
    const prior = previous.get(item.id);
    if (!prior) {
      changes.push(item.instances === 0
        ? `${item.name} expertise profile added - no starting agents.`
        : `${item.name} added - ${startingAgents(item.instances)} proposed.`);
      continue;
    }
    if (prior.name !== item.name)
      changes.push(`${prior.name} renamed to ${item.name} in the proposal.`);
    if (prior.instances !== item.instances)
      changes.push(`${item.name}: ${prior.instances} to ${item.instances} starting agents proposed.`);
    if (typeof prior.description === 'string' && typeof item.description === 'string'
        && prior.description !== item.description)
      changes.push(`${item.name}: role updated.`);
    if (typeof prior.instructions === 'string' && typeof item.instructions === 'string'
        && prior.instructions !== item.instructions)
      changes.push(`${item.name}: instructions updated.`);
  }
  for (const item of previous.values()) {
    if (current.has(item.id)) continue;
    changes.push(item.instances === 0
      ? `${item.name} expertise profile removed from the proposal.`
      : `${item.name} removed - ${startingAgents(item.instances)} no longer proposed.`);
  }
  return changes;
}
