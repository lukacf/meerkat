import { gcpPreflightAvailable, gcpCheckFresh, gcpCheckValid, gcpBindingMatches } from './gcp-preflight.mjs';

const UUID = /^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$/;
const HASH = /^[a-f0-9]{64}$/;
const RESOURCE = /^[a-z](?:[a-z0-9-]{0,61}[a-z0-9])?$/;
const PROJECT = /^[a-z][a-z0-9-]{4,28}[a-z0-9]$/;
const ACCOUNT = /^[A-Za-z0-9][A-Za-z0-9._+%-]{0,190}@[A-Za-z0-9][A-Za-z0-9.-]{0,190}$/;
const BINDING = ['guide_id', 'guide_version', 'build_id', 'blueprint_hash', 'source_digest', 'package_sha256'];
const ENDPOINT = ['transport', 'operation_id', 'account', 'project', 'zone', 'instance', 'instance_id', 'ownership_token', 'host', 'user', 'ssh_port'];
const UUID4 = /^[a-f0-9]{8}-[a-f0-9]{4}-4[a-f0-9]{3}-[89ab][a-f0-9]{3}-[a-f0-9]{12}$/;
const REMOVAL = new Set(['removing', 'removal-required', 'removed']);
const REMOVAL_RESOURCE = new Set(['waiting', 'checking', 'removing', 'absent']);

export function gcpRemovalLifecycle(status) { return REMOVAL.has(status); }

function removalValid(value, status) {
  return value && UUID4.test(value.request_id || '') && value.acknowledge_data_loss === true &&
    value.resources && Object.keys(value.resources).length === 3 &&
    ['vm', 'disk', 'firewall'].every((kind) => REMOVAL_RESOURCE.has(value.resources[kind]?.status) &&
      (status !== 'removed' || value.resources[kind].status === 'absent'));
}

export function gcpRemovalResourceRows(cloud) {
  return [['vm', 'VM'], ['disk', 'Boot disk'], ['firewall', 'SSH access rule']].map(([kind, label]) => {
    const name = cloud.destination.instance + (kind === 'firewall' ? '-iap-ssh' : '');
    const saved = cloud.resources?.[kind];
    const recorded = saved?.name === name && typeof saved.id === 'string' && /^[1-9][0-9]{0,19}$/.test(saved.id) &&
      BigInt(saved.id) <= 18446744073709551615n;
    return { kind, label, name, id: recorded ? saved.id : null, status: cloud.removal?.resources[kind]?.status || 'waiting' };
  });
}

export function gcpDeploymentUrl(id, action = '') {
  return `/api/guide/${encodeURIComponent(id)}/gcp-deployment${action ? `/${action}` : ''}`;
}

export function gcpResourceManagementLinks(destination) {
  if (!destination || !['project', 'zone', 'instance'].every((key) => typeof destination[key] === 'string') || !PROJECT.test(destination.project || '') ||
      !RESOURCE.test(destination.zone || '') || !RESOURCE.test(destination.instance || '')) return null;
  const project = encodeURIComponent(destination.project);
  return {
    vm: `https://console.cloud.google.com/compute/instances?project=${project}`,
    disk: `https://console.cloud.google.com/compute/disks?project=${project}`,
  };
}

export function gcpEndpointValid(value) {
  return value && typeof value === 'object' && Object.keys(value).length === ENDPOINT.length &&
    ENDPOINT.every((key) => Object.hasOwn(value, key)) && ENDPOINT.filter((key) => key !== 'ssh_port').every((key) => typeof value[key] === 'string') && value.transport === 'gcp-iap' && UUID.test(value.operation_id || '') &&
    ACCOUNT.test(value.account || '') && PROJECT.test(value.project || '') && RESOURCE.test(value.zone || '') && RESOURCE.test(value.instance || '') &&
    typeof value.instance_id === 'string' && /^[1-9][0-9]{0,19}$/.test(value.instance_id) &&
    BigInt(value.instance_id) <= 18446744073709551615n && /^[a-f0-9]{32}$/.test(value.ownership_token || '') &&
    value.host === `compute.${value.instance_id}` && value.user === 'mobkit' && value.ssh_port === 22;
}

