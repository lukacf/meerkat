/** Aggregated overview wiring plus exact selected links, without inventing routes. */
export const OVERVIEW_RELATIONSHIP_THRESHOLD = 2000;

const wiring = (edge, count = 1) => ({pairs: [[edge.source, edge.target]], edgeId: edge.id,
  source: edge.source, target: edge.target, kind: edge.kind, count,
  alpha: edge.kind === 'peer' ? .18 : .22, dash: edge.kind === 'tool' ? [2, 4] : undefined});

export function createRelationshipProjection(source) {
  const relationships = source.relationships || (source.edgeGroups || []).flatMap(group =>
    group.relationships.map(edge => ({...edge, kind: group.kind})));
  const byId = new Map(relationships.map(edge => [edge.id, edge]));
  const byNode = new Map();
  for (const edge of relationships) {
    for (const id of new Set([edge.source, edge.target])) {
      if (!byNode.has(id)) byNode.set(id, []);
      byNode.get(id).push(edge);
    }
  }
  const overview = relationships.length <= OVERVIEW_RELATIONSHIP_THRESHOLD
    ? relationships.map(edge => wiring(edge))
    : (source.edgeGroups || []).flatMap(group => {
      const edge = byId.get(group.relationships[0]?.id);
      return edge ? [wiring(edge, group.count)] : [];
    });
  let previousId, previous;
  return {relationshipCount: relationships.length, overviewCount: overview.length,
    select(selectedId) {
      if (!selectedId || !byNode.has(selectedId)) return overview;
      if (previousId === selectedId) return previous;
      // Replace a matching representative with the actual selected relationship,
      // then add every other exact incident edge. Unrelated cohorts stay grouped.
      const result = new Map(overview.map(edge => [edge.edgeId, edge]));
      for (const edge of byNode.get(selectedId)) result.set(edge.id, wiring(edge));
      previousId = selectedId;
      previous = [...result.values()];
      return previous;
    }};
}
