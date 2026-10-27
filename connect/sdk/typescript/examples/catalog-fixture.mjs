// Synthetic catalog descriptions only. They are not a support or readiness claim.
export function catalogFixture() {
  return {
    categories: [
      { id: 'communication', label: 'Communication' },
      { id: '__all__', label: 'Documents' },
    ],
    entries: [
      { id: 'slack-description', label: 'Slack', description: 'Read messages and expose an agent in a selected workspace.',
        roles: ['service', 'frontend'], categoryIds: ['communication'],
        support: 'Synthetic profile description; actual availability is supplied by the host.',
        prerequisites: ['A host-selected workspace', 'Separate permission for service reads and frontend audiences'],
        evidence: 'Preview data, not a provider or installation check.' },
      { id: 'docs-description', label: 'Document library', description: 'Read a selected document collection.',
        roles: ['service'], categoryIds: ['__all__'],
        support: 'Host-provided document profile description.', prerequisites: ['An explicitly selected collection'] },
    ],
  };
}
