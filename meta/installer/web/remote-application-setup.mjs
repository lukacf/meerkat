import { applicationSetupKey, applicationSetupReadback, applicationSetupView } from "./application-setup.mjs";
import { remoteProductControl, remoteControlBindingKey, remoteReceiptBinding, remoteReceiptCurrent } from "./remote-deployment.mjs";

export function remoteApplicationSetupKey(receipt, options, build, state, blocked = false) {
  const product = remoteProductControl(receipt, state);
  if (!product || !remoteReceiptCurrent(receipt, options, build, state, blocked)) return null;
  const localKey = applicationSetupKey(product.factory_status, state);
  return localKey ? JSON.stringify([remoteReceiptBinding(receipt), remoteControlBindingKey(product.binding), localKey]) : null;
}

export function remoteApplicationSetupReadback(value, receipt, options, build, state, blocked = false) {
  if (!remoteApplicationSetupKey(receipt, options, build, state, blocked) ||
      value?.source_digest !== receipt.accepted_source_digest ||
      remoteControlBindingKey(value.remote_control_binding) !== remoteControlBindingKey(receipt.product_control.binding)) return null;
  return applicationSetupReadback(value, receipt.product_control.factory_status, state);
}

export function remoteApplicationSetupView({ receipt, options, build, state, confirmed, pending, disabled, snapshot }) {
  const key = remoteApplicationSetupKey(receipt, options, build, state);
  if (!key) return null;
  const actual = receipt.product_control.factory_status, localKey = applicationSetupKey(actual, state);
  const checked = snapshot?.key === key && remoteApplicationSetupReadback(snapshot.value, receipt, options, build, state, disabled);
  const view = applicationSetupView({ receipt: actual, state, confirmed, pending: pending || !!receipt.pending_operation,
    snapshot: { key: snapshot?.key === key ? localKey : null, pending: snapshot?.pending,
      value: checked ? snapshot.value : null } });
  // Restore capability remains the existing host's checked observation.
  return view?.background && receipt.can_start_background !== true ? { ...view, background: undefined } : view;
}
