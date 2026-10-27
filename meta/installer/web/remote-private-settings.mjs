import { remoteReceiptBinding, remoteReceiptCurrent, remoteObservationAvailable, remotePrivateOptions, remoteProductSelected, remoteProductControl, remoteControlBindingKey, remoteProductActionSettled } from "./remote-deployment.mjs";
import { deploymentRequiredFields, deploymentFieldIssues, deploymentRemovalKeys } from "./deployment-configuration.mjs";

export function remoteInstalledCanConfigure(receipt, options, build, state, blocked = false) {
  if (remoteProductSelected(receipt)) {
    const product = remoteProductControl(receipt, state), actual = product?.factory_status;
    return !blocked && !!product && !receipt.pending_operation && remoteReceiptCurrent(receipt, options, build, state) &&
      receipt.private_settings_supported === true && receipt.can_configure === true && receipt.status === "stopped" &&
      receipt.source_current === true && actual.status === "stopped" && actual.execution_mode === "interactive" && !actual.service &&
      actual.process_live === false && actual.runtime_pid == null;
  }
  return !blocked && remoteReceiptCurrent(receipt, options, build, state) && remoteObservationAvailable(receipt) &&
    receipt.private_settings_supported === true && receipt.can_configure === true &&
    receipt.status === "stopped" && receipt.source_current === true &&
    receipt.worker_live === false && receipt.cleanup_confirmed === true;
}

export function remoteInstalledSettingsOptions(value, receipt, options, build, state, blocked = false) {
  if (!remoteInstalledCanConfigure(receipt, options, build, state, blocked) ||
      value?.scope !== "verified-remote-installed-private-configuration" || value.supported !== true ||
      value.source_verified !== true || value.requires_stopped !== true ||
      value.can_use_host_provider_credentials !== false || value.version !== state.version ||
      value.deployment_id !== receipt.deployment_id || value.build_id !== receipt.build_id ||
      value.source_digest !== receipt.accepted_source_digest || value.package_sha256 !== receipt.package_sha256 ||
      value.guide_version !== receipt.guide_version || !Number.isInteger(value.console_port) ||
      value.console_port < 1024 || value.console_port > 65535 || !Array.isArray(value.fields) ||
      !value.fields.every((field) => typeof field.configured === "boolean" && typeof field.removable === "boolean")) return null;
  if (remoteProductSelected(receipt) && remoteControlBindingKey(value.remote_control_binding) !==
      remoteControlBindingKey(receipt.product_control.binding)) return null;
  const checked = remotePrivateOptions({ preflight: { ready: true }, fields: value.fields,
    ...(Object.hasOwn(value, "console_scope") ? { console_scope: value.console_scope } : {}) });
  if (!checked) return null;
  return { ...checked, fields: checked.fields.map((field, index) => ({ ...field,
    configured: value.fields[index].configured, removable: value.fields[index].removable })),
    version: value.version, deployment_id: value.deployment_id, build_id: value.build_id,
    source_digest: value.source_digest, package_sha256: value.package_sha256,
    guide_version: value.guide_version, console_port: value.console_port,
    ...(remoteProductSelected(receipt) ? { remote_control_binding: value.remote_control_binding } : {}) };
}

export function remoteInstalledSettingsBody(value, receipt, options, build, state, values, removed, blocked = false) {
  if (!remoteInstalledCanConfigure(receipt, options, build, state, blocked) || !value ||
      value.version !== state.version || value.deployment_id !== receipt.deployment_id ||
      value.build_id !== build.build_id || value.source_digest !== build.source_digest ||
      value.package_sha256 !== receipt.package_sha256) return null;
  if (remoteProductSelected(receipt) && remoteControlBindingKey(value.remote_control_binding) !==
      remoteControlBindingKey(receipt.product_control.binding)) return null;
  const remove_fields = deploymentRemovalKeys(value, removed);
  if (!remove_fields || deploymentRequiredFields(value, values).length || deploymentFieldIssues(value, values, remove_fields).length) return null;
  const settings = {};
  for (const field of value.fields) {
    if (!field.automatic && !remove_fields.includes(field.key) && typeof values[field.key] === "string" && values[field.key] !== "")
      settings[field.key] = values[field.key];
  }
  if (!Object.keys(settings).length && !remove_fields.length) return null;
  return { version: value.version, deployment_id: value.deployment_id, build_id: value.build_id,
    source_digest: value.source_digest, package_sha256: value.package_sha256, settings, remove_fields };
}

export function remoteInstalledSettingsSaved(next, receipt, options, build, state, identity) {
  if (remoteProductSelected(receipt) && (!remoteProductControl(next, state) ||
      remoteControlBindingKey(next.product_control.binding) !== remoteControlBindingKey(receipt.product_control.binding) ||
      remoteProductActionSettled(next, state, identity, "configure")?.outcome !== "returned")) return false;
  return next?.configuration_saved === true && remoteReceiptBinding(next) === remoteReceiptBinding(receipt) &&
    remoteInstalledCanConfigure(next, options, build, state);
}

export function remoteInstalledSettingsPacket(body, receipt, state, identity) {
  if (!body) return null;
  if (!remoteProductSelected(receipt)) return body;
  return remoteProductControl(receipt, state) && !receipt.pending_operation &&
    typeof identity === "string" && /^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$/.test(identity)
    ? { ...body, action_request_id: identity } : null;
}