export function gcpCloudOwned(value, guideId, operationId = null) {
  if (!value || value.protocol !== 1 || value.guide_id !== guideId || !UUID.test(value.operation_id || '') ||
      operationId && value.operation_id !== operationId || !['provisioning', 'resume-required', 'blocked', 'vm-ready', ...REMOVAL].includes(value.status) ||
      !ACCOUNT.test(value.account || '') || typeof value.account !== 'string' || typeof value.detail !== 'string' || typeof value.phase !== 'string' || value.application_installed !== false || value.application_status !== 'not-installed' ||
      value.vm_verified !== (value.status === 'vm-ready') || value.resume_required !== (value.status === 'resume-required')) return false;
  if (gcpRemovalLifecycle(value.status) && !removalValid(value.removal, value.status) ||
      !gcpRemovalLifecycle(value.status) && value.removal != null) return false;
  const binding = value.binding, destination = value.destination;
  if (!binding || binding.guide_id !== guideId || !Number.isSafeInteger(binding.guide_version) || binding.guide_version < 1 ||
      typeof binding.build_id !== 'string' || !binding.build_id || !['blueprint_hash', 'source_digest', 'package_sha256'].every((key) => HASH.test(binding[key] || '')) ||
      !destination || !PROJECT.test(destination.project || '') || !RESOURCE.test(destination.zone || '') || !RESOURCE.test(destination.instance || '')) return false;
  return value.status !== 'vm-ready' && value.endpoint == null || gcpEndpointValid(value.endpoint) && value.endpoint.operation_id === value.operation_id &&
    ['project', 'zone', 'instance'].every((key) => value.endpoint[key] === destination[key]) &&
    value.account === value.endpoint.account;
}

export function gcpCloudSameGeneration(value, original) {
  return gcpCloudOwned(value, original?.guide_id, original?.operation_id) && value.account === original.account &&
    BINDING.every((key) => value.binding[key] === original.binding[key]) &&
    ['project', 'zone', 'instance'].every((key) => value.destination[key] === original.destination[key]) &&
    ['vm', 'disk', 'firewall'].every((kind) => !original.resources?.[kind] ||
      ['id', 'name'].every((key) => value.resources?.[kind]?.[key] === original.resources[kind][key]));
}

export function gcpCloudCurrent(value, state, build, blocked = false) {
  return gcpCloudOwned(value, state?.id) && !gcpRemovalLifecycle(value.status) && gcpPreflightAvailable(state, build, blocked) &&
    value.binding.guide_version <= state.version && value.binding.build_id === build.build_id &&
    value.binding.source_digest === build.source_digest && value.binding.blueprint_hash === build.blueprint_hash &&
    (!build.package_sha256 || value.binding.package_sha256 === build.package_sha256) &&
    ['project', 'zone'].every((key) => value.destination[key] === (state.proposal || state.blueprint).gcp[key]) &&
    value.destination.instance === (state.proposal || state.blueprint).project.slug;
}

export function gcpCanCreate(plan, state, build, blocked = false) {
  return gcpPreflightAvailable(state, build, blocked) && gcpBindingMatches(plan, state, build) &&
    gcpCheckFresh(plan) && gcpCheckValid(plan, plan?.account, plan?.project) && plan.ready_for_plan === true && plan.guided_supported === true;
}

export function gcpIntentKey(guideId) { return `mobkit-cloud-request-v1:${guideId}`; }

