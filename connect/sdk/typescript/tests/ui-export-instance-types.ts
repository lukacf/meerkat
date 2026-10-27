import type { PackageView } from '@rkat/toolkit/ui';

const legacyExport: PackageView['exports'][number] = {
  kind: 'service', id: 'mail-export', label: 'Mail', status: { tone: 'ready', text: 'Ready' },
};
const instanceIds = ['work-connection', 'personal-connection'] as const;
const linkedExport: PackageView['exports'][number] = { ...legacyExport, instanceIds };
const frontendExport: PackageView['exports'][number] = {
  ...legacyExport, kind: 'frontend', instanceIds: ['private-entry', 'public-entry'],
};
const invalidExport: PackageView['exports'][number] = {
  ...legacyExport,
  // @ts-expect-error Presentation references are string IDs, not numeric indexes.
  instanceIds: [123],
};
// @ts-expect-error A consumer cannot mutate host-owned projection references.
linkedExport.instanceIds?.push('another-connection');
void frontendExport;
void invalidExport;
