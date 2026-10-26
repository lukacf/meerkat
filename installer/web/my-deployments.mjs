import { deploymentInstalledSourceKind, deploymentPhase } from './deployment-panel.mjs';
import { remoteReceiptOwned, remoteObservationAvailable, remotePhase } from './remote-deployment.mjs';
import { gcpCloudOwned } from './gcp-guided.mjs';

const UUID = /^[a-f0-9]{8}(?:-[a-f0-9]{4}){3}-[a-f0-9]{12}$/;
const LOCAL_STATES = new Set(['queued', 'installing', 'checking', 'starting', 'running', 'stopping', 'stopped', 'failed', 'interrupted']);
const sources = ['deployment', 'remote-deployment', 'gcp-deployment'];
const labels = ['Forge host', 'Remote application', 'Cloud computer'];

export function deploymentSetups(value) {
  if (!Array.isArray(value?.setups)) throw new Error('Your saved setups could not be read.');
  const seen = new Set();
  return value.setups.map(setup => {
    if (!UUID.test(setup?.id || '') || seen.has(setup.id)) throw new Error('The saved setup list could not be verified.');
    seen.add(setup.id);
    return { id: setup.id, name: typeof setup.name === 'string' && setup.name.trim() ? setup.name : 'Untitled setup' };
  });
}

// A transient projection of the existing receipts. No inventory, action intent,
// credentials, or runtime authority is created or persisted by this reader.
export async function readDeploymentSummary(setup, send, signal) {
  const results = await Promise.allSettled(sources.map(source => send(
    `/api/guide/${encodeURIComponent(setup.id)}/${source}`, { signal, timeoutMs: 30000 })));
  return results.flatMap((result, index) => {
    const unknown = () => [{ kind: sources[index], label: labels[index], status: 'Status unknown',
      destination: 'Saved destination could not be verified', unknown: true }];
    if (result.status !== 'fulfilled') return unknown();
    const receipt = result.value;
    if (receipt === null) return [];
    if (index === 0) {
      if (!deploymentInstalledSourceKind(receipt, setup.id) || !LOCAL_STATES.has(receipt.status) ||
          ![undefined, null, 'local'].includes(receipt.target) || typeof receipt.destination !== 'string') return unknown();
      return [{ kind: sources[index], label: labels[index], destination: receipt.destination,
        status: deploymentPhase(receipt), removal: 'Forge has no generic uninstall for this folder. Stop keeps its files and history.' }];
    }
    if (index === 1) {
      if (!remoteReceiptOwned(receipt, setup)) return unknown();
      return [{ kind: sources[index], label: labels[index], destination: `${receipt.endpoint.host}: ${receipt.destination}`,
        status: remoteObservationAvailable(receipt) ? remotePhase(receipt) : 'Status unknown',
        unknown: !remoteObservationAvailable(receipt), removal: receipt.target === 'gcp'
          ? 'Stopping the application keeps the cloud computer and its charges. Use the recorded cloud removal controls separately.'
          : 'Forge has no generic remote uninstall. Stop keeps the application files and history.' }];
    }
    if (!gcpCloudOwned(receipt, setup.id)) return unknown();
    return [{ kind: sources[index], label: labels[index],
      destination: `${receipt.destination.project} / ${receipt.destination.zone} / ${receipt.destination.instance}`,
      status: ({ provisioning: 'Cloud preparation in progress', 'resume-required': 'Cloud setup needs attention',
        blocked: 'Cloud setup blocked', 'vm-ready': 'Cloud computer prepared - application status is separate',
        removing: 'Removal in progress - resources may remain', 'removal-required': 'Removal incomplete - resources may remain',
        removed: 'Original cloud resources confirmed removed' })[receipt.status],
      removal: 'Remove cloud computer reviews the recorded VM, boot disk and access rule, with a data-loss confirmation.' }];
  });
}

// Bound receipt reads across setups and retire queued work when the view closes.
export async function readDeploymentSummaries(setups, send, signal, publish) {
  let next = 0;
  await Promise.all(Array.from({ length: Math.min(3, setups.length) }, async () => {
    while (!signal.aborted && next < setups.length) {
      const setup = setups[next++];
      const entries = await readDeploymentSummary(setup, send, signal);
      if (!signal.aborted) publish(setup, entries);
    }
  }));
}
