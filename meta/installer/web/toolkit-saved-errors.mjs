const messages = Object.freeze({
  saved_settings_host_unavailable: "Saved tools review is not enabled on this host. Ask the host administrator to enable it, then reopen this panel.",
  saved_settings_host_changed: "The host's review configuration changed. Reopen the selected request, check the host details, and approve it again.",
  saved_settings_manifest_changed: "The recovery file changed. Choose the current file and review the selected request again.",
  saved_settings_request_changed: "The saved request changed. Choose the current recovery file and check the full request again.",
  saved_settings_approved_request_changed: "The saved request changed after approval. Choose the current recovery file and approve the full request again.",
  saved_settings_approval_changed: "This approval no longer matches the selected request or host. Reopen the request and check it again.",
  saved_settings_audience_approval_required: "Check the full request and confirm that it contains no secrets and is suitable for this setup's audience.",
  saved_settings_preview_required: "Choose a request and check its full contents before reviewing current settings.",
  saved_settings_guide_version_required: "Reopen Saved tools from the current setup before reviewing a request.",
  saved_settings_request_not_selected: "Choose one configured connection request from this recovery file.",
  saved_settings_manifest_too_large: "Choose a saved Toolkit recovery JSON file of at most 64 KiB.",
  saved_settings_manifest_refused: "This recovery file could not be verified against its saved requests. Choose the original Toolkit recovery file; its saved workspace must still be available.",
  saved_settings_family_unavailable: "This request's tool family is not enabled on the current host. Ask the host administrator to enable it, then review the request again.",
  saved_settings_profile_unavailable: "This saved profile is unavailable in the current tooling. Choose another request or discuss a replacement with the guide.",
  saved_settings_review_in_progress: "A current-settings review is already being prepared. Wait for its result before choosing another request.",
  saved_settings_review_interrupted: "The review was interrupted. Reopen Saved tools to check the retained attempt; no preparation was automatically repeated.",
  saved_settings_preparation_refused: "The host could not prepare this settings review. Ask the host administrator to check the current Toolkit setup before starting a new review.",
  saved_settings_preparation_unconfirmed: "Preparation could not be confirmed. Keep Forge running and check the retained attempt before starting another review.",
  saved_settings_current_owner_unavailable: "The current Toolkit review service could not start. Ask the host administrator to check its configuration, then reopen Saved tools.",
  saved_settings_current_family_changed: "The current tool family no longer matches this request. Reopen the request and check the host configuration before approving it again.",
  saved_settings_current_review_unconfirmed: "The current settings review could not be confirmed. Reopen Saved tools to check the retained attempt before starting another review.",
  saved_settings_worker_settlement_unconfirmed: "The review worker has not been confirmed closed. Keep Forge running and ask the host administrator to check the retained worker before starting another review.",
  saved_settings_storage_unavailable: "Saved tools could not be read or saved. Ask the host administrator to check Forge's private storage, then reopen this panel.",
  saved_settings_review_too_large: "This settings review is too large to save. Choose a smaller request or ask the host administrator for help.",
});

export const SAVED_TOOLKIT_ERROR_FALLBACK = "This saved-tools request could not be completed. Reopen Saved tools to check the retained request. Your conversation and draft are kept.";

export function savedToolkitErrorMessage(value) {
  const code = value instanceof Error ? value.message : value;
  return typeof code === "string" && Object.hasOwn(messages, code)
    ? messages[code] : SAVED_TOOLKIT_ERROR_FALLBACK;
}