export function gcpIntentValid(value, guideId) {
  return value?.protocol === 1 && UUID.test(value.request_id || '') && value.guide_id === guideId &&
    Number.isSafeInteger(value.version) && value.version > 0 && typeof value.build_id === 'string' && value.build_id &&
    ACCOUNT.test(value.account || '') && PROJECT.test(value.project || '') && value.binding?.guide_id === guideId &&
    value.binding.guide_version === value.version && value.binding.build_id === value.build_id &&
    ['blueprint_hash', 'source_digest', 'package_sha256'].every((key) => HASH.test(value.binding[key] || ''));
}

export function gcpIntentMatchesPlan(intent, plan, state, build, blocked = false) {
  return gcpIntentValid(intent, state?.id) && gcpCanCreate(plan, state, build, blocked) && intent.account === plan.account &&
    intent.project === plan.project && BINDING.every((key) => intent.binding[key] === plan.binding[key]);
}

export function gcpMakeIntent(plan, state, build, requestId) {
  if (!gcpCanCreate(plan, state, build) || !UUID.test(requestId || '')) return null;
  return { protocol: 1, guide_id: state.id, version: state.version, build_id: build.build_id,
    account: plan.account, project: plan.project, request_id: requestId, binding: structuredClone(plan.binding) };
}

export function gcpStoreIntent(storage, value) {
  if (!gcpIntentValid(value, value?.guide_id)) throw new Error('The cloud request does not match this accepted application.');
  const key = gcpIntentKey(value.guide_id), encoded = JSON.stringify(value);
  storage.setItem(key, encoded);
  if (storage.getItem(key) !== encoded) throw new Error('The cloud request could not be saved. No setup request was sent.');
}

export function gcpLoadIntent(storage, guideId) {
  const raw = storage.getItem(gcpIntentKey(guideId));
  if (raw === null) return null;
  let value;
  try { value = JSON.parse(raw); } catch { throw new Error('The saved cloud request is unreadable. Preserve it before starting another setup.'); }
  if (!gcpIntentValid(value, guideId)) throw new Error('The saved cloud request does not identify this setup. Preserve it before starting another setup.');
  return value;
}

export function gcpCloudMatchesIntent(cloud, intent) {
  return gcpIntentValid(intent, intent?.guide_id) && gcpCloudOwned(cloud, intent.guide_id, intent.request_id) &&
    cloud.account === intent.account && cloud.destination.project === intent.project &&
    BINDING.every((key) => cloud.binding[key] === intent.binding[key]);
}

export function gcpRemovalIntentKey(guideId) { return `mobkit-cloud-removal-v1:${guideId}`; }

export function gcpRemovalIntentValid(value, guideId) {
  return value?.protocol === 1 && value.guide_id === guideId && UUID4.test(value.operation_id || '') &&
    UUID4.test(value.request_id || '') && value.acknowledge_data_loss === true &&
    Object.keys(value).length === 5;
}

export function gcpMakeRemovalIntent(cloud, requestId) {
  if (!gcpCloudOwned(cloud, cloud?.guide_id) || !UUID4.test(requestId || '')) return null;
  return { protocol: 1, guide_id: cloud.guide_id, operation_id: cloud.operation_id,
    request_id: requestId, acknowledge_data_loss: true };
}

export function gcpStoreRemovalIntent(storage, value) {
  if (!gcpRemovalIntentValid(value, value?.guide_id)) throw new Error('The removal request does not identify this original cloud computer.');
  const key = gcpRemovalIntentKey(value.guide_id), encoded = JSON.stringify(value);
  storage.setItem(key, encoded);
  if (storage.getItem(key) !== encoded) throw new Error('The removal request could not be saved. No removal request was sent.');
}

export function gcpLoadRemovalIntent(storage, guideId) {
  const raw = storage.getItem(gcpRemovalIntentKey(guideId));
  if (raw === null) return null;
  let value;
  try { value = JSON.parse(raw); } catch { throw new Error('The saved removal request is unreadable. Preserve it and recheck original cloud status.'); }
  if (!gcpRemovalIntentValid(value, guideId)) throw new Error('The saved removal request does not identify this setup. Preserve it and recheck original cloud status.');
  return value;
}
