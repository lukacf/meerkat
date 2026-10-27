/** Complete identity browsing is host presentation, independent of composition. */
export const IDENTITY_PAGE_SIZE = 60;

export function populationInspection(model, memberIds, search = '', page = 0) {
  const included = Array.isArray(memberIds) ? new Set(memberIds) : null;
  const population = (model?.nodes || []).filter(node => node.kind === 'agent' && (!included || included.has(node.id)));
  const query = String(search || '').toLowerCase();
  const matches = population.filter(node => `${node.name} ${node.id}`.toLowerCase().includes(query));
  const pageCount = Math.max(1, Math.ceil(matches.length / IDENTITY_PAGE_SIZE));
  const pageIndex = Math.max(0, Math.min(pageCount - 1, Number.isInteger(page) ? page : 0));
  const offset = pageIndex * IDENTITY_PAGE_SIZE;
  return {populationCount: population.length, matchCount: matches.length, pageIndex, pageCount,
    start: matches.length ? offset + 1 : 0, end: Math.min(matches.length, offset + IDENTITY_PAGE_SIZE),
    items: matches.slice(offset, offset + IDENTITY_PAGE_SIZE),
    hasPrevious: pageIndex > 0, hasNext: pageIndex + 1 < pageCount};
}
