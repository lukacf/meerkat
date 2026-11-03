import { mountToolkitView } from '@rkat/toolkit/ui';
import type {
  CatalogCategoryView, CatalogEntryView, CatalogView, ToolkitSnapshot, ToolkitViewOptions,
} from '@rkat/toolkit/ui';

const category: CatalogCategoryView = { id: '__all__', label: 'Exact host category' };
const entry: CatalogEntryView = {
  id: 'descriptor', label: 'Catalog description', description: '', roles: ['service', 'frontend'],
  categoryIds: [category.id], support: 'Host description', prerequisites: [],
};
const catalog: CatalogView = { categories: [category], entries: [entry] };
const oldSnapshot: ToolkitSnapshot = {
  scope: 'selected-viewer', revision: 1, observedAt: 'synthetic', announcement: '',
  tasks: [], connections: [], frontends: [], packages: [],
};
const snapshot: ToolkitSnapshot = { ...oldSnapshot, catalog };
const options: ToolkitViewOptions = {
  scope: snapshot.scope, async dispatch(request) { void request.control; },
  onManualConfiguration: () => Promise.resolve(),
};
declare const host: HTMLElement;
mountToolkitView(host, options).update(snapshot);
const invalidRole: CatalogEntryView = { ...entry,
  // @ts-expect-error catalog roles remain the two explicit public roles
  roles: ['installed'],
};
const invalidNavigation: ToolkitViewOptions = { ...options,
  // @ts-expect-error navigation receives no authority or selection argument
  onManualConfiguration: (_selection: string) => {},
};
void invalidRole; void invalidNavigation;
