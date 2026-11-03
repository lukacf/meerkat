const TOPICS = new Set(['outcome', 'audience', 'interfaces', 'ownership', 'inputs', 'workflow',
  'knowledge', 'authority', 'destination', 'first_version']);
const STATUSES = new Set(['confirmed', 'proposed', 'unresolved']);
const DISPOSITIONS = new Set(['open', 'supported', 'prerequisite', 'deferred', 'cancelled', 'handoff']);
const record = value => !!value && typeof value === 'object' && !Array.isArray(value);
const has = (value, key) => Object.hasOwn(value, key);

function text(value, limit, empty = false) {
  if (typeof value !== 'string' || [...value].length > limit || (!empty && !value.trim())) return false;
  for (const char of value) {
    const point = char.codePointAt(0);
    if (point >= 0xd800 && point <= 0xdfff || point < 32 && !'\n\r\t'.includes(char) || point === 127) return false;
  }
  return true;
}

function evidence(value, required = false) {
  return Array.isArray(value) && value.length <= 40 && (!required || value.length > 0)
    && value.every(id => text(id, 256) && !/[\u0000-\u001f\u007f]/.test(id))
    && new Set(value).size === value.length;
}

/** Conversational composition preferences, separate from deployable requirements. */
export function simulationVisualizationFeedback(value) {
  if (!record(value) || Object.keys(value).sort().join(',') !== 'source_message_id,text'
      || !text(value.text, 1000) || !text(value.source_message_id, 256)
      || /[\u0000-\u001f\u007f]/.test(value.source_message_id)) return null;
  return {text: value.text, source_message_id: value.source_message_id};
}

/** Current saved understanding for composition only; never derive runtime edges from it. */
export function simulationDesignContext(brief) {
  if (!record(brief)) return null;
  const facts = has(brief, 'facts') ? brief.facts : {};
  const requirements = has(brief, 'requirements') ? brief.requirements : [];
  if (!record(facts) || Object.keys(facts).length > TOPICS.size || !Array.isArray(requirements)
      || requirements.length > 32) return null;
  const projectedFacts = {};
  for (const [topic, fact] of Object.entries(facts).sort(([a], [b]) => a < b ? -1 : a > b ? 1 : 0)) {
    if (!TOPICS.has(topic) || !record(fact) || !STATUSES.has(fact.status)
        || !text(fact.summary, 1600)) return null;
    const sources = has(fact, 'source_message_ids') ? fact.source_message_ids : [];
    if (!evidence(sources, fact.status === 'confirmed')) return null;
    projectedFacts[topic] = {summary: fact.summary, status: fact.status, source_message_ids: [...sources]};
  }
  const projectedRequirements = [], seen = new Set();
  for (const requirement of requirements) {
    if (!record(requirement) || typeof requirement.id !== 'string'
        || !/^[a-z][a-z0-9_-]{0,79}$/.test(requirement.id) || seen.has(requirement.id)
        || !text(requirement.outcome, 1200) || !DISPOSITIONS.has(requirement.disposition)) return null;
    const supported = has(requirement, 'supported_subset') ? requirement.supported_subset : '';
    const extension = has(requirement, 'extension') ? requirement.extension : '';
    const decisions = has(requirement, 'decision_message_ids') ? requirement.decision_message_ids : [];
    if (!text(supported, 1600, true) || !text(extension, 1600, true)
        || !evidence(requirement.source_message_ids, true)
        || !evidence(decisions, ['deferred', 'cancelled', 'handoff'].includes(requirement.disposition))
        || ['supported', 'prerequisite'].includes(requirement.disposition) && extension.trim()) return null;
    seen.add(requirement.id);
    projectedRequirements.push({id: requirement.id, outcome: requirement.outcome,
      supported_subset: supported, extension, disposition: requirement.disposition,
      source_message_ids: [...requirement.source_message_ids], decision_message_ids: [...decisions]});
  }
  return {facts: projectedFacts, requirements: projectedRequirements};
}
