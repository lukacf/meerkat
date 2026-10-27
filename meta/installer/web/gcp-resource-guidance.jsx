import React from 'react';
import { gcpResourceManagementLinks } from './gcp-guided.mjs';

export function CloudResourceGuidance({ destination, removalAvailable = false, removalInProgress = false }) {
  const links = gcpResourceManagementLinks(destination);
  if (!links) return null;
  return <aside className="deployment-note gcp-resource-guidance" aria-label="Cloud resources and charges">
    {!removalInProgress && <><p><strong>Stopping the application does not remove cloud resources.</strong> Cancelling private setup or closing Forge also leaves them in place. Cloud charges can continue.</p>
      <p>Back up records you need before deleting the VM or disk. In Google Cloud, find <strong>{destination.instance}</strong> in <strong>{destination.zone}</strong>, project <strong>{destination.project}</strong>. {removalAvailable ? 'Use the saved cloud card to review and remove its original resources.' : 'Guided removal is unavailable here; manage these resources in Google Cloud.'}</p></>}
    <div className="deployment-actions">
      <a className="setup-action" href={links.vm} target="_blank" rel="noopener noreferrer">Manage VM in Google Cloud</a>
      <a className="setup-action" href={links.disk} target="_blank" rel="noopener noreferrer">Manage disk in Google Cloud</a>
    </div>
  </aside>;
}
